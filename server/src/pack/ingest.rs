//! Pass A (stream_to_pending) and pass B (resolve_and_normalize) of ingest (CONTRACTS.md 2.4, A10).
//! Runs in the edge Worker. Ported from proofs-v2/streaming-pack-parser.md.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::rc::Rc;

use gix_hash::{Kind as H, ObjectId};
use gix_object::Kind;
use gix_pack::data::{entry::Header, Entry as PackEntry, File};
use gix_zlib::{Compression, Decompress, FlushDecompress, Inflate, Status};

use crate::edge::BodyReader;
use crate::error::Error;
use crate::store::{codec, keys, Bucket, ObjLoc, ObjRow, PackWriter, PushId, RawWriter};
use crate::ReqBudget;

use super::run::IndexSink;

pub struct EntryRec {
    pub offset: u64,
    pub header_len: u8,
    pub kind_or_delta: Header,
    pub compressed_len: u32,
}

// EntryRec is ~40 B (offset + Header enum + two lengths): the record vec alone must
// leave room for Cache (48 MiB) + window + part buffer inside a 128 MiB isolate
const MAX_ENTRIES: usize = 1_000_000;
const MAX_OBJ: u64 = 16 << 20; // A7
/// One entry's *compressed* wire size. `decompressed_size` ≤ 16 MiB bounds output but a
/// deflate stream can carry ~5 bytes-in/0 bytes-out padding, so an entry could claim a
/// ~2 GiB compressed length — which pass B would then range-read into memory whole.
/// A real 16 MiB object compresses to < ~17 MiB; 32 MiB is generous headroom.
const MAX_ENTRY_WIRE: u64 = 32 << 20;
/// Total compressed bytes held by in-flight delta chains across every nested
/// resolve_at frame — the aggregate bound MAX_CHAIN_BYTES alone can't provide.
const MAX_CHAIN_LIVE: u64 = 128 << 20;
const MAX_PENDING: u64 = 2 << 30; // 2 GiB compressed ceiling on one pushed pack
const WINDOW: u64 = 8 << 20;
const CACHE: usize = 48 << 20; // 16 MiB resolved LRU + 32 MiB external bases as one cache
const MAX_DEPTH: usize = 64; // git's default delta depth is 50
const MAX_CHAIN_BYTES: u64 = 64 << 20; // chain holds raw compressed delta bodies

fn unpack(m: impl Into<String>) -> Error {
    Error::Unpack(m.into())
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error::Internal(e.to_string())
}

/// Pass A. Each zlib boundary comes from a resumable `Decompress` whose `total_in` survives awaits.
/// Any error after the MPU is created aborts it — a dropped RawWriter would orphan the
/// upload and its already-uploaded parts in R2.
pub async fn stream_to_pending(
    body: &mut BodyReader,
    bucket: &Bucket,
    push: &PushId,
    budget: &mut ReqBudget,
) -> Result<(Vec<EntryRec>, u32), Error> {
    let mut out = RawWriter::create(bucket, keys::pending(&bucket.repo, push), budget).await?;
    match stream_inner(body, &mut out, budget).await {
        Ok(r) => match out.finish(budget).await {
            Ok(()) => Ok(r),
            Err(e) => Err(e), // finish aborts the MPU itself on failure
        },
        Err(e) => {
            out.abort().await;
            Err(e)
        }
    }
}

