//! The assess sidecar (read-only, off the git path): compose `ge-snapshot/v1`
//! — the versioned state document every evaluator consumes — send it to Jev,
//! and combine the typed answers into a disposition. Jev is evaluator #1
//! behind the snapshot seam; nothing here may decide anything on its own.

use serde_json::{json, Map, Value};
use worker::{Env, Method, Request, Response};

use crate::auth;
use crate::error::Error;
use crate::jev::{self, Answer};
use crate::wire::http::RepoRoute;
use crate::{ReqBudget, Spend};

/// Drift marker stamped on every response so sweep rows stay comparable as the
/// question set evolves (plan D8). Bump when ASSESS_QUESTIONS changes.
pub const QUESTIONS_VERSION: &str = "ge-questions/1";

/// The fixed assess question set — one fan-out, evaluated in parallel against
/// the same snapshot. Composition and thresholds stay in code below; the
/// questions only supply calibrated probabilities.
fn questions() -> Value {
    json!({
        "repo_kind": {
            "type": "choice",
            "instructions": "What kind of project lives in this repository?",
            "criteria": {
                "app": "a deployable application or service",
                "library": "reusable code consumed by other projects",
                "experiment": "a spike, test fixture, or scratch repo",
                "docs": "documentation or content only",
                "other": "none of the above fits"
            }
        },
        "looks_disposable": {
            "type": "noul",
            "instructions": "This repository looks like a throwaway or short-lived experiment that could safely be garbage-collected"
        },
        "abuse_suspect": {
            "type": "noul",
            "instructions": "The activity pattern suggests abuse: spam pushes, storage squatting, or automated noise rather than a real project"
        },
        "health": {
            "type": "score",
            "instructions": "Operational health of this repository based on dead jobs, wedged imports, and activity counters",
            "criteria": ["wedged or broken", "degraded", "healthy"]
        }
    })
}

/// Tombstoned or mid-import repos get `skip` without a model call (T4) —
/// transient state is neither meaningful to evaluate nor worth sending to a
/// third-party API.
fn transient(snapshot: &Value) -> bool {
    let s = &snapshot["state"];
    s["deleted"].as_bool().unwrap_or(false)
        || s["marked"].as_i64().unwrap_or(0) > 0
        || s["packs_ingesting"].as_i64().unwrap_or(0) > 0
}

/// Confidence a noul answer can stand in for: noul answers omit `confidence`,
/// but the noul value IS the calibrated yes-probability — a 0.99 is its own
/// certainty signal.
fn conf(a: &Answer) -> f64 {
    a.confidence.or(a.noul).unwrap_or(0.0)
}

/// The triage label — plain code over typed answers, per the composite-scoring
/// pattern: Jev supplies probabilities, thresholds live here where they're
/// testable. Order matters: investigate beats page beats ttl beats noise.
pub fn disposition(snapshot: &Value, answers: Option<&Map<String, Value>>) -> &'static str {
    if transient(snapshot) {
        return "skip";
    }
    let Some(a) = answers else {
        return "unavailable";
    };
    let get = |id: &str| Answer::get(a, id);
    let abuse = get("abuse_suspect");
    if abuse.as_ref().and_then(|x| x.noul).unwrap_or(0.0) > 0.9
        && abuse.as_ref().map(conf).unwrap_or(0.0) > 0.7
    {
        return "investigate";
    }
    if get("health").and_then(|x| x.score).unwrap_or(1.0) < 0.5 {
        return "page-operator";
    }
    if get("looks_disposable").and_then(|x| x.noul).unwrap_or(0.0) > 0.8 {
        return "ttl-candidate";
    }
    if get("repo_kind").and_then(|x| x.confidence).unwrap_or(0.0) < 0.5 {
        return "inconclusive";
    }
    "ok"
}

/// GET variant of `stub_json` — the DO read routes are all GET.
async fn do_get_json(
    stub: &worker::Stub,
    route: &RepoRoute,
    path: &str,
    budget: &mut ReqBudget,
) -> Result<Value, Error> {
    let mut init = worker::RequestInit::new();
    init.with_method(Method::Get);
    let mut r = worker::Request::new_with_init(&format!("https://do{path}"), &init)
        .map_err(|e| Error::Internal(e.to_string()))?;
    route.apply_headers(&mut r)?;
    budget.charge(1)?;
    let mut resp = stub.fetch_with_request(r).await?;
    if resp.status_code() != 200 {
        return Err(Error::from_do_response(resp, budget).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| Error::Internal(format!("do response: {e}")))
}

/// GET /:owner/:repo/_admin/assess — read-only operator assessment. Global
/// admin token only (same gate as export): the snapshot leaves the zone for a
/// third-party API, so it must stay an operator-controlled endpoint.
///
/// Jev failure is a 200 with `answers: null` + an `error` label — a null row
/// in a sweep is more useful than a missing one, and nothing here may ever
/// break a git operation.
pub async fn assess(req: &Request, env: &Env, route: &RepoRoute, spend: &Spend) -> Result<Response, Error> {
    auth::authenticate_admin(req, env)?;
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid().reporting(spend));
    let state = do_get_json(&stub, route, "/_do/state", &mut budget).await?;
    let log = do_get_json(&stub, route, "/_do/log", &mut budget).await?;
    let snap = snapshot(&route.owner, &route.repo, &state, &log);

    let (answers, error) = if transient(&snap) {
        (Value::Null, Value::Null)
    } else {
        match jev::evaluate(env, &snap.to_string(), &questions(), &mut budget).await {
            Ok(a) => (Value::Object(a), Value::Null),
            Err(e) => {
                worker::console_log!("assess {}/{}: jev {}", route.owner, route.repo, e.label());
                (Value::Null, json!(e.label()))
            }
        }
    };
    let out = json!({
        "schema": "ge-assess/v1",
        "questions_version": QUESTIONS_VERSION,
        "snapshot": snap,
        "answers": answers,
        "error": error,
        "disposition": disposition(&snap, answers.as_object()),
    });
    let body = serde_json::to_vec(&out).map_err(|e| Error::Internal(e.to_string()))?;
    super::git_resp(body, "application/json")
}

