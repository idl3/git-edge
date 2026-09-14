//! GC chain (CONTRACTS.md 5): GcMark -> GcConsolidate -> GcSweep, ported from
//! proofs-v2/gc-and-repack-alarm. All durable state lives in tables (marked, gc_frontier,
//! gc_seen, gc_parts, gc.* meta), so a requeued 'running' row resumes mid-flight and a kill
//! mid-slice replays idempotently.

use std::collections::{HashMap, HashSet, VecDeque};

use gix_hash::ObjectId;
use gix_object::Kind;
use worker::{SqlStorage, SqlStorageValue as V};

use super::{enqueue, Job, JobKind, SliceBudget, SliceOutcome};
use crate::error::Error;
use crate::pack::ingest::extract_links;
use crate::platform::now_ms;
use crate::repo_do::{oid, RepoDo};
use crate::store::{
    codec, keys, Bucket, Index, ObjLoc, ObjRow, PackId, PackMeta, PackWriter, WriterCkpt,
};

const ROUND: i64 = 512; // frontier ids per round
const ROWS: usize = 9_000; // insert_objects cap is 10_000 (1.2)
const LOAD: u64 = 32 << 20; // A4 read cap per chunk
const SPANS: usize = 64; // A4: one read_entries call charges <= 64 coalesced spans
const MIN_PART: u64 = 5 << 20; // R2 non-final part minimum
const BATCH_N: usize = 90; // idx batch, <= A6's 100 bound params
/// One buffered read's ceiling: entries with more wire bytes than this are copied
/// fragment-by-fragment instead of through read_entries (which reads whole entries).
const SPAN: u64 = 8 << 20;

