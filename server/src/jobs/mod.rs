//! Job dispatcher (CONTRACTS.md 4, 5; A3, A4) + running-row repair (A12).
//! `rearm` is the sole `set_alarm` caller; `enqueue` is sync and only writes a row.

pub mod gc;
pub mod import;
pub mod janitor;
pub mod purge;

use worker::{SqlStorage, SqlStorageValue as V};

use crate::error::Error;
use crate::platform;
use crate::repo_do::RepoDo;
use crate::ReqBudget;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Janitor,
    GcMark,
    GcConsolidate,
    GcSweep,
    PurgeRepo,
    ImportPack,
}
impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Janitor => "janitor",
            JobKind::GcMark => "gc_mark",
            JobKind::GcConsolidate => "gc_consolidate",
            JobKind::GcSweep => "gc_sweep",
            JobKind::PurgeRepo => "purge_repo",
            JobKind::ImportPack => "import_pack",
        }
    }
    fn of(s: &str) -> Result<Self, Error> {
        match s {
            "janitor" => Ok(JobKind::Janitor),
            "gc_mark" => Ok(JobKind::GcMark),
            "gc_consolidate" => Ok(JobKind::GcConsolidate),
            "gc_sweep" => Ok(JobKind::GcSweep),
            "purge_repo" => Ok(JobKind::PurgeRepo),
            "import_pack" => Ok(JobKind::ImportPack),
            k => Err(Error::Internal(format!("unknown job kind {k}"))),
        }
    }
}

pub struct Job {
    pub id: i64,
    pub kind: JobKind,
    pub run_at: i64,
    pub attempts: u32,
    pub cursor: Option<String>,
    pub payload: String,
    /// The fencing token `dispatch` wrote at claim time. Heartbeats CAS on it
    /// (A19): a slice whose row was requeued as stranded and reclaimed no longer
    /// matches, and must stop writing before the fenced outcome span runs.
    pub lease: String,
}

pub enum SliceOutcome {
    Done,
    Continue { cursor: String },
    Reschedule { run_at: i64 },
}

/// Slice budget (4.3): 20,000 ms wall clock and 400 subrequests; Continue at 80%.
pub struct SliceBudget {
    pub started_ms: f64,
    pub req: ReqBudget,
}
impl SliceBudget {
    pub fn fresh() -> Self {
        Self {
            started_ms: js_sys::Date::now(),
            req: ReqBudget { max_subrequests: 400, used: 0, started_ms: js_sys::Date::now(), max_ms: 20_000.0, sink: None },
        }
    }
    pub fn spent_80pct(&self) -> bool {
        let elapsed = js_sys::Date::now() - self.started_ms;
        elapsed > 16_000.0 || self.req.used > 320
    }
}

