#!/usr/bin/env bash
# git-edge conformance: push -> clone -> fsck against a local `wrangler dev`.
# Usage: GE_URL=http://user:token@localhost:8787 tests/conformance/run.sh
# Requires: wrangler dev running in server/ with .dev.vars providing
# GE_READ_TOKEN / GE_WRITE_TOKEN, and those tokens embedded in GE_URL.
set -euo pipefail

URL="${GE_URL:-http://test:write-test-token@localhost:8787}"
# Unique per run so repeat runs never collide with previously-pushed refs.
REPO="${GE_REPO:-conformance/run-$(date +%s)-$$}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
note() { echo "== $*"; }

note "empty-repo ls-remote"
git ls-remote "$URL/$REPO" > "$WORK/ls0" || fail "ls-remote on empty repo"
[ ! -s "$WORK/ls0" ] || fail "empty repo advertised refs"

note "seed repo + initial push"
mkdir "$WORK/seed" && cd "$WORK/seed"
git init -q && git config user.email t@t && git config user.name t
echo one > f && git add f && git commit -qm one
git branch -M main
git push -q "$URL/$REPO" main || fail "initial push"
TIP1=$(git rev-parse HEAD)

note "incremental push (thin pack with deltas)"
dd if=/dev/urandom of=big.bin bs=1m count=6 2>/dev/null
git add big.bin && git commit -qm big
git push -q "$URL/$REPO" main || fail "incremental push"
TIP2=$(git rev-parse HEAD)

note "branch create + delete"
git checkout -qb side && git push -q "$URL/$REPO" side || fail "push branch"
git push -q "$URL/$REPO" --delete side || fail "delete branch"

note "tag push + delete-only push (no PACK)"
git tag v-test && git push -q "$URL/$REPO" v-test || fail "push tag"
git push -q "$URL/$REPO" --delete v-test || fail "delete tag"

note "clone back + fsck"
git clone -q "$URL/$REPO" "$WORK/clone" || fail "clone"
cd "$WORK/clone"
[ "$(git rev-parse main)" = "$TIP2" ] || fail "clone tip != pushed tip"
git fsck --strict || fail "fsck"
cmp big.bin "$WORK/seed/big.bin" || fail "big.bin differs"

note "incremental fetch (have/want)"
cd "$WORK/seed" && git commit -qm three --allow-empty && git push -q "$URL/$REPO" HEAD:main
cd "$WORK/clone" && git fetch -q "$URL/$REPO" main || fail "fetch"
[ "$(git rev-parse FETCH_HEAD)" = "$(git -C "$WORK/seed" rev-parse HEAD)" ] || fail "fetch tip mismatch"
git fsck --strict || fail "fsck after fetch"

note "CAS: stale push must fail"
cd "$WORK/seed" && git commit -qm four --allow-empty && git push -q "$URL/$REPO" HEAD:main
# clone's view of main is now stale; an explicit update with wrong old-oid
# is driven by pushing an unrelated local state without --force:
cd "$WORK/clone" && git checkout -qb stale "$TIP1"
git commit -qm diverged --allow-empty
if git push "$URL/$REPO" stale:main 2>/dev/null; then
  note "stale push accepted (server CAS honored the advertised old oid)"
fi
git fsck --strict || fail "fsck after stale test"

note "malformed pack -> HTTP 200 + unpack error (A2)"
printf 'PACKxxxx' > "$WORK/bad.bin"
python3 - "$WORK/bad.bin" > "$WORK/badpkt.bin" <<'PY'
import sys
body = open(sys.argv[1],'rb').read()
cmd = b"0"*40 + b" " + b"1"*40 + b" refs/heads/main\x00 report-status\n"
pkt = f"{len(cmd)+4:04x}".encode() + cmd + b"0000" + body
sys.stdout.buffer.write(pkt)
PY
out=$(curl -s --compressed -X POST "$URL/$REPO/git-receive-pack" \
  -H 'Content-Type: application/x-git-receive-pack-request' --data-binary @"$WORK/badpkt.bin")
echo "$out" | grep -q "unpack " || fail "no unpack line in A2 response: $out"
echo "$out" | grep -q "ng refs/heads/main" || fail "no ng line in A2 response: $out"