/// `ge-snapshot/v1` — the committed extension seam. Fields are additive-only
/// from here on; bump the schema string for anything that isn't.
pub fn snapshot(owner: &str, repo: &str, state: &Value, log: &Value) -> Value {
    json!({
        "schema": "ge-snapshot/v1",
        "repo": { "owner": owner, "name": repo },
        // state verbatim — marked/deleted/packs_ingesting drive the skip
        // short-circuit in disposition()
        "state": state,
        "head": log.get("head").cloned().unwrap_or(Value::Null),
        "refs": log.get("refs").cloned().unwrap_or(Value::Null),
        "subjects": log.get("subjects").cloned().unwrap_or(json!([])),
        "file_ext": log.get("file_ext").cloned().unwrap_or(json!({})),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the v1 field set — the document is the product seam, so accidental
    /// schema drift must fail loudly here rather than in a sweep diff.
    #[test]
    fn snapshot_schema_is_pinned() {
        let state = json!({"objects": 10, "refs": 2, "deleted": false, "marked": 0, "packs_ingesting": 0});
        let log = json!({
            "head": "refs/heads/main",
            "refs": [{"name": "refs/heads/main", "target": "aa", "peeled": null}],
            "subjects": [{"sha": "aa", "ts": 1, "subject": "init"}],
            "file_ext": {"rs": 3}
        });
        let snap = snapshot("o", "r", &state, &log);
        assert_eq!(snap["schema"], "ge-snapshot/v1");
        let keys: Vec<&str> = snap.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["file_ext", "head", "refs", "repo", "schema", "state", "subjects"]);
        // tombstone fields ride through verbatim — disposition depends on them
        assert_eq!(snap["state"]["deleted"], false);
        assert_eq!(snap["state"]["packs_ingesting"], 0);
        // missing log fields degrade to empty, not absent
        let bare = snapshot("o", "r", &state, &json!({}));
        assert_eq!(bare["subjects"], json!([]));
        assert_eq!(bare["file_ext"], json!({}));
    }

    fn snap(state_extra: Value) -> Value {
        let mut state = json!({"deleted": false, "marked": 0, "packs_ingesting": 0});
        state.as_object_mut().unwrap().extend(state_extra.as_object().unwrap().clone());
        snapshot("o", "r", &state, &json!({}))
    }

    fn answers(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn disposition_thresholds() {
        // tombstoned / mid-import short-circuits before any answers matter
        assert_eq!(disposition(&snap(json!({"deleted": true})), None), "skip");
        assert_eq!(disposition(&snap(json!({"marked": 1})), None), "skip");
        assert_eq!(disposition(&snap(json!({"packs_ingesting": 2})), None), "skip");

        let live = snap(json!({}));
        // jev down/unconfigured → unavailable, never a fabricated verdict
        assert_eq!(disposition(&live, None), "unavailable");

        let healthy = answers(json!({
            "repo_kind": {"type": "choice", "choice": "app", "confidence": 0.9},
            "looks_disposable": {"type": "noul", "noul": 0.1},
            "abuse_suspect": {"type": "noul", "noul": 0.05},
            "health": {"type": "score", "score": 1.9, "confidence": 0.8}
        }));
        assert_eq!(disposition(&live, Some(&healthy)), "ok");

        // abuse needs BOTH a high noul and high confidence
        let mut abuse_low_conf = healthy.clone();
        abuse_low_conf["abuse_suspect"] = json!({"type": "noul", "noul": 0.95});
        abuse_low_conf["abuse_suspect"]["confidence"] = json!(0.5);
        assert_eq!(disposition(&live, Some(&abuse_low_conf)), "ok");
        abuse_low_conf["abuse_suspect"]["confidence"] = json!(0.8);
        assert_eq!(disposition(&live, Some(&abuse_low_conf)), "investigate");

        let mut wedged = healthy.clone();
        wedged["health"] = json!({"type": "score", "score": 0.2, "confidence": 0.9});
        assert_eq!(disposition(&live, Some(&wedged)), "page-operator");

        let mut disposable = healthy.clone();
        disposable["looks_disposable"] = json!({"type": "noul", "noul": 0.9});
        assert_eq!(disposition(&live, Some(&disposable)), "ttl-candidate");

        let mut vague = healthy.clone();
        vague["repo_kind"] = json!({"type": "choice", "choice": "other", "confidence": 0.3});
        assert_eq!(disposition(&live, Some(&vague)), "inconclusive");

        // investigate wins over page-operator when both fire
        let mut both = wedged.clone();
        both["abuse_suspect"] = json!({"type": "noul", "noul": 0.95, "confidence": 0.9});
        assert_eq!(disposition(&live, Some(&both)), "investigate");
    }
}
