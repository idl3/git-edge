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

const MAX_ENTRIES: usize = 2_000_000;
const MAX_OBJ: u64 = 16 << 20; // A7
const WINDOW: u64 = 8 << 20;
const CACHE: usize = 48 << 20; // 16 MiB resolved LRU + 32 MiB external bases as one cache
const MAX_DEPTH: usize = 64; // git's default delta depth is 50

fn unpack(m: impl Into<String>) -> Error {
    Error::Unpack(m.into())
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error::Internal(e.to_string())
}

/// Pass A. Each zlib boundary comes from a resumable `Decompress` whose `total_in` survives awaits.
pub async fn stream_to_pending(
    body: &mut BodyReader,
    bucket: &Bucket,
    push: &PushId,
    budget: &mut ReqBudget,
) -> Result<(Vec<EntryRec>, u32), Error> {
    let mut hasher = gix_hash::hasher(H::Sha1);
    let mut out = RawWriter::create(bucket, keys::pending(&bucket.repo, push), budget).await?;
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
    take(body, &mut hasher, &mut out, 12, budget).await?;
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
        take(body, &mut hasher, &mut out, hlen, budget).await?;
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
            take(body, &mut hasher, &mut out, used, budget).await?;
            clen = clen.saturating_add(used as u64);
        }
        if z.total_out() != e.decompressed_size {
            return Err(unpack(format!("object at {pos}: size mismatch")));
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
    out.finish(budget).await?;
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

/// In-pack base at `start`: walk the chain back to a cached or full entry, then apply forward.
async fn resolve_at(
    cx: &mut Cx<'_>,
    entries: &[EntryRec],
    start: u64,
    budget: &mut ReqBudget,
) -> Result<Obj, Error> {
    let (mut off, mut chain) = (start, Vec::new());
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
        match rec.kind_or_delta {
            Header::OfsDelta { base_distance } => {
                chain.push((off, raw));
                off = Header::verified_base_pack_offset(off, base_distance)
                    .ok_or_else(|| unpack("bad ofs-delta"))?;
            }
            Header::RefDelta { base_id } => {
                chain.push((off, raw));
                match cx.by_id.get(&base_id).copied() {
                    Some(j) => off = entries.get(j).ok_or_else(|| internal("idx"))?.offset,
                    None => break external(cx, base_id, budget).await?,
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
    Ok((kind, data))
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
    };
    let (mut pre, mut sum) = (Vec::new(), 0u64);
    for (id, loc) in external_bases {
        sum = sum.saturating_add(u64::from(loc.len));
        if sum > 32 << 20 {
            break;
        }
        pre.push((*id, loc.clone()));
    }
    for (id, entry) in bucket.read_entries(&pre, budget).await? {
        let (k, d) = codec::decode_entry(&entry)?;
        cx.cache.put(Key::Id(id), k, Rc::new(d));
    }
    let need_ids = entries.iter().any(|r| matches!(r.kind_or_delta, Header::RefDelta { .. }));
    let mut rows = Vec::new();
    for (i, rec) in entries.iter().enumerate() {
        let raw = cx.win.entry(rec, budget).await?;
        let base = match rec.kind_or_delta {
            Header::OfsDelta { base_distance } => Some(
                resolve_at(
                    &mut cx,
                    entries,
                    Header::verified_base_pack_offset(rec.offset, base_distance)
                        .ok_or_else(|| unpack("bad ofs-delta"))?,
                    budget,
                )
                .await?,
            ),
            Header::RefDelta { base_id } => Some(match cx.by_id.get(&base_id).copied() {
                Some(j) => {
                    resolve_at(&mut cx, entries, entries.get(j).ok_or_else(|| internal("idx"))?.offset, budget)
                        .await?
                }
                None => external(&mut cx, base_id, budget).await?,
            }),
            _ => None,
        };
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
            sink.post(&rows, budget).await?;
            rows.clear();
        }
    }
    Ok(rows)
}

/// Object references for the 2.5 connectivity check (1.4): commit -> tree + parents,
/// tree -> entries except gitlinks, tag -> target.
pub fn extract_links(kind: Kind, data: &[u8]) -> Result<Vec<ObjectId>, Error> {
    let mut out = Vec::new();
    match kind {
        Kind::Commit => {
            let mut it = gix_object::CommitRefIter::from_bytes(data, gix_hash::Kind::Sha1);
            if let Ok(t) = it.tree_id() {
                out.push(t);
            }
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
            if let Ok(t) = gix_object::TagRef::from_bytes(data, gix_hash::Kind::Sha1) {
                out.push(t.target());
            }
        }
        Kind::Blob => {}
    }
    Ok(out)
}
