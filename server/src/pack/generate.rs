//! pack::generate — send_set + write_pack, running inside RepoDo (CONTRACTS.md 1.4, 9).
//! Ported from proofs-v2/partial-clone-filters.md.

use std::collections::{HashMap, HashSet};

use bstr::BString;
use gix_hash::ObjectId;
use gix_object::{CommitRefIter, Kind, TagRef, TreeRefIter};

use crate::error::Error;
use crate::repo_do::RepoDo;
use crate::store::{codec, keys, Bucket, Index, MemFind, ObjLoc, PackId};
use crate::wire::{Filter, Sideband};
use crate::ReqBudget;

pub const MAX_COMMITS: usize = 200_000;
pub const MAX_MEM: usize = 64 << 20;
pub const MAX_OBJECTS: usize = 1_000_000;
const CHUNK: usize = 10_000;
const INFINITE: u32 = 0x7fff_ffff; // git's INFINITE_DEPTH (--unshallow)
const GAP: u64 = 256 << 10;
const WINDOW: u64 = 8 << 20;
const TOO_BIG: &str = "fetch too large for this server; clone instead";

pub struct PackSlice {
    pub pack: PackId,
    pub count: u32,
    pub bytes: u64,
    pub bitmap: Vec<u8>,
}
pub struct Read {
    pub pack: usize,
    pub off: u64,
    pub len: u64,
    pub ents: Vec<(u64, u32)>,
}
#[derive(Default)]
pub struct SendSet {
    pub packs: Vec<PackSlice>,
    pub reads: Vec<Read>,
    pub shallow: Vec<ObjectId>,
    pub unshallow: Vec<ObjectId>,
}
impl SendSet {
    /// Sync, idempotent: one bit per objects.idx (2.3).
    fn mark(&mut self, idx: &Index<'_>, loc: &ObjLoc) -> Result<(), Error> {
        let i = match self.packs.iter().position(|p| p.pack == loc.pack) {
            Some(i) => i,
            None => {
                let (count, bytes) = idx.pack_meta(&loc.pack)?;
                let bits = usize::try_from(count).map_err(|_| Error::Internal("count".into()))?.div_ceil(8);
                self.packs.push(PackSlice { pack: loc.pack.clone(), count, bytes, bitmap: vec![0; bits] });
                self.packs.len().saturating_sub(1)
            }
        };
        let byte = usize::try_from(loc.idx / 8).map_err(|_| Error::Internal("idx".into()))?;
        *self
            .packs
            .get_mut(i)
            .and_then(|p| p.bitmap.get_mut(byte))
            .ok_or_else(|| Error::Storage("idx past pack count".into()))? |= 1u8 << (loc.idx % 8);
        Ok(())
    }
    pub fn count(&self) -> u32 {
        self.packs.iter().flat_map(|p| &p.bitmap).map(|b| b.count_ones() as u32).sum()
    }
}

