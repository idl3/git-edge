//! Janitor (5.1): 15-minute cadence, contract steps 1–3. Each arm is bounded; a kill mid-slice
//! re-issues the same work next run — marking and R2 deleting never happen in one slice
//! (deletion only targets rows marked dead/not-open at least GRACE earlier).
//! NB: LIMIT on UPDATE/DELETE requires SQLITE_ENABLE_UPDATE_DELETE_LIMIT, which workerd's build
//! does not guarantee — all bounds are applied on the SELECT, mutations key by row id.

use worker::SqlStorageValue as V;

use super::{Job, SliceBudget, SliceOutcome};
use crate::error::Error;
use crate::platform;
use crate::repo_do::RepoDo;
use crate::store::{keys, PackId, PushId};

const BATCH: i64 = 400; // keys per slice (5.1: at most 400)
const GRACE_MS: i64 = 3_600_000;
const PUSH_TIMEOUT_MS: i64 = 3_600_000;
const REFLOG_KEEP_MS: i64 = 90 * 24 * 3_600_000;

#[derive(serde::Deserialize)]
struct I {
    id: String,
}
#[derive(serde::Deserialize)]
struct N {
    id: i64,
}

pub async fn run_slice(d: &RepoDo, _job: &Job, budget: &mut SliceBudget) -> Result<SliceOutcome, Error> {
    let now = platform::now_ms();

    // 1. expire open pushes past PUSH_TIMEOUT
    let stale: Vec<I> = d
        .q(
            "SELECT id FROM pushes WHERE state='open' AND began_at < ? LIMIT ?",
            vec![V::from(now.saturating_sub(PUSH_TIMEOUT_MS)), V::from(BATCH)],
        )?
        .to_array::<I>()?
        .into_iter()
        .collect();
    for p in &stale {
        d.q(
            "UPDATE pushes SET state='expired', ended_at=? WHERE id=? AND state='open'",
            vec![V::from(now), V::from(p.id.as_str())],
        )?;
    }

    // 2. ingesting packs whose push is expired/rejected -> dead + drop object rows (one span each)
    let dead_packs: Vec<I> = d
        .q(
            "SELECT p.id FROM packs p JOIN pushes u ON u.pack_id = p.id \
             WHERE p.state='ingesting' AND u.state IN ('expired','rejected') LIMIT ?",
            vec![V::from(BATCH)],
        )?
        .to_array::<I>()?
        .into_iter()
        .collect();
    for p in &dead_packs {
        d.q("DELETE FROM objects WHERE pack_id=?", vec![V::from(p.id.as_str())])?;
        d.q(
            "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state='ingesting'",
            vec![V::from(now), V::from(p.id.as_str())],
        )?;
    }
    // a pack 'ingesting' with no open push pointing at it is abandoned (crash between pack finish
    // and commit): dead after PUSH_TIMEOUT so step 3 reclaims it.
    let orphaned: Vec<I> = d
        .q(
            "SELECT id FROM packs WHERE state='ingesting' AND created_at < ? \
             AND NOT EXISTS(SELECT 1 FROM pushes u WHERE u.pack_id = packs.id AND u.state='open') LIMIT ?",
            vec![V::from(now.saturating_sub(PUSH_TIMEOUT_MS)), V::from(BATCH)],
        )?
        .to_array::<I>()?
        .into_iter()
        .collect();
    for p in &orphaned {
        d.q("DELETE FROM objects WHERE pack_id=?", vec![V::from(p.id.as_str())])?;
        d.q(
            "UPDATE packs SET state='dead', dead_at=? WHERE id=? AND state='ingesting'",
            vec![V::from(now), V::from(p.id.as_str())],
        )?;
    }

    // 3. R2 deletes: pending packs of not-open pushes older than GRACE (swept_at marks done);
    //    dead packs past GRACE whose object rows are gone.
    let pend: Vec<I> = d
        .q(
            "SELECT id FROM pushes WHERE state <> 'open' AND began_at < ? AND swept_at IS NULL LIMIT ?",
            vec![V::from(now.saturating_sub(GRACE_MS)), V::from(BATCH)],
        )?
        .to_array::<I>()?
        .into_iter()
        .collect();
    for p in &pend {
        if budget.spent_80pct() {
            return Ok(SliceOutcome::Continue { cursor: "{}".into() });
        }
        let key = keys::pending(&d.repo_id()?, &PushId(p.id.clone()));
        budget.req.charge(1)?;
        let _ = d.bucket()?.inner.delete(key.as_str()).await; // missing key is fine
        d.q("UPDATE pushes SET swept_at=? WHERE id=?", vec![V::from(now), V::from(p.id.as_str())])?;
    }
    let dead: Vec<I> = d
        .q(
            "SELECT id FROM packs WHERE state='dead' AND dead_at < ? \
             AND NOT EXISTS(SELECT 1 FROM objects o WHERE o.pack_id = packs.id) LIMIT ?",
            vec![V::from(now.saturating_sub(GRACE_MS)), V::from(BATCH)],
        )?
        .to_array::<I>()?
        .into_iter()
        .collect();
    for p in &dead {
        if budget.spent_80pct() {
            return Ok(SliceOutcome::Continue { cursor: "{}".into() });
        }
        let key = keys::pack(&d.repo_id()?, &PackId(p.id.clone()));
        budget.req.charge(1)?;
        let _ = d.bucket()?.inner.delete(key.as_str()).await;
        d.q("DELETE FROM packs WHERE id=?", vec![V::from(p.id.as_str())])?;
    }

    // 4. reflog expiry: 90 days, keyed by row id.
    let old_log: Vec<N> = d
        .q(
            "SELECT id FROM reflog WHERE at < ? LIMIT ?",
            vec![V::from(now.saturating_sub(REFLOG_KEEP_MS)), V::from(BATCH)],
        )?
        .to_array::<N>()?
        .into_iter()
        .collect();
    for r in &old_log {
        d.q("DELETE FROM reflog WHERE id=?", vec![V::from(r.id)])?;
    }

    Ok(SliceOutcome::Done)
}