async fn stream_inner(
    body: &mut BodyReader,
    out: &mut RawWriter,
    budget: &mut ReqBudget,
) -> Result<(Vec<EntryRec>, u32), Error> {
    let mut hasher = gix_hash::hasher(H::Sha1);
    if !body.fill(12).await? {
        return Err(unpack("pack header truncated"));
    }
    let head: [u8; 12] = body
        .buffered()
        .get(..12)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| unpack("pack header"))?;
    let (version, count) =
        gix_pack::data::header::decode(&head).map_err(|e| unpack(e.to_string()))?;
    if version != gix_pack::data::Version::V2 {
        return Err(unpack("pack version"));
    }
    take(body, &mut hasher, out, 12, budget).await?;
    let (mut recs, mut pos, mut z, mut sink) = (Vec::new(), 12u64, Decompress::new(), vec![0u8; 64 << 10]);
    for _ in 0..count {
        if recs.len() >= MAX_ENTRIES {
            return Err(unpack("too many objects"));
        }
        body.fill(30).await?;
        let e = PackEntry::from_bytes(body.buffered(), pos, H::Sha1)
            .map_err(|e| unpack(format!("entry at {pos}: {e}")))?;
        if e.decompressed_size > MAX_OBJ {
            return Err(Error::Limit("object too large (16 MiB max)".into()));
        }
        let hlen = e.header_size();
        take(body, &mut hasher, out, hlen, budget).await?;
        let (mut clen, mut st) = (0u64, Status::Ok);
        z.reset();
        while st != Status::StreamEnd {
            if body.buffered().is_empty() && !body.fill(1).await? {
                return Err(unpack("pack truncated inside object"));
            }
            let (bi, bo) = (z.total_in(), z.total_out());
            st = z
                .decompress(body.buffered(), &mut sink, FlushDecompress::None)
                .map_err(|e| unpack(format!("zlib at {pos}: {e}")))?;
            let used = usize::try_from(z.total_in().saturating_sub(bi)).map_err(internal)?;
            if used == 0 && z.total_out() == bo {
                return Err(unpack(format!("zlib stalled at {pos}")));
            }
            take(body, &mut hasher, out, used, budget).await?;
            clen = clen.saturating_add(used as u64);
            if body.total > MAX_PENDING {
                return Err(Error::Limit("pack exceeds the 2 GiB limit".into()));
            }
        }
        if z.total_out() != e.decompressed_size {
            return Err(unpack(format!("object at {pos}: size mismatch")));
        }
        if clen > MAX_ENTRY_WIRE {
            return Err(unpack(format!("entry at {pos}: wire size exceeds 32 MiB")));
        }
        recs.push(EntryRec {
            offset: pos,
            header_len: u8::try_from(hlen).map_err(|_| unpack("header too long"))?,
            kind_or_delta: e.header,
            compressed_len: u32::try_from(clen).map_err(|_| Error::Limit("entry too large".into()))?,
        });
        pos = pos.saturating_add(hlen as u64).saturating_add(clen);
    }
    if !body.fill(20).await? {
        return Err(unpack("pack trailer truncated"));
    }
    let want = hasher.try_finalize().map_err(|_| unpack("sha1 collision in pack"))?;
    if body.buffered().get(..20) != Some(want.as_slice()) {
        return Err(unpack("bad pack checksum"));
    }
    out.append(want.as_slice());
    body.consume(20);
    if body.fill(1).await? {
        return Err(Error::Protocol("bytes after pack trailer".into()));
    }
    Ok((recs, count))
}

async fn take(
    body: &mut BodyReader,
    h: &mut gix_hash::Hasher,
    out: &mut RawWriter,
    n: usize,
    budget: &mut ReqBudget,
) -> Result<(), Error> {
    let bytes = body.buffered().get(..n).ok_or_else(|| unpack("pack truncated"))?;
    h.update(bytes);
    out.append(bytes);
    body.consume(n);
    out.flush_if_full(budget).await
}

// ---- Pass B ----

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Off(u64),
    Id(ObjectId),
}
type Obj = (Kind, Rc<Vec<u8>>);

#[derive(Default)]
struct Cache {
    bytes: usize,
    map: HashMap<Key, Obj>,
    order: VecDeque<Key>,
}
impl Cache {
    fn get(&self, k: &Key) -> Option<Obj> {
        self.map.get(k).cloned()
    }
    fn put(&mut self, k: Key, kind: Kind, d: Rc<Vec<u8>>) -> Obj {
        self.bytes = self.bytes.saturating_add(d.len());
        self.order.push_back(k);
        self.map.insert(k, (kind, d.clone()));
        while self.bytes > CACHE {
            match self.order.pop_front().and_then(|k| self.map.remove(&k)) {
                Some((_, v)) => self.bytes = self.bytes.saturating_sub(v.len()),
                None => break,
            }
        }
        (kind, d)
    }
}

