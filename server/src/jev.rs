//! Jev / System One client (docs.typesafe.ai): the ONLY module allowed to make
//! outbound vendor calls from this codebase — the DO never sees it. One POST,
//! zero retries, bounded by GE_JEV_TIMEOUT_MS; the caller renders failure as
//! `answers: null`, so vendor state can never touch the git path.

use std::time::Duration;

use futures_util::{future::Either, pin_mut, FutureExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use worker::{AbortController, Delay, Env, Fetch, Headers, Method, Request, RequestInit};

use crate::ReqBudget;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";

/// Why Jev didn't answer. Structured so the assess route can serialize a stable
/// `error` label without scraping Display text.
#[derive(Debug)]
pub enum JevError {
    /// TYPESAFE_API_KEY isn't configured — expected in dev, not a failure.
    Unavailable,
    Timeout,
    Transport(String),
    Http(u16),
    Decode(String),
}

impl JevError {
    /// Stable wire string for the assess response's `error` field.
    pub fn label(&self) -> String {
        match self {
            JevError::Unavailable => "unavailable".into(),
            JevError::Timeout => "timeout".into(),
            JevError::Transport(_) => "transport".into(),
            JevError::Http(s) => format!("http-{s}"),
            JevError::Decode(_) => "decode".into(),
        }
    }
}

/// One typed answer — the union of the three System One shapes. Each question
/// type fills a different subset, and noul answers may omit `confidence`
/// entirely, so everything optional is Option.
#[derive(Debug, Clone, Deserialize)]
pub struct Answer {
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub noul: Option<f64>,
    #[serde(default)]
    pub probabilities: Option<Map<String, Value>>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

impl Answer {
    /// One question's answer out of the wire map; absent or malformed → None,
    /// which disposition() reads as "don't trust it" rather than "no".
    pub fn get(answers: &Map<String, Value>, id: &str) -> Option<Self> {
        answers
            .get(id)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// POST one fan-out: state + questions → typed answers keyed by question id.
/// No retries — assess is read-only, so a null answer row is more useful to a
/// sweep than a delayed one.
pub async fn evaluate(
    env: &Env,
    state: &str,
    questions: &Value,
    budget: &mut ReqBudget,
) -> Result<Map<String, Value>, JevError> {
    let key = env
        .secret("TYPESAFE_API_KEY")
        .ok()
        .map(|s| s.to_string())
        .ok_or(JevError::Unavailable)?;

    budget
        .charge(1)
        .map_err(|e| JevError::Transport(e.to_string()))?;
    let headers = Headers::new();
    headers
        .set("authorization", &format!("Bearer {key}"))
        .and_then(|_| headers.set("content-type", "application/json"))
        .map_err(|e| JevError::Transport(e.to_string()))?;
    let body = json!({ "state": state, "model": MODEL, "questions": questions }).to_string();
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.into()));
    let req = Request::new_with_init(ENDPOINT, &init)
        .map_err(|e| JevError::Transport(e.to_string()))?;

    // Bound the wait: the isolate bills while a fetch is open, and a wedged
    // vendor must not turn an admin endpoint into a hang. Abort cancels the
    // in-flight request rather than just abandoning it.
    let timeout_ms = env
        .var("GE_JEV_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.to_string().parse::<u64>().ok())
        .unwrap_or(8_000);
    let controller = AbortController::default();
    let signal = controller.signal();
    let fetch = Fetch::Request(req);
    let send = fetch.send_with_signal(&signal).fuse();
    let delay = Delay::from(Duration::from_millis(timeout_ms)).fuse();
    pin_mut!(send, delay);
    let mut resp = match futures_util::future::select(send, delay).await {
        Either::Left((r, _)) => r.map_err(|e| JevError::Transport(e.to_string()))?,
        Either::Right(((), _pending)) => {
            controller.abort();
            return Err(JevError::Timeout);
        }
    };

    let status = resp.status_code();
    if !(200..300).contains(&status) {
        return Err(JevError::Http(status));
    }
    #[derive(Deserialize)]
    struct Wire {
        answers: Map<String, Value>,
    }
    let wire: Wire = resp
        .json()
        .await
        .map_err(|e| JevError::Decode(e.to_string()))?;
    Ok(wire.answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canned response copied from the System One quickstart's response body —
    /// pins the wire shape so an upstream field rename fails loudly here.
    #[test]
    fn wire_round_trip() {
        let body = r#"{
            "model": "jev-latest",
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "technical",
                    "probabilities": {"billing": 0.159, "technical": 0.84, "sales": 0.001},
                    "confidence": 0.596
                },
                "frustration": {
                    "type": "score",
                    "score": 1.035,
                    "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                    "confidence": 0.842
                },
                "is_urgent": {"type": "noul", "noul": 0.999}
            },
            "usage": {"input_tokens": 312, "output_tokens": 48}
        }"#;
        #[derive(Deserialize)]
        struct Wire {
            answers: Map<String, Value>,
        }
        let w: Wire = serde_json::from_str(body).unwrap();
        assert_eq!(w.answers.len(), 3);

        let dept = Answer::get(&w.answers, "department").unwrap();
        assert_eq!(dept.choice.as_deref(), Some("technical"));
        assert_eq!(dept.confidence, Some(0.596));

        let fr = Answer::get(&w.answers, "frustration").unwrap();
        assert_eq!(fr.score, Some(1.035));

        // noul answers carry no confidence field — Option must not fabricate one
        let urg = Answer::get(&w.answers, "is_urgent").unwrap();
        assert_eq!(urg.noul, Some(0.999));
        assert_eq!(urg.confidence, None);

        assert!(Answer::get(&w.answers, "missing").is_none());
    }

    #[test]
    fn error_labels_are_stable() {
        assert_eq!(JevError::Unavailable.label(), "unavailable");
        assert_eq!(JevError::Timeout.label(), "timeout");
        assert_eq!(JevError::Http(429).label(), "http-429");
    }
}