pub async fn send_set(
    d: &RepoDo,
    bucket: &Bucket,
    wants: &[ObjectId],
    haves: &[ObjectId],
    filter: Option<&Filter>,
    deepen: Option<u32>,
    client_shallow: &[ObjectId],
    budget: &mut ReqBudget,
) -> Result<SendSet, Error> {
    let sql = d.sql();
    let cs: HashSet<ObjectId> = client_shallow.iter().copied().collect();
    let idx = Index(&sql);
    // step 1: unknown haves are dropped
    let acks: HashSet<ObjectId> =
        haves.iter().zip(idx.lookup(haves)?).filter_map(|(h, l)| l.map(|_| *h)).collect();
    let (mut set, mut mem, mut seen, mut buf) =
        (SendSet::default(), MemFind::default(), HashSet::<ObjectId>::new(), Vec::new());
    let (mut commits, mut trees, mut bases, mut edge, mut tags) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::<ObjectId>::new());
    for (id, loc) in wants.iter().zip(idx.lookup(wants)?) {
        // step 1: every want must be live
        let loc = loc.ok_or_else(|| Error::Protocol(format!("upload-pack: not our ref {id}")))?;
        set.mark(&idx, &loc)?;
        seen.insert(*id);
        match loc.kind {
            Kind::Commit => commits.push(*id),
            Kind::Tree => trees.push(*id),
            Kind::Tag => tags.push(*id),
            Kind::Blob => {}
        }
    }
    while !tags.is_empty() {
        load(&idx, bucket, &mut mem, &tags, budget).await?; // peel, one round per nesting level
        for t in std::mem::take(&mut tags) {
            let tag = TagRef::from_bytes(find(&mem, &t, &mut buf)?, gix_hash::Kind::Sha1)
                .map_err(|e| Error::Storage(e.to_string()))?;
            match (tag.target_kind, tag.target()) {
                (Kind::Commit, x) => commits.push(x),
                (Kind::Tree, x) => trees.push(x),
                (Kind::Tag, x) => tags.push(x),
                (Kind::Blob, x) => {
                    if let Some(l) = idx.lookup(&[x])?.pop().flatten() {
                        set.mark(&idx, &l)?;
                        seen.insert(x);
                    }
                }
            }
        }
        mem.clear();
    }
    let cap = deepen.unwrap_or(u32::MAX); // deepen n: depths 0..n-1 are sent
    if deepen == Some(INFINITE) {
        commits.extend(cs.iter().copied()); // --unshallow
    }
    let (mut depth, mut lvl) = (HashMap::<ObjectId, u32>::new(), 0u32);
    while !commits.is_empty() {
        commits.sort_unstable();
        commits.dedup();
        commits.retain(|c| !depth.contains_key(c)); // BFS by level: first depth wins
        if depth.len().saturating_add(commits.len()) > MAX_COMMITS {
            return Err(Error::Limit(TOO_BIG.into()));
        }
        let (locs, mut next) = (load(&idx, bucket, &mut mem, &commits, budget).await?, Vec::new());
        for (c, loc) in std::mem::take(&mut commits).into_iter().zip(locs) {
            let data = find(&mem, &c, &mut buf)?;
            depth.insert(c, lvl);
            let tree = CommitRefIter::from_bytes(data, gix_hash::Kind::Sha1).tree_id().map_err(|e| Error::Storage(e.to_string()))?;
            let parents: Vec<ObjectId> = CommitRefIter::from_bytes(data, gix_hash::Kind::Sha1).parent_ids().collect();
            let (is_ack, is_cs, boundary) =
                (acks.contains(&c), cs.contains(&c), lvl.saturating_add(1) >= cap);
            if is_ack {
                bases.push(tree);
            } else {
                set.mark(&idx, &loc)?;
                trees.push(tree);
            }
            if deepen.is_some() && is_cs && !boundary {
                set.unshallow.push(c);
            } else if deepen.is_some() && boundary && !parents.is_empty() && !is_cs {
                set.shallow.push(c);
            }
            if (deepen.is_some() && boundary) || (deepen.is_none() && (is_ack || is_cs)) {
                continue;
            }
            for p in parents {
                if deepen.is_some() || !acks.contains(&p) {
                    next.push(p);
                } else {
                    edge.push(p);
                }
            }
        }
        mem.clear();
        commits = next;
        lvl = lvl.saturating_add(1);
    }
    edge.sort_unstable();
    edge.dedup();
    edge.truncate(64);
    load(&idx, bucket, &mut mem, &edge, budget).await?;
    for e in &edge {
        bases.push(
            CommitRefIter::from_bytes(find(&mem, e, &mut buf)?, gix_hash::Kind::Sha1)
                .tree_id()
                .map_err(|e| Error::Storage(e.to_string()))?,
        );
    }
    mem.clear();
    bases.sort_unstable();
    bases.dedup();
    bases.truncate(64);
    trees.sort_unstable();
    trees.dedup();
    let mut items: Vec<(ObjectId, Vec<ObjectId>)> =
        trees.into_iter().map(|t| (t, bases.clone())).collect();
    while !items.is_empty() {
        let (mut next, mut blobs) = (Vec::new(), Vec::new());
        for chunk in items.chunks(CHUNK) {
            let ids: Vec<ObjectId> = chunk.iter().map(|(t, _)| *t).collect();
            let mut bs: Vec<ObjectId> = chunk.iter().flat_map(|(_, b)| b.iter().copied()).collect();
            bs.sort_unstable();
            bs.dedup();
            for loc in load(&idx, bucket, &mut mem, &ids, budget).await? {
                set.mark(&idx, &loc)?;
            }
            load(&idx, bucket, &mut mem, &bs, budget).await?;
            for (t, b) in chunk {
                expand_tree(&mem, t, b, filter, &mut seen, &mut next, &mut blobs)?;
            }
            mem.clear();
        }
        for (b, loc) in blobs.iter().zip(idx.lookup(&blobs)?) {
            let loc = loc.ok_or_else(|| Error::Internal(format!("reachable blob {b} is not live")))?;
            if !matches!(filter, Some(Filter::BlobLimit(n)) if loc.size > *n) {
                set.mark(&idx, &loc)?;
            }
        }
        items = next;
    }
    plan_reads(&idx, &mut set, budget)?;
    Ok(set)
}

