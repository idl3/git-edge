#!/usr/bin/env bash
# ge-sweep — one-shot fleet triage: enumerate an owner's repos via
# /<owner>/_admin/repos, run the /_admin/assess sidecar on each, and print a
# severity-sorted table (worst first).
#
#   GE_TOKEN=... tools/ge-sweep.sh <owner>
#   GE_TOKEN=... GE_URL=https://edge.example.com tools/ge-sweep.sh acme
#
# Env:
#   GE_URL    deployment base (def http://localhost:8787 — a `wrangler dev`)
#   GE_TOKEN  global write token; fed to curl through --config stdin, so it
#             never appears in argv or a URL. Unset = rely on creds in GE_URL.
#
# Serial on purpose: each assess is two DO reads + one Jev call. To parallelize,
# take the repo list and fan it out yourself:
#   curl ... /$OWNER/_admin/repos | jq -r '.repos[]' | \
#     xargs -P4 -I{} curl ... "$GE_URL/$OWNER/{}/_admin/assess"
set -euo pipefail

note() { echo "== $*" >&2; }
die()  { echo "ge-sweep: FAIL: $*" >&2; exit 1; }
skip() { echo "[e2e:skipped] reason: $*" >&2; exit 0; }

OWNER="${1:-}"
[ -n "$OWNER" ] || die "usage: GE_TOKEN=... tools/ge-sweep.sh <owner>"
EXPLICIT_URL="${GE_URL:-}"
GE_URL="${EXPLICIT_URL:-http://localhost:8787}"
command -v python3 >/dev/null || die "python3 required"

# Reachability: an explicit GE_URL that fails is a hard error; the default
# :8787 target failing just means no dev server — skip, don't fail CI.
if ! curl -sf -o /dev/null --max-time 3 "$GE_URL/healthz"; then
  [ -n "$EXPLICIT_URL" ] && die "no response from $GE_URL/healthz"
  skip "no dev server at $GE_URL"
fi

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# The Authorization header travels via stdin config — never argv or a URL.
auth_config() {
  [ -n "${GE_TOKEN:-}" ] && printf 'header = "Authorization: Bearer %s"\n' "$GE_TOKEN" || true
}

note "repos for $OWNER"
auth_config | curl -sf -K - -o "$WORK/repos.json" "$GE_URL/$OWNER/_admin/repos" \
  || die "GET /_admin/repos failed (check GE_URL + GE_TOKEN)"
python3 - "$WORK/repos.json" > "$WORK/repos" <<'PY'
import json, sys
print("\n".join(json.load(open(sys.argv[1])).get("repos", [])))
PY
[ -s "$WORK/repos" ] || { note "no repos claimed by $OWNER"; exit 0; }

: > "$WORK/rows"
while IFS= read -r repo; do
  [ -n "$repo" ] || continue
  if auth_config | curl -sf -K - -o "$WORK/a.json" "$GE_URL/$OWNER/$repo/_admin/assess"; then
    python3 - "$WORK/a.json" "$repo" >> "$WORK/rows" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
repo = sys.argv[2]
disp = d.get("disposition") or "unknown"
conf = ((d.get("answers") or {}).get("repo_kind") or {}).get("confidence")
conf = f"{conf:.2f}" if isinstance(conf, (int, float)) else "-"
print(f"{disp}\t{conf}\t{repo}")
PY
  else
    # a wedged repo is itself a triage signal — record it, keep sweeping
    printf 'fetch-error\t-\t%s\n' "$repo" >> "$WORK/rows"
  fi
done < "$WORK/repos"

note "disposition | confidence | repo (worst first)"
python3 - "$WORK/rows" <<'PY'
import sys
RANK = {"fetch-error": 0, "investigate": 1, "page-operator": 2, "inconclusive": 3,
        "ttl-candidate": 4, "unavailable": 5, "skip": 6, "ok": 7}
rows = [l.rstrip("\n").split("\t") for l in open(sys.argv[1])]
rows.sort(key=lambda r: (RANK.get(r[0], 8), r[2]))
for disp, conf, repo in rows:
    print(f"{disp:<15} {conf:>5}  {repo}")
PY
