//! Job dispatcher (CONTRACTS.md 4, 5; A3, A4) + running-row repair (A12).
//! `rearm` is the sole `set_alarm` caller; `enqueue` is sync and only writes a row.

pub mod gc;
pub mod janitor;

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
}
impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Janitor => "janitor",
            JobKind::GcMark => "gc_mark",
            JobKind::GcConsolidate => "gc_consolidate",
            JobKind::GcSweep => "gc_sweep",
        }
    }
    fn of(s: &str) -> Result<Self, Error> {
        match s {
            "janitor" => Ok(JobKind::Janitor),
            "gc_mark" => Ok(JobKind::GcMark),
            "gc_consolidate" => Ok(JobKind::GcConsolidate),
            "gc_sweep" => Ok(JobKind::GcSweep),
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
            req: ReqBudget { max_subrequests: 400, used: 0, started_ms: js_sys::Date::now(), max_ms: 20_000.0 },
        }
    }
    pub fn spent_80pct(&self) -> bool {
        let elapsed = js_sys::Date::now() - self.started_ms;
        elapsed > 16_000.0 || self.req.used > 320
    }
}

/// A checkpoint span refreshes the lease: `repair` may only requeue a row whose started_at
/// has gone stale — a progressing slice heartbeats and is never mistaken for a stranded one.
pub fn heartbeat(sql: &SqlStorage, job_id: i64) -> Result<(), Error> {
    sql.exec(
        "UPDATE jobs SET started_at=? WHERE id=? AND state='running'",
        Some(vec![V::from(platform::now_ms()), V::from(job_id)]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
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
    // a stranded 'running' row is a crashed slice — it must count as an attempt or a
    // crash-looping job retries at every boot forever, bypassing the dead threshold
    sql.exec(
        "UPDATE jobs SET state='dead', last_error='stranded: attempts exhausted' \
         WHERE state='running' AND started_at < ? AND attempts >= 8",
        Some(vec![V::from(now.saturating_sub(60_000))]),
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    sql.exec(
        "UPDATE jobs SET state='queued', run_at=?, attempts=attempts+1, last_error='stranded mid-slice' \
         WHERE state='running' AND started_at < ? AND attempts < 8",
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
            sql.exec("DELETE FROM jobs WHERE kind=? AND state='dead'", Some(vec![V::from(kind.as_str())]))
                .map_err(|e| Error::Storage(e.to_string()))?;
            enqueue(sql, kind, now, "{}")?;
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
    let _ = rearm(d).await;
    r
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
    // mark running + record started_at (A12) in one sync span
    d.q(
        "UPDATE jobs SET state='running', started_at=? WHERE id=? AND state='queued'",
        vec![V::from(now), V::from(r.id)],
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
    };
    let mut budget = SliceBudget::fresh();
    let out = run_slice(d, &job, &mut budget).await;
    worker::console_log!("job {}#{} attempts={} -> {}", job.kind.as_str(), job.id, job.attempts,
        match &out { Ok(SliceOutcome::Done) => "done".into(),
            Ok(SliceOutcome::Continue { .. }) => "continue".into(),
            Ok(SliceOutcome::Reschedule { .. }) => "reschedule".into(),
            Err(e) => format!("err {e}") });
    // apply the outcome in a sync span (4.2, 4.4)
    match out {
        Ok(SliceOutcome::Done) => {
            d.q("DELETE FROM jobs WHERE id=?", vec![V::from(r.id)])?;
            if job.kind == JobKind::Janitor {
                enqueue(&d.sql(), JobKind::Janitor, now + 15 * 60 * 1000, "{}")?;
            }
        }
        Ok(SliceOutcome::Continue { cursor }) => {
            d.q(
                "UPDATE jobs SET state='queued', run_at=?, cursor=? WHERE id=?",
                vec![V::from(now), V::from(cursor.as_str()), V::from(r.id)],
            )?;
        }
        Ok(SliceOutcome::Reschedule { run_at }) => {
            d.q(
                "UPDATE jobs SET state='queued', run_at=?, cursor=NULL WHERE id=?",
                vec![V::from(run_at), V::from(r.id)],
            )?;
        }
        Err(e) => {
            let attempts = job.attempts.saturating_add(1);
            let backoff = (30_000i64).saturating_mul(1i64.checked_shl(attempts.min(20)).unwrap_or(1 << 20));
            let next = now.saturating_add(backoff.min(3_600_000));
            if attempts >= 8 {
                d.q(
                    "UPDATE jobs SET state='dead', last_error=? WHERE id=?",
                    vec![V::from(e.to_string().as_str()), V::from(r.id)],
                )?;
            } else {
                d.q(
                    "UPDATE jobs SET state='queued', attempts=?, last_error=?, run_at=? WHERE id=?",
                    vec![V::from(i64::from(attempts)), V::from(e.to_string().as_str()), V::from(next), V::from(r.id)],
                )?;
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
    }
}