/// One round of the section 9 loop: sync lookup, one coalesced async read (7.2), decode into mem.
async fn load(
    idx: &Index<'_>,
    bucket: &Bucket,
    mem: &mut MemFind,
    ids: &[ObjectId],
    budget: &mut ReqBudget,
) -> Result<Vec<ObjLoc>, Error> {
    let locs: Vec<(ObjectId, ObjLoc)> = ids
        .iter()
        .zip(idx.lookup(ids)?)
        .map(|(id, l)| l.map(|l| (*id, l)).ok_or_else(|| Error::Internal(format!("reachable {id} is not live"))))
        .collect::<Result<_, _>>()?;
    for (id, entry) in bucket.read_entries(&locs, budget).await? {
        let (k, data) = codec::decode_entry(&entry)?;
        mem.insert(id, k, data);
    }
    if mem.bytes > MAX_MEM {
        Err(Error::Limit(TOO_BIG.into()))
    } else {
        Ok(locs.into_iter().map(|(_, l)| l).collect())
    }
}

fn find<'a>(mem: &'a MemFind, id: &ObjectId, _buf: &mut Vec<u8>) -> Result<&'a [u8], Error> {
    mem.get(id).map(|(_, d)| d).ok_or_else(|| Error::Storage(format!("entry {id} not read")))
}

/// Sync. git's sparse edge pruning: an id held by a counterpart tree is had by the client.
#[allow(clippy::too_many_arguments)]
fn expand_tree(
    mem: &MemFind,
    tree: &ObjectId,
    bases: &[ObjectId],
    filter: Option<&Filter>,
    seen: &mut HashSet<ObjectId>,
    next: &mut Vec<(ObjectId, Vec<ObjectId>)>,
    blobs: &mut Vec<ObjectId>,
) -> Result<(), Error> {
    let (mut had, mut by_name, mut buf) = (HashSet::new(), HashMap::<BString, ObjectId>::new(), Vec::new());
    for b in bases {
        for e in TreeRefIter::from_bytes(find(mem, b, &mut buf)?, gix_hash::Kind::Sha1) {
            let e = e.map_err(|e| Error::Storage(e.to_string()))?;
            had.insert(e.oid.to_owned());
            if e.mode.is_tree() {
                by_name.insert(e.filename.to_owned(), e.oid.to_owned());
            }
        }
    }
    for e in TreeRefIter::from_bytes(find(mem, tree, &mut buf)?, gix_hash::Kind::Sha1) {
        let (e, oid) = {
            let e = e.map_err(|e| Error::Storage(e.to_string()))?;
            (e, e.oid.to_owned())
        };
        if e.mode.is_commit() || had.contains(&oid) || !seen.insert(oid) {
            continue; // gitlink, had by the client, or queued already
        }
        if seen.len() > MAX_OBJECTS {
            return Err(Error::Limit(TOO_BIG.into()));
        }
        if e.mode.is_tree() {
            next.push((oid, by_name.get(e.filename).into_iter().copied().collect()));
        } else if !matches!(filter, Some(Filter::BlobNone)) {
            blobs.push(oid);
        }
    }
    Ok(())
}

