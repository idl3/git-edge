//! Push events: a signed webhook per committed push, delivered from a durable
//! outbox. commit_push writes one `push_events` row in the same sync span as the
//! ref writes (so an event exists iff refs moved) and enqueues `push_event`;
//! this job drains the outbox oldest-first, one HTTPS POST per row.
//!
//! Feature switch: GE_PUSH_WEBHOOK_URL (var) AND GE_PUSH_WEBHOOK_SECRET
//! (secret, >= 32 bytes). Either missing = off: no rows written, nothing sent —
//! an event is never delivered unsigned.
//!
//! Wire: POST <url>, JSON body, headers
//!   x-ge-event: push
//!   x-ge-delivery: <repo_id>:<seq>      (stable across retries — dedup on it)
//!   x-ge-timestamp: <epoch ms at send>
//!   x-ge-signature: v1=<hex HMAC-SHA256(secret, "v1\n<ts>\n<delivery>\n" + body)>
//! Receivers verify the signature, bound the timestamp, and dedup on delivery.
//! Delivery is at-least-once and in order: the drain stops at the first failure
//! and the job retries with the dispatcher's backoff.

use worker::{Fetch, Headers, Method, Request, RequestInit, SqlStorageValue as V};

use super::{Job, SliceBudget, SliceOutcome};
use crate::error::Error;
use crate::platform;
use crate::repo_do::RepoDo;

/// Rows per slice; the 80% budget check usually ends a slice first.
const BATCH: i64 = 25;
/// Outbox bound. Past it, commits still land (a dead receiver must not stop
/// pushes) and `meta.push_events_dropped` counts what wasn't recorded.
pub const MAX_PENDING: i64 = 10_000;

/// (url, secret) when the feature is on.
pub fn config(env: &worker::Env) -> Option<(String, String)> {
    let url = env.var("GE_PUSH_WEBHOOK_URL").ok().map(|v| v.to_string()).filter(|u| !u.is_empty())?;
    let secret = env
        .secret("GE_PUSH_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.to_string())
        .or_else(|| env.var("GE_PUSH_WEBHOOK_SECRET").ok().map(|v| v.to_string()))
        .filter(|s| s.len() >= 32)?;
    Some((url, secret))
}

#[derive(serde::Deserialize)]
struct Row {
    seq: i64,
    body: String,
}

pub async fn run_slice(d: &RepoDo, _job: &Job, budget: &mut SliceBudget) -> Result<SliceOutcome, Error> {
    let Some((url, secret)) = config(&d.env) else {
        return Ok(SliceOutcome::Done); // switched off since enqueue: rows wait for it to return
    };
    let repo_id = d.repo_id()?.0;
    let rows = d
        .q("SELECT seq, body FROM push_events ORDER BY seq LIMIT ?", vec![V::from(BATCH)])?
        .to_array::<Row>()?;
    for r in &rows {
        if budget.spent_80pct() {
            return Ok(SliceOutcome::Continue { cursor: String::new() });
        }
        budget.req.charge(1)?;
        let delivery = format!("{repo_id}:{}", r.seq);
        let ts = platform::now_ms();
        let sig = crate::sign::event_sig(&secret, ts, &delivery, r.body.as_bytes())?;
        match post(&url, &delivery, ts, &sig, &r.body).await {
            Ok(()) => {
                d.q("DELETE FROM push_events WHERE seq=?", vec![V::from(r.seq)])?;
            }
            Err(e) => {
                d.q(
                    "UPDATE push_events SET attempts=attempts+1, last_error=? WHERE seq=?",
                    vec![V::from(e.to_string().as_str()), V::from(r.seq)],
                )?;
                // in-order: later rows wait behind this one; the dispatcher backs off
                return Err(e);
            }
        }
    }
    if rows.len() as i64 == BATCH {
        return Ok(SliceOutcome::Continue { cursor: String::new() });
    }
    Ok(SliceOutcome::Done)
}

async fn post(url: &str, delivery: &str, ts: i64, sig: &str, body: &str) -> Result<(), Error> {
    let h = Headers::new();
    let set = |k: &str, v: &str| h.set(k, v).map_err(|e| Error::Internal(e.to_string()));
    set("content-type", "application/json")?;
    set("user-agent", "git-edge-push-events/1")?;
    set("x-ge-event", "push")?;
    set("x-ge-delivery", delivery)?;
    set("x-ge-timestamp", &ts.to_string())?;
    set("x-ge-signature", &format!("v1={sig}"))?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(h).with_body(Some(body.into()));
    let req = Request::new_with_init(url, &init).map_err(|e| Error::Internal(e.to_string()))?;
    let resp = Fetch::Request(req)
        .send()
        .await
        .map_err(|e| Error::Internal(format!("push webhook: {e}")))?;
    let code = resp.status_code();
    if !(200..300).contains(&code) {
        // retryable by design: a receiver outage must not lose events
        return Err(Error::Internal(format!("push webhook answered HTTP {code}")));
    }
    Ok(())
}