#[derive(serde::Deserialize)]
struct N {
    #[allow(dead_code)]
    n: i64,
}
#[derive(serde::Deserialize)]
struct S {
    sha: String,
}
#[derive(serde::Deserialize)]
struct P {
    pack_id: String,
}
#[derive(serde::Deserialize)]
struct B {
    #[allow(dead_code)]
    pack_id: String,
    #[serde(with = "serde_bytes")]
    bitmap: Vec<u8>,
    count: i64,
}
#[derive(serde::Deserialize)]
struct BM {
    #[serde(with = "serde_bytes")]
    bitmap: Vec<u8>,
}
#[derive(serde::Deserialize)]
struct R {
    sha: String,
    pack_id: String,
    idx: i64,
    offset: i64,
    len: i64,
    kind: i64,
    size: i64,
}
#[derive(serde::Deserialize)]
struct E {
    etag: String,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Pos {
    ci: u32,
    idx: u32,
    pack: String,
    upload: String,
    st: Option<WriterCkpt>,
    total: u32,
    lo: i64,
    hi: i64,
}

fn exec(sql: &SqlStorage, q: &str, args: Vec<V>) -> Result<worker::SqlCursor, Error> {
    sql.exec(q, Some(args)).map_err(|e| Error::Storage(e.to_string()))
}
fn js<T: serde::Serialize + ?Sized>(v: &T) -> Result<V, Error> {
    Ok(serde_json::to_string(v).map_err(|e| Error::Internal(e.to_string()))?.into())
}
fn put(d: &RepoDo, k: &str, v: impl Into<String>) -> Result<(), Error> {
    exec(
        &d.sql(),
        "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        vec![k.into(), v.into().into()],
    )?;
    Ok(())
}
fn loc(r: &R) -> Result<(ObjectId, ObjLoc), Error> {
    let u = |n: i64| -> Result<u64, Error> {
        u64::try_from(n).map_err(|_| Error::Internal("negative integer in objects".into()))
    };
    Ok((
        oid(&r.sha)?,
        ObjLoc {
            pack: PackId(r.pack_id.clone()),
            idx: u32::try_from(r.idx).map_err(|_| Error::Internal("idx".into()))?,
            offset: u(r.offset)?,
            len: u32::try_from(r.len).map_err(|_| Error::Internal("len".into()))?,
            kind: crate::store::kind_of(u8::try_from(r.kind).map_err(|_| Error::Internal("kind".into()))?)?,
            size: u(r.size)?,
        },
    ))
}

// ---- marked-bitmap helpers (one bit per entry idx) ----
fn bm_set(bm: &mut Vec<u8>, i: u32) -> Result<(), Error> {
    let byte = usize::try_from(i / 8).unwrap_or(usize::MAX);
    // bitmaps are sized (count+7)/8 at mark start — an idx past the end is a corrupt
    // objects row, not a reason to allocate (idx = u32::MAX would try 512 MiB)
    let b = bm
        .get_mut(byte)
        .ok_or_else(|| Error::Internal("idx past pack count".into()))?;
    *b |= 1 << (i % 8);
    Ok(())
}
fn bm_idxs_from(bm: &[u8], from: u32, limit: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = from;
    while out.len() < limit && usize::try_from(i / 8).map(|b| b < bm.len()).unwrap_or(false) {
        if bm[usize::try_from(i / 8).unwrap_or(0)] & (1 << (i % 8)) != 0 {
            out.push(i);
        }
        i = i.saturating_add(1);
    }
    out
}
fn bm_popcount(bm: &[u8]) -> u32 {
    bm.iter().map(|b| b.count_ones()).sum()
}

/// Group loads so one `read_entries` call charges <= 64 coalesced spans and <= LOAD bytes (A4).
/// Upper bound: <= SPANS entries since a span covers at least one entry.
fn chunks(loads: &[(ObjectId, ObjLoc)]) -> Vec<Vec<(ObjectId, ObjLoc)>> {
    let mut out = Vec::new();
    let (mut cur, mut bytes) = (Vec::new(), 0u64);
    for l in loads {
        if cur.len() >= SPANS || bytes.saturating_add(l.1.size) > LOAD {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes = bytes.saturating_add(l.1.size);
        cur.push(l.clone());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// GcMark (5.1): exact reachability over live objects; one bit per (pack, idx). A frontier row is
/// deleted only inside the span that records its expansion — a kill mid-round replays it.
pub async fn gc_mark(d: &RepoDo, job: &Job, budget: &mut SliceBudget) -> Result<SliceOutcome, Error> {
    let sql = d.sql();
    if d.meta_opt("gc.started_at")?.is_none() {
        // first slice: refuse to overlap a chain already mid-flight
        let busy = exec(
            &sql,
            "SELECT 1 AS n FROM jobs WHERE kind IN ('gc_consolidate','gc_sweep') \
             AND state IN ('queued','running') LIMIT 1",
            vec![],
        )?
        .to_array::<N>()?;
        if !busy.is_empty() {
            return Ok(SliceOutcome::Reschedule { run_at: now_ms() + d.gc_quiet_ms() });
        }
        let now = now_ms();
        for t in ["marked", "gc_frontier", "gc_seen", "gc_parts"] {
            exec(&sql, &format!("DELETE FROM {t}"), vec![])?;
        }
        exec(&sql, "DELETE FROM meta WHERE key LIKE 'gc.%'", vec![])?;
        put(d, "gc.refs_version", d.meta("refs_version")?)?;
        put(d, "gc.started_at", now.to_string())?;
        exec(
            &sql,
            "INSERT INTO marked(pack_id,bitmap) SELECT id, zeroblob((count+7)/8) FROM packs \
             WHERE state='live' AND created_at < ?",
            vec![now.saturating_sub(d.gc_grace_ms()).into()],
        )?;
        exec(
            &sql,
            "INSERT OR IGNORE INTO gc_frontier(sha) SELECT target FROM refs UNION \
             SELECT peeled FROM refs WHERE peeled IS NOT NULL",
            vec![],
        )?;
    }
    let bucket = d.bucket()?;
    let cand: HashSet<String> = exec(&sql, "SELECT pack_id FROM marked", vec![])?
        .to_array::<P>()?
        .into_iter()
        .map(|r| r.pack_id)
        .collect();
    let mut bits: HashMap<String, Vec<u8>> = HashMap::new(); // touched bitmaps, flushed by commit_ids
    loop {
        if budget.spent_80pct() {
            return Ok(SliceOutcome::Continue { cursor: "{}".into() });
        }
        if d.meta_i64("refs_version")? != d.meta_i64("gc.refs_version")? {
            return abort(d, &sql);
        }
        // span A: pop a batch, plan marks and loads; nothing commits until a chunk's span B
        let batch: Vec<String> = exec(&sql, "SELECT sha FROM gc_frontier LIMIT ?", vec![ROUND.into()])?
            .to_array::<S>()?
            .into_iter()
            .map(|r| r.sha)
            .collect();
        if batch.is_empty() {
            exec(&sql, "DELETE FROM gc_seen", vec![])?;
            enqueue(&sql, JobKind::GcConsolidate, now_ms(), "{}")?;
            return Ok(SliceOutcome::Done);
        }
        let rows = exec(
            &sql,
            // GROUP BY keeps one row per sha — N duplicate live packs per sha would
            // otherwise materialize N×batch rows (attacker-seeded OOM, and only the
            // first row per sha is ever used below)
            "SELECT o.sha, o.pack_id, o.idx, o.offset, o.len, o.kind, o.size FROM objects o \
             JOIN packs p ON p.id=o.pack_id WHERE p.state='live' AND \
             o.sha IN (SELECT value FROM json_each(?)) GROUP BY o.sha ORDER BY o.sha",
            vec![js(&batch)?],
        )?
        .to_array::<R>()?;
        let (mut loads, mut prev) = (Vec::<(ObjectId, ObjLoc)>::new(), String::new());
        let mut marked_sha = false;
        for r in &rows {
            if prev != r.sha {
                prev = r.sha.clone();
                marked_sha = false;
                if r.kind != 3 {
                    loads.push(loc(r)?); // blobs never read (5.1)
                }
            }
            if !marked_sha && cand.contains(&r.pack_id) {
                if !bits.contains_key(&r.pack_id) {
                    let b = exec(&sql, "SELECT bitmap FROM marked WHERE pack_id=?", vec![r.pack_id.clone().into()])?
                        .to_array::<BM>()?
                        .into_iter()
                        .next()
                        .map(|x| x.bitmap)
                        .unwrap_or_default();
                    bits.insert(r.pack_id.clone(), b);
                }
                if let Some(bm) = bits.get_mut(&r.pack_id) {
                    bm_set(bm, u32::try_from(r.idx).map_err(|_| Error::Internal("idx".into()))?)?;
                    marked_sha = true;
                }
            }
        }
        let load_ids: HashSet<String> = loads.iter().map(|(id, _)| id.to_string()).collect();
        for chunk in chunks(&loads) {
            let ids: Vec<String> = chunk.iter().map(|(id, _)| id.to_string()).collect();
            let mut kids: Vec<String> = Vec::new();
            for (_id, entry) in bucket.read_entries(&chunk, &mut budget.req).await? {
                let (kind, data) = codec::decode_entry(&entry)?; // A7 cap inside
                for l in extract_links(kind, &data)? {
                    kids.push(l.to_string());
                }
                // a giant tree can yield hundreds of thousands of links in one chunk —
                // flush admission early rather than OOM mid-slice (an OOM kill never
                // increments attempts and would wedge the job forever)
                if kids.len() > 50_000 {
                    commit_ids(&sql, &[], &kids, &mut bits)?;
                    kids.clear();
                }
            }
            commit_ids(&sql, &ids, &kids, &mut bits)?; // span B: expansion commits
            super::heartbeat(&sql, job.id)?; // a progressing slice is not a straggler
            if budget.spent_80pct() {
                break;
            }
        }
        let rest: Vec<String> = batch.iter().filter(|s| !load_ids.contains(*s)).cloned().collect();
        if !rest.is_empty() {
            commit_ids(&sql, &rest, &[], &mut bits)?; // blobs/misses: nothing to read
            super::heartbeat(&sql, job.id)?;
        }
    }
}

/// One sync span: ids are now expanded — seen-recorded, unseen kids admitted, frontier rows
/// deleted, bits flushed.
fn commit_ids(
    sql: &SqlStorage,
    ids: &[String],
    kids: &[String],
    bits: &mut HashMap<String, Vec<u8>>,
) -> Result<(), Error> {
    exec(sql, "INSERT OR IGNORE INTO gc_seen(sha) SELECT value FROM json_each(?)", vec![js(ids)?])?;
    exec(
        sql,
        "INSERT OR IGNORE INTO gc_frontier(sha) SELECT j.value FROM json_each(?) j \
         WHERE NOT EXISTS(SELECT 1 FROM gc_seen s WHERE s.sha=j.value)",
        vec![js(kids)?],
    )?;
    exec(sql, "DELETE FROM gc_frontier WHERE sha IN (SELECT value FROM json_each(?))", vec![js(ids)?])?;
    for (p, mut bm) in bits.drain() {
        // a duplicate concurrent mark slice may have flushed different bits since our
        // last SELECT — OR-merge with the stored bitmap inside this sync span so a
        // last-writer-wins overwrite can never drop a reachable object's mark
        #[derive(serde::Deserialize)]
        struct Cur {
            #[serde(with = "serde_bytes")]
            bitmap: Vec<u8>,
        }
        if let Some(old) = exec(sql, "SELECT bitmap FROM marked WHERE pack_id=?", vec![p.clone().into()])?
            .to_array::<Cur>()?
            .into_iter()
            .next()
        {
            if old.bitmap.len() > bm.len() {
                bm.resize(old.bitmap.len(), 0);
            }
            for (a, b) in bm.iter_mut().zip(old.bitmap.iter()) {
                *a |= *b;
            }
        }
        exec(sql, "UPDATE marked SET bitmap=? WHERE pack_id=?", vec![bm.into(), p.into()])?;
    }
    Ok(())
}

/// The 5.3 abort, shared by every arm: kill the build pack if one exists, drop all gc state,
/// enqueue a fresh mark after the quiet window.
fn abort(d: &RepoDo, sql: &SqlStorage) -> Result<SliceOutcome, Error> {
    if let Some(p) = d.meta_opt("gc.new_pack")? {
        exec(sql, "DELETE FROM objects WHERE pack_id=?", vec![p.clone().into()])?;
        exec(
            sql,
            "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state IN ('ingesting','live')",
            vec![now_ms().into(), p.into()],
        )?;
    }
    for t in ["marked", "gc_frontier", "gc_seen", "gc_parts"] {
        exec(sql, &format!("DELETE FROM {t}"), vec![])?;
    }
    exec(sql, "DELETE FROM meta WHERE key LIKE 'gc.%'", vec![])?;
    enqueue(sql, JobKind::GcMark, now_ms() + d.gc_quiet_ms(), "{}")?;
    Ok(SliceOutcome::Done)
}

enum Flow {
    Yield,
    Done,
    Retry(Error),
    Rebuild,
}

/// GcConsolidate (5.2): one new normalized pack holding exactly the marked entries, verbatim.
pub async fn gc_consolidate(d: &RepoDo, job: &Job, budget: &mut SliceBudget) -> Result<SliceOutcome, Error> {
    let sql = d.sql();
    let bucket = d.bucket()?;
    loop {
        let Some(mut pos) = (match d.meta_opt("gc.pos")? {
            Some(j) => serde_json::from_str::<Pos>(&j).ok(),
            None => begin_build(d, &sql)?,
        }) else {
            return Ok(SliceOutcome::Done); // nothing to repack: state already wiped
        };
        let key = keys::pack(&bucket.repo, &PackId(pos.pack.clone()));
        let etags: Vec<String> = exec(&sql, "SELECT etag FROM gc_parts ORDER BY part_no", vec![])?
            .to_array::<E>()?
            .into_iter()
            .map(|r| r.etag)
            .collect();
        let out = if pos.upload.is_empty() {
            let w = PackWriter::create(&bucket, &key, pos.total, &mut budget.req).await?;
            pos.upload = w.upload_id().await; // upload id commits with the first part;
            w // a kill before then only orphans the MPU (~7d)
        } else {
            PackWriter::resume(
                &bucket,
                &key,
                &pos.upload,
                &etags,
                pos.st.as_ref().ok_or_else(|| Error::Internal("gc.pos without WriterCkpt".into()))?,
                &mut budget.req,
            )
            .await?
        };
        match build(d, &sql, &bucket, &key, out, &mut pos, job.id, budget).await? {
            Flow::Yield => return Ok(SliceOutcome::Continue { cursor: "{}".into() }),
            Flow::Done => return Ok(SliceOutcome::Done),
            Flow::Retry(e) => return Err(e), // 4.4 backoff; second failure -> Rebuild
            Flow::Rebuild => wipe_build(d, &sql)?, // dead upload: marks intact, restart
        }
    }
}

/// Fill >= 5 MiB parts with the marked entries in (pack_id, idx) order — deterministic, so a
/// killed slice replays the same bytes to the same part numbers and insert_objects' ON CONFLICT
/// DO NOTHING absorbs replayed rows.
async fn build(
    d: &RepoDo,
    sql: &SqlStorage,
    bucket: &Bucket,
    key: &str,
    mut out: PackWriter,
    pos: &mut Pos,
    job_id: i64,
    budget: &mut SliceBudget,
) -> Result<Flow, Error> {
    let cands: Vec<String> = exec(sql, "SELECT pack_id FROM marked ORDER BY pack_id", vec![])?
        .to_array::<P>()?
        .into_iter()
        .map(|r| r.pack_id)
        .collect();
    let (mut staged, mut bm): (Vec<ObjRow>, (u32, Vec<u8>)) = (vec![], (u32::MAX, vec![]));
    // parts drain_parts uploaded mid-entry but no checkpoint has recorded yet —
    // flushed into gc_parts by the next flush(), in the same span as the WriterCkpt
    let mut unrecorded: Vec<(u16, String)> = Vec::new();
    let mut ordinal: u32 = pos.st.as_ref().map(|s| s.count).unwrap_or(0);
    loop {
        if budget.spent_80pct() {
            return Ok(Flow::Yield);
        }
        let mut idxs: Vec<u32> = Vec::new();
        while idxs.is_empty() {
            match cands.get(pos.ci as usize) {
                // input exhausted: finish owns tail+trailer as the last part (any size allowed)
                None => {
                    Index(sql).insert_objects(&PackId(pos.pack.clone()), &staged)?;
                    staged.clear();
                    return match out.finish(&mut budget.req).await {
                        Ok(meta) => finish_build(d, sql, pos, &meta),
                        Err(e) => mpu_err(d, sql, bucket, key, pos, e, budget).await,
                    };
                }
                Some(cid) => {
                    if bm.0 != pos.ci {
                        bm.1 = exec(
                            sql,
                            "SELECT bitmap FROM marked WHERE pack_id=?",
                            vec![cid.clone().into()],
                        )?
                        .to_array::<BM>()?
                        .into_iter()
                        .next()
                        .map(|x| x.bitmap)
                        .unwrap_or_default();
                        bm.0 = pos.ci;
                    }
                    idxs = bm_idxs_from(&bm.1, pos.idx, BATCH_N);
                    match idxs.last() {
                        Some(l) => pos.idx = l.saturating_add(1),
                        None => {
                            pos.ci = pos.ci.saturating_add(1);
                            pos.idx = 0;
                        }
                    }
                }
            }
        }
        let rows = exec(
            sql,
            // ORDER BY idx makes the build fully deterministic: a replay after a kill
            // must reproduce identical append offsets, or ON CONFLICT keeps stale rows
            "SELECT sha, pack_id, idx, offset, len, kind, size FROM objects WHERE pack_id=? \
             AND idx IN (SELECT value FROM json_each(?)) ORDER BY idx",
            vec![cands.get(pos.ci as usize).cloned().unwrap_or_default().into(), js(&idxs)?],
        )?
        .to_array::<R>()?;
        if rows.len() != idxs.len() {
            return Err(Error::Internal("marked entry lacks objects row".into()));
        }
        let locs: Vec<(ObjectId, ObjLoc)> = rows.iter().map(loc).collect::<Result<_, _>>()?;
        for chunk in chunks(&locs) {
            if budget.spent_80pct() {
                return Ok(Flow::Yield); // the inner batch can run ~90 reads — still bounded
            }
            // An entry too big for one buffered read can't ride read_entries (it
            // materializes whole entries and refuses > 48 MiB). Small entries coalesce
            // as before; big ones stream SPAN-sized fragments straight into the writer
            // via raw_extend, draining full parts as they form. Order stays the chunk's
            // idx order, so replay is byte-identical.
            let mut got: HashMap<ObjectId, VecDeque<Vec<u8>>> = HashMap::new();
            let small: Vec<(ObjectId, ObjLoc)> = chunk
                .iter()
                .filter(|(_, l)| u64::from(l.len) <= SPAN)
                .cloned()
                .collect();
            for (id, entry) in bucket.read_entries(&small, &mut budget.req).await? {
                got.entry(id).or_default().push_back(entry);
            }
            for (id, l) in &chunk {
                let (off, len) = if u64::from(l.len) > SPAN {
                    let start = out.offset();
                    let key = keys::pack(&bucket.repo, &l.pack);
                    let end = l.offset.saturating_add(u64::from(l.len));
                    let mut p = l.offset;
                    while p < end {
                        if budget.spent_80pct() {
                            // mid-entry abandon is safe: nothing commits until the next
                            // checkpoint, and replay restarts this entry deterministically
                            return Ok(Flow::Yield);
                        }
                        let frag = bucket
                            .read_range(&key, p, end.saturating_sub(p).min(SPAN), &mut budget.req)
                            .await?;
                        p = p.saturating_add(u64::try_from(frag.len()).map_err(|_| Error::Internal("frag".into()))?);
                        out.raw_extend(&frag);
                        unrecorded.extend(out.drain_parts(MIN_PART as usize, &mut budget.req).await?);
                    }
                    out.raw_entry_done(l.kind, start)?
                } else {
                    let entry = got
                        .get_mut(id)
                        .and_then(|q| q.pop_front())
                        .ok_or_else(|| Error::Storage("entry missing from read".into()))?;
                    out.append_stored(&entry)? // verbatim; sha+count inside
                };
                staged.push(ObjRow {
                    sha: *id,
                    idx: ordinal,
                    offset: off,
                    len,
                    kind: l.kind,
                    size: l.size,
                });
                ordinal = ordinal.saturating_add(1);
                if l.kind == Kind::Commit {
                    pos.lo = pos.lo.min(i64::try_from(off).unwrap_or((1 << 53) - 1));
                    pos.hi = pos
                        .hi
                        .max(i64::try_from(off.saturating_add(u64::from(len))).unwrap_or((1 << 53) - 1));
                }
            }
            // flush per chunk, not per batch: `out.part` would otherwise accumulate the
            // whole 90-entry batch (~1.4 GiB of buffered pack bytes) and OOM the isolate
            if let Some(f) =
                flush(d, sql, bucket, key, &mut out, pos, &mut staged, &mut unrecorded, job_id, budget).await?
            {
                return Ok(f);
            }
        }
        if staged.len() >= ROWS {
            Index(sql).insert_objects(&PackId(pos.pack.clone()), &staged)?;
            staged.clear();
        }
        // a multi-MiB chunk loop can run past the 60 s straggler window on slow R2 —
        // heartbeat so repair doesn't requeue a live consolidate into a duplicate slice
        super::heartbeat(sql, job_id)?;
        if let Some(f) = flush(d, sql, bucket, key, &mut out, pos, &mut staged, &mut unrecorded, job_id, budget).await? {
            return Ok(f);
        }
    }
}

/// Upload the buffered part once it reaches MIN_PART, then commit — one span — the rows, the
/// etags and the position. `unrecorded` holds parts drain_parts already uploaded mid-entry;
/// they must join gc_parts in this same span, alongside the WriterCkpt that accounts for
/// their bytes — recorded earlier, resume would count parts the checkpoint state doesn't.
/// The sub-5 MiB tail is never checkpointed: finish folds it into the final part.
async fn flush(
    d: &RepoDo,
    sql: &SqlStorage,
    bucket: &Bucket,
    key: &str,
    out: &mut PackWriter,
    pos: &mut Pos,
    staged: &mut Vec<ObjRow>,
    unrecorded: &mut Vec<(u16, String)>,
    job_id: i64,
    budget: &mut SliceBudget,
) -> Result<Option<Flow>, Error> {
    if out.buffered() < MIN_PART {
        return Ok(None);
    }
    let Some((part_no, etag, st)) = (match out.checkpoint(&mut budget.req).await {
        Ok(x) => x,
        Err(e) => return mpu_err(d, sql, bucket, key, pos, e, budget).await.map(Some),
    }) else {
        return Ok(None);
    };
    Index(sql).insert_objects(&PackId(pos.pack.clone()), staged)?; // OR IGNORE: replay-safe
    staged.clear();
    for (no, tag) in unrecorded.drain(..).chain(std::iter::once((part_no, etag))) {
        exec(
            sql,
            "INSERT OR REPLACE INTO gc_parts(part_no,etag) VALUES(?,?)",
            vec![i64::from(no).into(), tag.into()],
        )?;
    }
    pos.st = Some(st);
    put(d, "gc.pos", serde_json::to_string(pos).map_err(|e| Error::Internal(e.to_string()))?)?;
    super::heartbeat(sql, job_id)?; // checkpoint span: refresh the repair lease
    Ok(None)
}

/// complete() may have succeeded before an error/kill: head() on the key is the oracle. A truly
/// dead upload (expired MPU, aborted) gives no key: retry once through 4.4, then rebuild from
/// the intact marks.
async fn mpu_err(
    d: &RepoDo,
    sql: &SqlStorage,
    bucket: &Bucket,
    key: &str,
    pos: &Pos,
    e: Error,
    budget: &mut SliceBudget,
) -> Result<Flow, Error> {
    budget.req.charge(1)?;
    if let Some(h) = bucket.inner.head(key).await? {
        // complete() had already won
        let meta = PackMeta {
            pack: PackId(pos.pack.clone()),
            push_id: None,
            count: pos.total,
            bytes: h.size(),
            commit_lo: u64::try_from(pos.lo).unwrap_or(u64::MAX),
            commit_hi: u64::try_from(pos.hi).unwrap_or(0),
            created_at: now_ms(),
        };
        return finish_build(d, sql, pos, &meta);
    }
    let f = d.meta_i64("gc.fails").unwrap_or(0).saturating_add(1);
    put(d, "gc.fails", f.to_string())?;
    Ok(if f >= 2 { Flow::Rebuild } else { Flow::Retry(e) })
}

/// First build slice (5.2): repack only if >= 2 candidates or any candidate has unmarked
/// entries — otherwise wipe gc state and Done, because sweeping without a new pack would
/// delete reachable objects.
fn begin_build(d: &RepoDo, sql: &SqlStorage) -> Result<Option<Pos>, Error> {
    if d.meta_i64("refs_version")? != d.meta_i64("gc.refs_version")? {
        abort(d, sql)?;
        return Ok(None);
    }
    let cands = exec(
        sql,
        "SELECT m.pack_id, m.bitmap, p.count FROM marked m JOIN packs p ON p.id=m.pack_id \
         ORDER BY m.pack_id",
        vec![],
    )?
    .to_array::<B>()?;
    let (mut total, mut unmarked) = (0u32, false);
    for c in &cands {
        let n = bm_popcount(&c.bitmap);
        total = total.saturating_add(n);
        unmarked |= i64::from(n) < c.count;
    }
    if total == 0 && !cands.is_empty() {
        // nothing reachable in any candidate: no repack to write — sweep deletes them outright
        exec(sql, "DELETE FROM gc_parts", vec![])?;
        exec(sql, "DELETE FROM meta WHERE key IN ('gc.pos','gc.new_pack','gc.fails')", vec![])?;
        enqueue(sql, JobKind::GcSweep, now_ms(), "{}")?;
        return Ok(None);
    }
    if !(cands.len() >= 2 || unmarked) {
        if let Some(p) = d.meta_opt("gc.new_pack")? {
            // half-written leftover, no pos
            exec(sql, "DELETE FROM objects WHERE pack_id=?", vec![p.clone().into()])?;
            exec(
                sql,
                "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state='ingesting'",
                vec![now_ms().into(), p.into()],
            )?;
        }
        for t in ["marked", "gc_frontier", "gc_seen", "gc_parts"] {
            exec(sql, &format!("DELETE FROM {t}"), vec![])?;
        }
        exec(sql, "DELETE FROM meta WHERE key LIKE 'gc.%'", vec![])?;
        return Ok(None);
    }
    // a retried begin re-enters here with gc.new_pack still set — the prior build pack
    // has push_id IS NULL, which the janitor deliberately never touches, so dead-mark
    // it now or the row (and eventually its R2 key) is orphaned forever
    if let Some(p) = d.meta_opt("gc.new_pack")? {
        exec(sql, "DELETE FROM objects WHERE pack_id=?", vec![p.clone().into()])?;
        exec(
            sql,
            "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state='ingesting'",
            vec![now_ms().into(), p.into()],
        )?;
    }
    let pack = PackId::random()?;
    exec(
        sql,
        "INSERT INTO packs(id,state,count,bytes,commit_lo,commit_hi,push_id,created_at) \
         VALUES(?,'ingesting',0,0,?,0,NULL,?)",
        vec![pack.0.as_str().into(), ((1i64 << 53) - 1).into(), now_ms().into()],
    )?;
    let pos = Pos { pack: pack.0.clone(), total, lo: (1 << 53) - 1, ..Pos::default() };
    put(d, "gc.new_pack", pack.0.clone())?;
    put(d, "gc.pos", serde_json::to_string(&pos).map_err(|e| Error::Internal(e.to_string()))?)?;
    Ok(Some(pos))
}

fn wipe_build(d: &RepoDo, sql: &SqlStorage) -> Result<(), Error> {
    if let Some(p) = d.meta_opt("gc.new_pack")? {
        exec(sql, "DELETE FROM objects WHERE pack_id=?", vec![p.clone().into()])?;
        exec(
            sql,
            "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state='ingesting'",
            vec![now_ms().into(), p.into()],
        )?;
    }
    exec(sql, "DELETE FROM gc_parts", vec![])?;
    exec(sql, "DELETE FROM meta WHERE key IN ('gc.pos','gc.new_pack','gc.fails')", vec![])?;
    Ok(())
}

fn finish_build(d: &RepoDo, sql: &SqlStorage, pos: &Pos, meta: &PackMeta) -> Result<Flow, Error> {
    exec(
        sql,
        "UPDATE packs SET state='live', count=?, bytes=?, commit_lo=?, commit_hi=? \
         WHERE id=? AND state IN ('ingesting','live')",
        vec![
            i64::from(meta.count).into(),
            i64::try_from(meta.bytes).map_err(|_| Error::Internal("bytes".into()))?.into(),
            i64::try_from(meta.commit_lo).unwrap_or((1 << 53) - 1).min((1 << 53) - 1).into(),
            i64::try_from(meta.commit_hi).unwrap_or(0).into(),
            pos.pack.as_str().into(),
        ],
    )?;
    if d.changes()? != 1 {
        return Err(Error::Internal("build pack row".into()));
    }
    enqueue(sql, JobKind::GcSweep, now_ms(), "{}")?; // the only deleter (5.3)
    Ok(Flow::Done) // gc.* cleared by the sweep span
}

/// GcSweep (5.3): the whole step is one sync span — the refs_version check, the row deletes and
/// the gc_epoch bump are atomic — and it never touches R2: dead keys are the Janitor's, after GRACE.
pub async fn gc_sweep(d: &RepoDo) -> Result<SliceOutcome, Error> {
    let sql = d.sql();
    if d.meta_opt("gc.refs_version")?.is_none() {
        return Ok(SliceOutcome::Done);
    }
    if d.meta_i64("refs_version")? != d.meta_i64("gc.refs_version")? {
        return abort(d, &sql);
    }
    exec(
        &sql,
        "UPDATE packs SET state='dead', dead_at=? WHERE id IN (SELECT pack_id FROM marked)",
        vec![now_ms().into()],
    )?;
    exec(&sql, "DELETE FROM objects WHERE pack_id IN (SELECT pack_id FROM marked)", vec![])?;
    exec(&sql, "UPDATE meta SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT) WHERE key='gc_epoch'", vec![])?;
    for t in ["marked", "gc_frontier", "gc_seen", "gc_parts"] {
        exec(&sql, &format!("DELETE FROM {t}"), vec![])?;
    }
    exec(&sql, "DELETE FROM meta WHERE key LIKE 'gc.%'", vec![])?;
    Ok(SliceOutcome::Done)
}