/// Sync, after the walk. 7.2 coalescing per pack; bound by the pack's 8 MiB window count.
pub fn plan_reads(idx: &Index<'_>, set: &mut SendSet, budget: &ReqBudget) -> Result<(), Error> {
    let mut reads = Vec::new();
    for (pi, p) in set.packs.iter().enumerate() {
        let locs = idx.entries_of(&p.pack, &p.bitmap)?;
        let plan = coalesce(pi, &locs, GAP);
        reads.extend(if u64::try_from(plan.len()).unwrap_or(u64::MAX) > p.bytes.div_ceil(WINDOW) {
            coalesce(pi, &locs, WINDOW)
        } else {
            plan
        });
    }
    let n = u32::try_from(reads.len()).map_err(|_| Error::Budget)?;
    if budget.used.saturating_add(n) > budget.max_subrequests {
        return Err(Error::Budget); // projection: never a mid-stream abort
    }
    set.reads = reads;
    Ok(())
}

fn coalesce(pack: usize, locs: &[ObjLoc], gap: u64) -> Vec<Read> {
    let mut out: Vec<Read> = Vec::new();
    for l in locs {
        let end = l.offset.saturating_add(u64::from(l.len));
        match out.last_mut() {
            Some(r)
                if l.offset.saturating_sub(r.off.saturating_add(r.len)) < gap
                    && l.offset.saturating_sub(r.off) < WINDOW =>
            {
                r.len = r.len.max(end.saturating_sub(r.off));
                r.ents.push((l.offset, l.len));
            }
            _ => out.push(Read {
                pack,
                off: l.offset,
                len: end.saturating_sub(l.offset),
                ents: vec![(l.offset, l.len)],
            }),
        }
    }
    out
}

/// One step of the response stream: one range read, marked entries copied verbatim (2.1).
pub async fn pack_chunk(
    bucket: &Bucket,
    set: &SendSet,
    i: usize,
    h: &mut gix_hash::Hasher,
    budget: &mut ReqBudget,
) -> Result<Vec<u8>, Error> {
    let Some(r) = set.reads.get(i) else { return Ok(Vec::new()) };
    let pack = set.packs.get(r.pack).ok_or_else(|| Error::Internal("read past packs".into()))?;
    let buf = bucket.read_range(&keys::pack(&bucket.repo, &pack.pack), r.off, r.len, budget).await?;
    let mut out = Vec::new();
    for &(off, len) in &r.ents {
        let start = usize::try_from(off.saturating_sub(r.off)).map_err(|_| Error::Internal("offset".into()))?;
        let e = buf
            .get(start..start.saturating_add(usize::try_from(len).unwrap_or(usize::MAX)))
            .ok_or_else(|| Error::Storage("short range read".into()))?;
        h.update(e);
        out.extend_from_slice(e);
    }
    Ok(out)
}

/// Header, every chunk, trailer (section 9 step 6).
pub async fn write_pack(
    bucket: &Bucket,
    set: &SendSet,
    out: &mut Sideband<'_>,
    budget: &mut ReqBudget,
) -> Result<(), Error> {
    let mut hdr = b"PACK".to_vec();
    hdr.extend_from_slice(&2u32.to_be_bytes());
    hdr.extend_from_slice(&set.count().to_be_bytes());
    let mut h = gix_hash::hasher(gix_hash::Kind::Sha1);
    h.update(&hdr);
    out.data(&hdr);
    for i in 0..set.reads.len() {
        let chunk = pack_chunk(bucket, set, i, &mut h, budget).await?;
        out.data(&chunk);
    }
    out.data(h.try_finalize().map_err(|e| Error::Internal(e.to_string()))?.as_slice());
    Ok(())
}