struct Window<'a> {
    bucket: &'a Bucket,
    key: &'a str,
    pack_len: u64,
    start: u64,
    buf: Vec<u8>,
}
impl Window<'_> {
    /// Raw bytes of one entry; slides to [offset, +8 MiB) with one range read when outside.
    async fn entry(&mut self, rec: &EntryRec, budget: &mut ReqBudget) -> Result<Vec<u8>, Error> {
        let len = u64::from(rec.header_len).saturating_add(u64::from(rec.compressed_len));
        // pass A enforces MAX_ENTRY_WIRE; keep the bound explicit here so a huge range
        // read can never be issued even if the invariant is ever broken upstream
        if len > MAX_ENTRY_WIRE.saturating_add(64) {
            return Err(unpack("entry window too large"));
        }
        let lo = match rec
            .offset
            .checked_sub(self.start)
            .filter(|lo| lo.saturating_add(len) <= self.buf.len() as u64)
        {
            Some(lo) => lo,
            None => {
                let n = len.max(WINDOW).min(self.pack_len.saturating_sub(rec.offset));
                self.buf = self.bucket.read_range(self.key, rec.offset, n, budget).await?;
                self.start = rec.offset;
                0
            }
        };
        let (a, b) = (
            usize::try_from(lo).map_err(internal)?,
            usize::try_from(lo.saturating_add(len)).map_err(internal)?,
        );
        self.buf.get(a..b).map(<[u8]>::to_vec).ok_or_else(|| Error::Storage("entry outside window".into()))
    }
}

struct Cx<'a> {
    win: Window<'a>,
    cache: Cache,
    z: Inflate,
    by_id: HashMap<ObjectId, usize>,
    external: &'a HashMap<ObjectId, ObjLoc>,
    /// Nested `resolve_at` depth (ref-delta -> ref-delta hops). Each level keeps its
    /// `chain` alive across the inner await, so depth without a byte bound is an OOM.
    res_depth: u32,
    /// Compressed bytes held by in-flight delta chains across ALL nested resolve_at
    /// calls — decremented when a chain finishes or aborts.
    chain_live: u64,
}

/// One decode_entry over `[PACK v2][base as a level-0 zlib full entry][delta re-headed as ofs-delta][20 zero bytes]`.
fn decode_mini(z: &mut Inflate, base: Option<(Kind, &[u8])>, raw: &[u8]) -> Result<(Kind, Vec<u8>), Error> {
    let mut mini = b"PACK\0\0\0\x02\0\0\0\x02".to_vec();
    if let Some((k, d)) = base {
        let hk = match k {
            Kind::Commit => Header::Commit,
            Kind::Tree => Header::Tree,
            Kind::Blob => Header::Blob,
            Kind::Tag => Header::Tag,
        };
        hk.write_to(d.len() as u64, &mut mini).map_err(internal)?;
        let mut w = gix_zlib::stream::deflate::Write::new(mini, Compression::new(0).unwrap_or_default());
        w.write_all(d).and_then(|_| std::io::Write::flush(&mut w)).map_err(internal)?;
        mini = w.into_inner();
    }
    let (at, e) = (
        mini.len() as u64,
        PackEntry::from_bytes(raw, 0, H::Sha1).map_err(|e| unpack(e.to_string()))?,
    );
    let body = raw.get(e.header_size()..).ok_or_else(|| unpack("short entry"))?;
    let hdr = match (e.header.is_delta(), base) {
        (true, Some(_)) => Header::OfsDelta { base_distance: at.saturating_sub(12) },
        (true, None) => return Err(unpack("delta without base")),
        (false, _) => e.header,
    };
    hdr.write_to(e.decompressed_size, &mut mini).map_err(internal)?;
    mini.extend_from_slice(body);
    mini.extend_from_slice(&[0u8; 20]);
    let file = File::<&[u8]>::from_data(&mini, "mini".into(), H::Sha1)
        .map_err(internal)?
        .with_alloc_limit_bytes(usize::try_from(MAX_OBJ).ok());
    let mut out = Vec::new();
    let oc = file
        .decode_entry(
            file.entry(at).map_err(|e| unpack(e.to_string()))?,
            &mut out,
            z,
            &|_, _| None,
            &mut gix_pack::cache::Never,
        )
        .map_err(|e| unpack(format!("delta: {e}")))?;
    Ok((oc.kind, out))
}

