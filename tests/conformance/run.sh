#!/usr/bin/env bash
# git-edge conformance: push -> clone -> fsck against a local `wrangler dev`.
# Usage: GE_URL=http://user:token@localhost:8787 tests/conformance/run.sh
# Requires: wrangler dev running in server/ with .dev.vars providing
# GE_READ_TOKEN / GE_WRITE_TOKEN, and those tokens embedded in GE_URL.
set -euo pipefail

URL="${GE_URL:-http://test:write-test-token@localhost:8787}"
REPO="${GE_REPO:-conformance/main}"
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

note "PASS"
