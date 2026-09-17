#!/usr/bin/env bash
# assess e2e: fixture push -> GET /_admin/assess -> assert the ge-assess/v1 shape.
#
# Needs a live worker; resolution order:
#   1. GE_URL (creds embedded, same convention as tests/conformance/run.sh)
#   2. an already-running `wrangler dev` on :8787 (admin token from .dev.vars)
#   3. `npx wrangler dev` booted in the background when wrangler + .dev.vars exist
# Prints "[e2e:skipped] reason: ..." and exits 0 when none of those hold.
set -euo pipefail

note() { echo "== $*"; }
skip() { echo "[e2e:skipped] reason: $*"; exit 0; }
fail() { echo "FAIL: $*" >&2; exit 1; }

SERVER="$(cd "$(dirname "$0")/../../server" && pwd)"
WORK="$(mktemp -d)"
WRANGLER_PID=""
cleanup() {
  [ -n "$WRANGLER_PID" ] && kill "$WRANGLER_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

alive() { curl -sf -o /dev/null --max-time 2 "$1/healthz"; }

# .dev.vars is a dotenv file — parse the write token out, never echo it.
devvars_token() {
  sed -n 's/^GE_WRITE_TOKEN=//p' "$SERVER/.dev.vars" 2>/dev/null | head -1 | tr -d '"'"'"'\r' | tr -d "'"
}

URL="${GE_URL:-}"
if [ -z "$URL" ]; then
  if alive "http://localhost:8787"; then
    TOK="$(devvars_token)"
    [ -n "$TOK" ] || skip "localhost:8787 is up but $SERVER/.dev.vars has no GE_WRITE_TOKEN"
    URL="http://test:$TOK@localhost:8787"
  elif command -v npx >/dev/null && [ -f "$SERVER/.dev.vars" ]; then
    TOK="$(devvars_token)"
    [ -n "$TOK" ] || skip ".dev.vars exists but GE_WRITE_TOKEN is missing"
    note "booting wrangler dev in the background — first run builds the worker"
    (cd "$SERVER" && npx wrangler dev --port 8787 >"$WORK/wrangler.log" 2>&1) &
    WRANGLER_PID=$!
    for _ in $(seq 1 120); do
      alive "http://localhost:8787" && break
      kill -0 "$WRANGLER_PID" 2>/dev/null || {
        tail -20 "$WORK/wrangler.log" >&2
        skip "wrangler dev exited during boot"
      }
      sleep 1
    done
    alive "http://localhost:8787" || {
      tail -20 "$WORK/wrangler.log" >&2
      skip "wrangler dev did not come up in 120s"
    }
    URL="http://test:$TOK@localhost:8787"
  else
    skip "no GE_URL, nothing on :8787, and no wrangler+.dev.vars to boot one"
  fi
fi

REPO="e2e/assess-$(date +%s)-$$"
note "fixture repo -> $REPO (main + a second tip so the union walk is exercised)"
mkdir "$WORK/seed" && cd "$WORK/seed"
git init -q && git config user.email t@t && git config user.name t
echo one > f.rs && git add f.rs && git commit -qm "init rust fixture"
echo two > g.sh && git add g.sh && git commit -qm "add shell fixture"
git branch -M main
git push -q "$URL/$REPO" main || fail "initial push"
git checkout -qb side && echo three > h.md && git add h.md && git commit -qm "side branch commit"
git push -q "$URL/$REPO" side || fail "side-branch push"

note "GET /_admin/assess"
code=$(curl -s -o "$WORK/assess.json" -w '%{http_code}' "$URL/$REPO/_admin/assess")
[ "$code" = "200" ] || { cat "$WORK/assess.json" >&2; fail "assess returned $code, want 200"; }

python3 - "$WORK/assess.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d.get("schema") == "ge-assess/v1", d
assert d.get("questions_version"), d
snap = d.get("snapshot") or {}
assert snap.get("schema") == "ge-snapshot/v1", snap
# union walk: subjects from BOTH tips, not just where HEAD points
subs = {s.get("subject") for s in snap.get("subjects", [])}
assert "side branch commit" in subs and "init rust fixture" in subs, subs
# root-tree extension histogram of the head branch
ext = snap.get("file_ext") or {}
assert ext.get("rs") == 1 and ext.get("sh") == 1, ext
refs = {r.get("name") for r in snap.get("refs", [])}
assert "refs/heads/main" in refs and "refs/heads/side" in refs, refs
# no TYPESAFE_API_KEY in dev -> answers:null + an error label; with a key,
# answers arrive and disposition comes from the typed verdict
if d.get("answers") is None:
    assert d.get("error"), d
    assert d.get("disposition") in ("unavailable", "skip"), d
else:
    assert d.get("disposition") in (
        "ok", "investigate", "page-operator", "ttl-candidate", "inconclusive"), d
print("assess shape OK — disposition:", d.get("disposition"), "| error:", d.get("error"))
PY

note "e2e:assess PASS"