/// A chain walk that can't proceed because a base id isn't resolvable *yet* — the base
/// may be a later in-pack entry (forward REF_DELTA), so the caller defers rather than
/// erroring. `Await` is only raised for ids not in `by_id`, `external`, or the caches.
pub enum Base {
    Ready(Obj),
    Await(ObjectId),
}

/// Thin-pack base (2.4): prefetched cache, else one coalesced read of its live location.
async fn external(cx: &mut Cx<'_>, id: ObjectId, budget: &mut ReqBudget) -> Result<Obj, Error> {
    if let Some(hit) = cx.cache.get(&Key::Id(id)) {
        return Ok(hit);
    }
    let loc = cx
        .external
        .get(&id)
        .ok_or_else(|| unpack(format!("missing base {id}")))?
        .clone();
    let (_, entry) = cx
        .win
        .bucket
        .read_entries(&[(id, loc)], budget)
        .await?
        .pop()
        .ok_or_else(|| Error::Storage("base read".into()))?;
    let (k, d) = codec::decode_entry(&entry)?;
    Ok(cx.cache.put(Key::Id(id), k, Rc::new(d)))
}

/// Resolve a ref-delta base id: resolved in-pack entry -> chain walk; repo base -> read;
/// unknown -> `Await` so the caller can defer until the entry is processed.
async fn base_by_id(
    cx: &mut Cx<'_>,
    entries: &[EntryRec],
    base_id: ObjectId,
    budget: &mut ReqBudget,
) -> Result<Base, Error> {
    if let Some(hit) = cx.cache.get(&Key::Id(base_id)) {
        return Ok(Base::Ready(hit));
    }
    match cx.by_id.get(&base_id).copied() {
        Some(j) => {
            // async recursion (resolve_at -> base_by_id -> resolve_at) needs boxing;
            // bounded twice: nested hop depth AND shared in-flight chain bytes
            if cx.res_depth >= MAX_DEPTH as u32 {
                return Err(unpack("delta chain too deep"));
            }
            cx.res_depth += 1;
            let r = Box::pin(resolve_at(cx, entries, entries.get(j).ok_or_else(|| internal("idx"))?.offset, budget)).await;
            cx.res_depth -= 1;
            r
        }
        None if cx.external.contains_key(&base_id) => {
            Ok(Base::Ready(external(cx, base_id, budget).await?))
        }
        None => Ok(Base::Await(base_id)),
    }
}

/// In-pack base at `start`: walk the chain back to a cached or full entry, then apply forward.
async fn resolve_at(
    cx: &mut Cx<'_>,
    entries: &[EntryRec],
    start: u64,
    budget: &mut ReqBudget,
) -> Result<Base, Error> {
    let (mut off, mut chain, mut chain_bytes) = (start, Vec::new(), 0u64);
    let (kind, mut data) = loop {
        if let Some(hit) = cx.cache.get(&Key::Off(off)) {
            break hit;
        }
        if chain.len() >= MAX_DEPTH {
            return Err(unpack("delta chain too deep"));
        }
        let i = entries
            .binary_search_by_key(&off, |r| r.offset)
            .map_err(|_| unpack("delta base is not an entry"))?;
        let rec = entries.get(i).ok_or_else(|| internal("idx"))?;
        let raw = cx.win.entry(rec, budget).await?;
        // the chain holds each delta's compressed bytes: 64 × ~16 MiB worst case is a
        // GiB-scale allocation — bound the bytes, not just the depth. chain_live bounds
        // the SAME memory summed across every nested resolve_at frame.
        chain_bytes = chain_bytes.saturating_add(raw.len() as u64);
        cx.chain_live = cx.chain_live.saturating_add(raw.len() as u64);
        if chain_bytes > MAX_CHAIN_BYTES || cx.chain_live > MAX_CHAIN_LIVE {
            cx.chain_live = cx.chain_live.saturating_sub(chain_bytes);
            return Err(unpack("delta chain too large"));
        }
        match rec.kind_or_delta {
            Header::OfsDelta { base_distance } => {
                chain.push((off, raw));
                off = Header::verified_base_pack_offset(off, base_distance)
                    .ok_or_else(|| unpack("bad ofs-delta"))?;
            }
            Header::RefDelta { base_id } => {
                chain.push((off, raw));
                match base_by_id(cx, entries, base_id, budget).await? {
                    // an unresolved in-pack base: abandon the walk; the whole entry defers
                    Base::Await(id) => {
                        cx.chain_live = cx.chain_live.saturating_sub(chain_bytes);
                        return Ok(Base::Await(id));
                    }
                    Base::Ready(obj) => break obj,
                }
            }
            _ => {
                let (k, d) = decode_mini(&mut cx.z, None, &raw)?;
                break cx.cache.put(Key::Off(off), k, Rc::new(d));
            }
        }
    };
    while let Some((o, raw)) = chain.pop() {
        let (_, d) = decode_mini(&mut cx.z, Some((kind, &data)), &raw)?;
        data = cx.cache.put(Key::Off(o), kind, Rc::new(d)).1;
    }
    cx.chain_live = cx.chain_live.saturating_sub(chain_bytes);
    Ok(Base::Ready((kind, data)))
}