/// A checkpoint span refreshes the lease: `repair` may only requeue a row whose started_at
/// has gone stale — a progressing slice heartbeats and is never mistaken for a stranded one.
///
/// A19: the heartbeat is a CAS on the fencing token, not a bare `id` match. A slice
/// whose row was repair-requeued (`state='queued'`) or reclaimed under a fresh lease
/// (`lease` changed) gets `false` — it must stop immediately via `stale_lease()`,
/// because every write it still had queued belongs to the new lease-holder, and an
/// unconditional `started_at` bump would mask a genuinely stalled new owner from
/// `repair`.
pub fn heartbeat(sql: &SqlStorage, job: &Job) -> Result<bool, Error> {
    sql.exec(
        "UPDATE jobs SET started_at=? WHERE id=? AND state='running' AND lease=?",
        Some(vec![
            V::from(platform::now_ms()),
            V::from(job.id),
            V::from(job.lease.as_str()),
        ]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    #[derive(serde::Deserialize)]
    struct N {
        n: i64,
    }
    sql.exec("SELECT changes() AS n", Some(vec![]))
        .map_err(|e| Error::Storage(e.to_string()))?
        .one::<N>()
        .map(|r| r.n == 1)
        .map_err(|e| Error::Storage(e.to_string()))
}

/// Returned by a slice whose heartbeat CAS failed: the row belongs to another
/// lease now. The fenced outcome span no-ops on it, `attempts` is not consumed,
/// and dispatch reports the event as `stale`, never a retry.
pub fn stale_lease() -> Error {
    Error::Internal("stale job lease".into())
}

/// Sync; writes the row only (A3). Dedups against 'queued' rows: at most one queued row per kind.
/// Deduping against 'running' too would suppress a job's own re-enqueue (edge review), so a running
/// row does not block a new enqueue — the next dispatch simply runs the kind again, which is
/// idempotent by construction.
pub fn enqueue(sql: &SqlStorage, kind: JobKind, run_at: i64, payload: &str) -> Result<(), Error> {
    sql.exec(
        "INSERT INTO jobs(kind,run_at,payload) SELECT ?,?,? \
         WHERE NOT EXISTS(SELECT 1 FROM jobs WHERE kind=? AND state='queued')",
        Some(vec![V::from(kind.as_str()), V::from(run_at), V::from(payload), V::from(kind.as_str())]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}

/// The only `set_alarm` in the crate (4.1). Alarm = MIN(run_at) over queued rows, or deleted.
pub async fn rearm(d: &RepoDo) -> Result<(), Error> {
    #[derive(serde::Deserialize)]
    struct R {
        run_at: i64,
    }
    let next = d
        .q("SELECT run_at FROM jobs WHERE state='queued' ORDER BY run_at LIMIT 1", vec![])?
        .to_array::<R>()?
        .into_iter()
        .next();
    match next {
        Some(r) => {
            // set_alarm interprets i64/Duration as an *offset from now*; jobs.run_at is an
            // absolute epoch-ms timestamp, so it must go through a Date (ScheduledTime::new).
            let when = js_sys::Date::new(&js_sys::Number::from(r.run_at as f64));
            d.state.storage().set_alarm(worker::ScheduledTime::new(when)).await?
        }
        None => d.state.storage().delete_alarm().await?,
    }
    Ok(())
}

/// A4 + A12, called from boot: re-enqueue dead maintenance jobs, and requeue 'running' rows a
/// killed isolate stranded (a slice cannot legally exceed ~20 s; 60 s is generous).
pub fn repair(sql: &SqlStorage) -> Result<(), Error> {
    let now = platform::now_ms();
    // A stranded 'running' row is a crashed isolate, not a job error — it counts
    // against `strands`, not `attempts`, so a legitimately long multi-slice job
    // survives rebuild/restart churn. A crash-looping job still dies: strands
    // only reset when a slice completes, so 64 consecutive dead isolates means
    // the slice itself is what kills them.
    sql.exec(
        "UPDATE jobs SET state='dead', last_error='stranded: isolate died 64x without a completed slice' \
         WHERE state='running' AND started_at < ? AND strands >= 64",
        Some(vec![V::from(now.saturating_sub(60_000))]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    sql.exec(
        "UPDATE jobs SET state='queued', run_at=?, strands=strands+1, last_error='stranded mid-slice', \
         lease=NULL WHERE state='running' AND started_at < ? AND strands < 64",
        Some(vec![V::from(now), V::from(now.saturating_sub(60_000))]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    // a dead or missing Janitor is re-enqueued at every boot (A4)
    #[derive(serde::Deserialize)]
    struct N {
        n: i64,
    }
    let janitors = sql
        .exec(
            "SELECT COUNT(*) AS n FROM jobs WHERE kind='janitor' AND state IN ('queued','running')",
            Some(vec![]),
        )
        .map_err(|e| Error::Storage(e.to_string()))?
        .one::<N>()
        .map_err(|e| Error::Storage(e.to_string()))?;
    if janitors.n == 0 {
        enqueue(sql, JobKind::Janitor, now, "{}")?;
    }
    for kind in [JobKind::GcMark, JobKind::GcConsolidate, JobKind::GcSweep] {
        // dead maintenance jobs of a chain that may have been mid-flight resume via re-enqueue (A4)
        let n = sql
            .exec(
                "SELECT COUNT(*) AS n FROM jobs WHERE kind=? AND state='dead'",
                Some(vec![V::from(kind.as_str())]),
            )
            .map_err(|e| Error::Storage(e.to_string()))?
            .one::<N>()
            .map_err(|e| Error::Storage(e.to_string()))?;
        if n.n > 0 {
            // keep the last failure inspectable — the rows are deleted and a fresh job
            // has no error context otherwise
            #[derive(serde::Deserialize)]
            struct Dead {
                last_error: Option<String>,
            }
            let err = sql
                .exec(
                    "SELECT last_error FROM jobs WHERE kind=? AND state='dead' AND last_error IS NOT NULL \
                     ORDER BY id DESC LIMIT 1",
                    Some(vec![V::from(kind.as_str())]),
                )
                .map_err(|e| Error::Storage(e.to_string()))?
                .to_array::<Dead>()
                .map_err(|e| Error::Storage(e.to_string()))?
                .into_iter()
                .next()
                .and_then(|d| d.last_error);
            if let Some(e) = err {
                sql.exec(
                    "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    Some(vec![V::from(format!("dead.{}", kind.as_str())), V::from(e)]),
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            }
            // bound resurrection: a deterministically failing chain (corrupt pack,
            // wedge) would otherwise restart at every boot forever — a ~400-subrequest
            // slice of doomed work each time. resurrected.<kind> counts restarts and
            // resets when the chain completes (gc_sweep Done clears it).
            let key = format!("resurrected.{}", kind.as_str());
            #[derive(serde::Deserialize)]
            struct M {
                value: String,
            }
            let tries = sql
                .exec("SELECT value FROM meta WHERE key=?", Some(vec![V::from(key.as_str())]))
                .map_err(|e| Error::Storage(e.to_string()))?
                .to_array::<M>()
                .map_err(|e| Error::Storage(e.to_string()))?
                .into_iter()
                .next()
                .and_then(|m| m.value.parse::<i64>().ok())
                .unwrap_or(0);
            if tries >= 3 {
                continue; // leave the dead rows and dead.<kind> for inspection
            }
            sql.exec("DELETE FROM jobs WHERE kind=? AND state='dead'", Some(vec![V::from(kind.as_str())]))
                .map_err(|e| Error::Storage(e.to_string()))?;
            enqueue(sql, kind, now, "{}")?;
            sql.exec(
                "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                Some(vec![V::from(key.as_str()), V::from((tries + 1).to_string())]),
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        }
    }
    Ok(())
}

/// alarm() -> dispatch (4.2): one slice per firing. Never lets an error escape (4.4).
pub async fn dispatch(d: &RepoDo) -> Result<(), Error> {
    let r = dispatch_inner(d).await;
    if let Err(e) = &r {
        worker::console_log!("jobs::dispatch: {e}");
    }
    // A18: `jobs_dead > 0` is the wedge signal — one gauge datapoint per alarm
    // pass while any row is dead, so an external alarm does not need _state polls.
    dead_gauge(d);
    let _ = rearm(d).await;
    r
}

/// A18 dead-job gauge: high-cardinality-safe (repo index, fixed blobs, one double).
/// Query errors — e.g. a `purge_repo` slice dropped the schema — are ignored.
fn dead_gauge(d: &RepoDo) {
    #[derive(serde::Deserialize)]
    struct N {
        n: i64,
    }
    let dead = d
        .q("SELECT COUNT(*) AS n FROM jobs WHERE state='dead'", vec![])
        .ok()
        .and_then(|c| c.one::<N>().ok())
        .map(|r| r.n)
        .unwrap_or(0);
    if dead > 0 {
        let repo = platform::do_id_name(&d.state).unwrap_or_else(|| "-".into());
        platform::jobs_dead_gauge(&d.env, &repo, dead);
    }
}

async fn dispatch_inner(d: &RepoDo) -> Result<(), Error> {
    #[derive(serde::Deserialize)]
    struct JobRow {
        id: i64,
        kind: String,
        run_at: i64,
        attempts: i64,
        cursor: Option<String>,
        payload: String,
    }
    let now = platform::now_ms();
    let row = d
        .q(
            "SELECT id,kind,run_at,attempts,cursor,payload FROM jobs \
             WHERE state='queued' AND run_at <= ? ORDER BY run_at, id LIMIT 1",
            vec![V::from(now)],
        )?
        .to_array::<JobRow>()?
        .into_iter()
        .next();
    let Some(r) = row else { return Ok(()) };
    // mark running + record started_at (A12) + a lease token in one sync span. The
    // lease survives heartbeats (which rewrite started_at) and fences the outcome
    // span: a slice whose row was repair-requeued and reclaimed by another dispatch
    // can no longer mutate the row when it finally finishes.
    let lease = platform::hex16()?;
    d.q(
        "UPDATE jobs SET state='running', started_at=?, lease=? WHERE id=? AND state='queued'",
        vec![V::from(now), V::from(lease.as_str()), V::from(r.id)],
    )?;
    if d.changes()? != 1 {
        return Ok(()); // someone else took it; impossible inside a span, but harmless
    }
    let job = Job {
        id: r.id,
        kind: JobKind::of(&r.kind)?,
        run_at: r.run_at,
        attempts: u32::try_from(r.attempts).unwrap_or(0),
        cursor: r.cursor.clone(),
        payload: r.payload.clone(),
        lease: lease.clone(),
    };
    // A17 metrics: the repo label comes from ctx.id.name, not meta — a purge_repo
    // slice wipes meta mid-flight and the label must survive it.
    let repo = platform::do_id_name(&d.state).unwrap_or_else(|| "-".into());
    let attempt = job.attempts.saturating_add(1);
    platform::job_event(&d.env, &repo, job.kind.as_str(), "start", "ok", attempt, 0, false, "");
    let mut budget = SliceBudget::fresh();
    let out = run_slice(d, &job, &mut budget).await;
    let dur_ms = (js_sys::Date::now() - budget.started_ms).max(0.0) as i64;
    worker::console_log!("job {}#{} attempts={} -> {}", job.kind.as_str(), job.id, job.attempts,
        match &out { Ok(SliceOutcome::Done) => "done".into(),
            Ok(SliceOutcome::Continue { .. }) => "continue".into(),
            Ok(SliceOutcome::Reschedule { .. }) => "reschedule".into(),
            Err(e) => format!("err {e}") });
    // apply the outcome in a sync span (4.2, 4.4) — fenced on our lease: if `repair`
    // requeued this row mid-slice and another dispatch claimed it (started_at changed),
    // this slice is stale and must not mutate the row or run Done-side-effects — a
    // concurrent gc_mark sibling would otherwise lose its bitmap updates.
    let fenced = |q: &str, mut args: Vec<V>| -> Result<bool, Error> {
        args.push(V::from(r.id));
        args.push(V::from(lease.as_str()));
        match d.q(q, args) {
            Ok(_) => Ok(d.changes()? == 1),
            // a purge_repo slice drops the schema under its own outcome span —
            // "no such table" means the row is gone for good: not landed, not an
            // error worth logging on every successful purge
            Err(e) if e.message().contains("no such table") => Ok(false),
            Err(e) => Err(e),
        }
    };
    let emit = |event: &str, outcome: &str, will_retry: bool, class: &str| {
        platform::job_event(&d.env, &repo, job.kind.as_str(), event, outcome, attempt, dur_ms, will_retry, class);
    };
    // a fenced write that lands nothing means this slice lost the row mid-flight —
    // report `stale`, never a retry (the new owner's attempt must not consume ours)
    match out {
        Ok(SliceOutcome::Done) => {
            if fenced("DELETE FROM jobs WHERE id=? AND state='running' AND lease=?", vec![])? {
                if job.kind == JobKind::Janitor {
                    enqueue(&d.sql(), JobKind::Janitor, now + 15 * 60 * 1000, "{}")?;
                }
                if job.kind == JobKind::GcSweep {
                    // chain completed — clear the resurrection counters repair uses
                    d.q("DELETE FROM meta WHERE key LIKE 'resurrected.gc_%'", vec![])?;
                }
                emit("done", "ok", false, "");
            } else {
                emit("done", "stale", false, "");
            }
        }
        Ok(SliceOutcome::Continue { cursor }) => {
            if fenced(
                "UPDATE jobs SET state='queued', run_at=?, cursor=?, strands=0, lease=NULL WHERE id=? AND state='running' AND lease=?",
                vec![V::from(now), V::from(cursor.as_str())],
            )? {
                emit("continue", "ok", true, "");
            } else {
                emit("continue", "stale", false, "");
            }
        }
        Ok(SliceOutcome::Reschedule { run_at }) => {
            if fenced(
                "UPDATE jobs SET state='queued', run_at=?, cursor=NULL, strands=0, lease=NULL WHERE id=? AND state='running' AND lease=?",
                vec![V::from(run_at)],
            )? {
                emit("reschedule", "ok", true, "");
            } else {
                emit("reschedule", "stale", false, "");
            }
        }
        Err(e) => {
            let backoff = (30_000i64).saturating_mul(1i64.checked_shl(attempt.min(20)).unwrap_or(1 << 20));
            let next = now.saturating_add(backoff.min(3_600_000));
            if attempt >= 8 {
                if fenced(
                    "UPDATE jobs SET state='dead', last_error=? WHERE id=? AND state='running' AND lease=?",
                    vec![V::from(e.to_string().as_str())],
                )? {
                    emit("dead", "dead", false, e.class());
                } else {
                    emit("dead", "stale", false, e.class());
                }
            } else if fenced(
                "UPDATE jobs SET state='queued', attempts=?, last_error=?, run_at=?, lease=NULL WHERE id=? AND state='running' AND lease=?",
                vec![V::from(i64::from(attempt)), V::from(e.to_string().as_str()), V::from(next)],
            )? {
                emit("retry", "retry", true, e.class());
            } else {
                emit("retry", "stale", false, e.class());
            }
        }
    }
    Ok(())
}

/// One slice of the job (4.5 dispatch arms).
pub async fn run_slice(d: &RepoDo, job: &Job, budget: &mut SliceBudget) -> Result<SliceOutcome, Error> {
    match job.kind {
        JobKind::Janitor => janitor::run_slice(d, job, budget).await,
        JobKind::GcMark => gc::gc_mark(d, job, budget).await,
        JobKind::GcConsolidate => gc::gc_consolidate(d, job, budget).await,
        JobKind::GcSweep => gc::gc_sweep(d).await,
        JobKind::PurgeRepo => purge::run_slice(d, job, budget).await,
        JobKind::ImportPack => import::run_slice(d, job, budget).await,
    }
}