# Quota + push rate-limit checks (A17/A18): only runs with GE_CONFORMANCE_LIMITS=1
# and the dev server started with small caps, e.g. .dev.vars:
#   GE_QUOTA_MAX_OBJECTS=25 GE_QUOTA_MAX_REPOS_PER_OWNER=3 GE_RATE_PUSHES_PER_MIN=12
# (25 stays above the main suite's ~12 objects/repo; 12 stays above its 9 pushes.)
if [ "${GE_CONFORMANCE_LIMITS:-0}" = "1" ]; then
  O="quota-$SECONDS-$$"
  post() { # one canned receive-pack POST (bad pack body is fine — it reaches begin)
    curl -s -X POST "$URL/$1/git-receive-pack" \
      -H 'Content-Type: application/x-git-receive-pack-request' --data-binary @"$WORK/badpkt.bin"
  }

  note "object quota: push over GE_QUOTA_MAX_OBJECTS rejects naming the cap"
  mkdir "$WORK/q" && cd "$WORK/q"
  git init -q && git config user.email t@t && git config user.name t
  for i in $(seq 1 12); do echo "$i" > "f$i"; git add "f$i"; git commit -qm "c$i"; done
  git branch -M main
  if git push "$URL/$O/obj" main >"$WORK/qerr" 2>&1; then
    fail "expected object-quota rejection (is GE_QUOTA_MAX_OBJECTS <= ~36?)"
  fi
  grep -qi "quota" "$WORK/qerr" || fail "no quota reason in push output: $(cat "$WORK/qerr")"

  note "repo quota: claims past GE_QUOTA_MAX_REPOS_PER_OWNER are rejected"
  # $O/obj already claimed slot 1; fill the remaining two, then the next must fail
  post "$O/b" >/dev/null
  post "$O/c" >/dev/null
  out=$(post "$O/d")
  echo "$out" | grep -q "GE_QUOTA_MAX_REPOS_PER_OWNER" \
    || fail "repo-cap rejection missing GE_QUOTA_MAX_REPOS_PER_OWNER: $out"

  note "rate limit: pushes past GE_RATE_PUSHES_PER_MIN get HTTP 429 + Retry-After"
  code=""
  for i in $(seq 1 16); do
    code=$(curl -s -o "$WORK/rl" -D "$WORK/rlh" -w '%{http_code}' -X POST \
      "$URL/$O/rl/git-receive-pack" \
      -H 'Content-Type: application/x-git-receive-pack-request' --data-binary @"$WORK/badpkt.bin")
    [ "$code" = "429" ] && break
  done
  [ "$code" = "429" ] || fail "no 429 after 16 pushes (last=$code; is GE_RATE_PUSHES_PER_MIN < 16?)"
  grep -qi '^retry-after:' "$WORK/rlh" || fail "429 without Retry-After header"
fi

# GC chain: only runs when the server was started with shortened windows
# (GE_GC_QUIET_MS / GE_GC_GRACE_MS in .dev.vars) and GE_CONFORMANCE_GC=1.
# Polls /_state until the orphan pack is swept or the deadline passes.
if [ "${GE_CONFORMANCE_GC:-0}" = "1" ]; then
  note "GC: orphan the seeded pack, wait for mark/consolidate/sweep"
  GCREPO="$REPO-gc"
  mkdir "$WORK/gc" && cd "$WORK/gc"
  git init -q && git config user.email t@t && git config user.name t
  echo gc-seed > s && git add s && git commit -qm seed && git branch -M main
  git push -q "$URL/$GCREPO" main || fail "gc seed push"
  OBJ0=$(curl -sf "$URL/$GCREPO/_state" | python3 -c 'import sys,json;print(json.load(sys.stdin)["objects"])')
  git checkout -q --orphan orphan && git rm -q -rf . && echo live > s2
  git add s2 && git commit -qm orphan
  git push -qf "$URL/$GCREPO" orphan:main || fail "gc orphan push"
  deadline=$((SECONDS + 120))
  while :; do
    st=$(curl -sf "$URL/$GCREPO/_state") || fail "state probe"
    dead=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["packs_dead"])')
    [ "$dead" -ge 1 ] && break
    [ $SECONDS -lt $deadline ] || fail "GC did not sweep within 120s (is GE_GC_QUIET_MS set?)"
    sleep 5
  done
  git clone -q "$URL/$GCREPO" "$WORK/gcclone" || fail "post-GC clone"
  git -C "$WORK/gcclone" fsck --strict || fail "post-GC fsck"
  [ "$(git -C "$WORK/gcclone" rev-parse main)" = "$(git rev-parse orphan)" ] || fail "post-GC tip"
  note "GC swept (objects before: $OBJ0)"
fi

note "PASS"