/// `sink` posts `/_do/push/index` in 10,000-row batches and collects links (2.5).
pub async fn resolve_and_normalize(
    bucket: &Bucket,
    pending_key: &str,
    entries: &[EntryRec],
    external_bases: &HashMap<ObjectId, ObjLoc>,
    out: &mut PackWriter,
    sink: &mut IndexSink<'_>,
    budget: &mut ReqBudget,
) -> Result<Vec<ObjRow>, Error> {
    let pack_len = entries.last().map_or(32, |r| {
        r.offset
            .saturating_add(u64::from(r.header_len))
            .saturating_add(u64::from(r.compressed_len))
            .saturating_add(20)
    });
    let mut cx = Cx {
        win: Window { bucket, key: pending_key, pack_len, start: 0, buf: Vec::new() },
        cache: Cache::default(),
        z: Inflate::default(),
        by_id: HashMap::new(),
        external: external_bases,
        res_depth: 0,
        chain_live: 0,
    };
    let (mut pre, mut sum) = (Vec::new(), 0u64);
    for (id, loc) in external_bases {
        sum = sum.saturating_add(u64::from(loc.len));
        if sum > 32 << 20 {
            break;
        }
        pre.push((*id, loc.clone()));
    }
    // prefetch is opportunistic: `read_entries` bounds *merged span* bytes (gaps
    // included), which `pre`'s plain size sum can't predict — on Limit just skip the
    // batch; every base still resolves lazily through `external()`
    if let Ok(entries) = bucket.read_entries(&pre, budget).await {
        for (id, entry) in entries {
            let (k, d) = codec::decode_entry(&entry)?;
            cx.cache.put(Key::Id(id), k, Rc::new(d));
        }
    }
    let need_ids = entries.iter().any(|r| matches!(r.kind_or_delta, Header::RefDelta { .. }));
    let mut rows = Vec::new();
    // A REF_DELTA may name a base that appears LATER in the same pack — including
    // mid-chain inside another delta. `one_entry` returns `Await(base)` for those;
    // resolving an entry wakes exactly its waiters (index-pack parity, O(n) not O(n^2)).
    let mut pending: HashMap<ObjectId, Vec<usize>> = HashMap::new();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    let mut i = 0;
    while i < order.len() {
        let at = order[i];
        i += 1;
        match one_entry(&mut cx, entries, at, need_ids, out, sink, &mut rows, budget).await? {
            Step::Done(id) => {
                if let Some(ws) = pending.remove(&id) {
                    order.extend(ws);
                }
            }
            Step::Await(base) => {
                pending.entry(base).or_default().push(at);
            }
        }
    }
    if let Some((&base, _)) = pending.iter().next() {
        return Err(unpack(format!("missing base {base}")));
    }
    Ok(rows)
}

