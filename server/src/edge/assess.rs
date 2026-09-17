//! The assess sidecar (read-only, off the git path): compose `ge-snapshot/v1`
//! — the versioned state document every evaluator consumes — send it to Jev,
//! and combine the typed answers into a disposition. Jev is evaluator #1
//! behind the snapshot seam; nothing here may decide anything on its own.

use serde_json::{json, Value};

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
}