/// The per-entry outcome: resolved+emitted (`Done`), or blocked on a base id that a later
/// in-pack entry will produce (`Await`).
enum Step {
    Done(ObjectId),
    Await(ObjectId),
}

/// Resolve + normalize one pack entry: base resolution, decode, hash, links, append, index row.
#[allow(clippy::too_many_arguments)]
async fn one_entry(
    cx: &mut Cx<'_>,
    entries: &[EntryRec],
    i: usize,
    need_ids: bool,
    out: &mut PackWriter,
    sink: &mut IndexSink<'_>,
    rows: &mut Vec<ObjRow>,
    budget: &mut ReqBudget,
) -> Result<Step, Error> {
    let rec = entries.get(i).ok_or_else(|| internal("idx"))?;
    let base = match rec.kind_or_delta {
        Header::OfsDelta { base_distance } => Some(
            resolve_at(
                cx,
                entries,
                Header::verified_base_pack_offset(rec.offset, base_distance)
                    .ok_or_else(|| unpack("bad ofs-delta"))?,
                budget,
            )
            .await?,
        ),
        Header::RefDelta { base_id } => Some(base_by_id(cx, entries, base_id, budget).await?),
        _ => None,
    };
    let base = match base {
        Some(Base::Ready(o)) => Some(o),
        Some(Base::Await(id)) => return Ok(Step::Await(id)),
        None => None,
    };
    let raw = cx.win.entry(rec, budget).await?;
    let (kind, data) =
        decode_mini(&mut cx.z, base.as_ref().map(|(k, d)| (*k, d.as_slice())), &raw)?;
    let id = gix_object::compute_hash(H::Sha1, kind, &data).map_err(|_| unpack("sha1 collision"))?;
    let links = extract_links(kind, &data)?;
    if kind == Kind::Tag {
        if let Some(&t) = links.first() {
            sink.tags.insert(id, t);
        }
    }
    sink.links.extend(links);
    // bound inside the loop too — a single giant tree can spike `links` far past
    // the limit between post() batches otherwise
    if sink.links.len() > super::run::MAX_LINKS {
        return Err(Error::Limit("push references too many objects (1,000,000 max)".into()));
    }
    let (offset, len) = out.append_entry(kind, &data)?;
    out.flush_if_full(budget).await?;
    rows.push(ObjRow {
        sha: id,
        idx: u32::try_from(i).map_err(|_| unpack("too many objects"))?,
        offset,
        len,
        kind,
        size: data.len() as u64,
    });
    if need_ids {
        cx.by_id.insert(id, i);
    }
    cx.cache.put(Key::Off(rec.offset), kind, Rc::new(data));
    if rows.len() >= 10_000 {
        sink.post(rows, budget).await?;
        rows.clear();
    }
    Ok(Step::Done(id))
}

/// Object references for the 2.5 connectivity check (1.4): commit -> tree + parents,
/// tree -> entries except gitlinks, tag -> target.
pub fn extract_links(kind: Kind, data: &[u8]) -> Result<Vec<ObjectId>, Error> {
    let mut out = Vec::new();
    match kind {
        Kind::Commit => {
            let mut it = gix_object::CommitRefIter::from_bytes(data, gix_hash::Kind::Sha1);
            // strict: a commit whose tree header doesn't parse is not a valid commit —
            // storing it would hard-fail every later fetch that walks it
            out.push(it.tree_id().map_err(|e| Error::Unpack(e.to_string()))?);
            out.extend(it.parent_ids());
        }
        Kind::Tree => {
            for e in gix_object::TreeRefIter::from_bytes(data, gix_hash::Kind::Sha1) {
                let e = e.map_err(|e| Error::Unpack(e.to_string()))?;
                if !e.mode.is_commit() {
                    out.push(e.oid.to_owned());
                }
            }
        }
        Kind::Tag => {
            let t = gix_object::TagRef::from_bytes(data, gix_hash::Kind::Sha1)
                .map_err(|e| Error::Unpack(e.to_string()))?;
            out.push(t.target());
        }
        Kind::Blob => {}
    }
    Ok(out)
}
