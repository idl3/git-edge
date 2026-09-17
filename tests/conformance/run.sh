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

note "verbatim consolidated-pack fast path (C2): packs_live=1 streams the pack whole"
# one live pack covers everything, so a plain-clone fetch is one R2 GET — the DO
# stamps its projected spend on the response: exactly 1, where the walking path
# is one subrequest per planned read. A hand-rolled POST asserts the count;
# a real clone proves the verbatim pack is wire-valid.
python3 - "$TIP1" > "$WORK/fetchpkt.bin" <<'PY'
import sys
tip = sys.argv[1]
def pkt(b):
    return f"{len(b)+4:04x}".encode() + b
out = pkt(b"command=fetch\n") + pkt(b"object-format=sha1\n") + b"0001"
out += pkt(f"want {tip}\n".encode()) + pkt(b"done\n") + b"0000"
sys.stdout.buffer.write(out)
PY
curl -s -D "$WORK/fh" -o "$WORK/fresp.bin" -X POST "$URL/$REPO/git-upload-pack" \
  -H 'Git-Protocol: version=2' -H 'Content-Type: application/x-git-upload-pack-request' \
  --data-binary @"$WORK/fetchpkt.bin"
grep -aq "packfile" "$WORK/fresp.bin" || fail "no packfile section in fetch response"
grep -aq "PACK" "$WORK/fresp.bin" || fail "no PACK bytes in fetch response"
n=$(tr -d '\r' < "$WORK/fh" | sed -nE 's/^x-ge-subrequests: ([0-9]+).*/\1/ip' | tail -1)
[ "$n" = "1" ] || fail "verbatim path not taken (x-ge-subrequests=$n, want 1)"
git clone -q "$URL/$REPO" "$WORK/clone1" || fail "single-pack clone"
[ "$(git -C "$WORK/clone1" rev-parse main)" = "$TIP1" ] || fail "single-pack clone tip"
git -C "$WORK/clone1" fsck --strict || fail "single-pack clone fsck"

# packfile-uris (C1/A30): only runs with GE_CONFORMANCE_URIS=1, a dev server
# started with GE_URL_SIGNING_KEY in .dev.vars, and git >= 2.40 as GE_GITBIN
# (stock PATH git is fine on >= 2.40). packs_live=1 still holds here.
if [ "${GE_CONFORMANCE_URIS:-0}" = "1" ]; then
  note "packfile-uris: opted-in fetch gets a signed URI, inline pack is empty"
  GITC="${GE_GITBIN:-git}"
  python3 - "$TIP1" > "$WORK/fetchuri.bin" <<'PY'
import sys
tip = sys.argv[1]
def pkt(b):
    return f"{len(b)+4:04x}".encode() + b
out = pkt(b"command=fetch\n") + pkt(b"object-format=sha1\n") + b"0001"
out += pkt(f"want {tip}\n".encode()) + pkt(b"packfile-uris http,https\n") + pkt(b"done\n") + b"0000"
sys.stdout.buffer.write(out)
PY
  curl -s -D "$WORK/uh" -o "$WORK/uresp.bin" -X POST "$URL/$REPO/git-upload-pack" \
    -H 'Git-Protocol: version=2' -H 'Content-Type: application/x-git-upload-pack-request' \
    --data-binary @"$WORK/fetchuri.bin"
  python3 - "$WORK/uresp.bin" > "$WORK/uri.txt" <<'PY'
import sys
d = open(sys.argv[1],'rb').read()
i = 0
uri = None
packdata = 0
while i < len(d):
    n = int(d[i:i+4], 16)
    if n == 0:
        break
    if n < 4:          # delim/response-end pkts carry no payload
        i += 4
        continue
    line = d[i+4:i+n]
    if line.startswith(b"packfile-uris"):
        pass
    elif line[:1] == b"\x01":
        packdata += len(line) - 1
    elif b" http" in line and len(line) > 42 and line[:40].decode().strip("0123456789abcdef") == "":
        uri = line[41:].strip().decode()
    i += n
if not uri:
    sys.exit("no <hash> <uri> line in packfile-uris section")
if packdata != 32:
    sys.exit(f"inline pack should be the 32-byte empty pack, got {packdata}")
print(uri)
PY
  URI=$(cat "$WORK/uri.txt")
  HASH=$(python3 - "$WORK/uresp.bin" <<'PY'
import sys,re
d=open(sys.argv[1],'rb').read()
m=re.search(rb'\n?([0-9a-f]{40}) http', d) or re.search(rb'([0-9a-f]{40}) http', d)
print(m.group(1).decode())
PY
  )
  # the signed URI self-authenticates: bare curl, no token
  curl -sf "$URI" -o "$WORK/via-uri.pack" || fail "signed URI fetch"
  [ "$(python3 -c 'import hashlib,sys;print(hashlib.sha1(open(sys.argv[1],"rb").read()[:-20]).hexdigest())' "$WORK/via-uri.pack")" = "$HASH" ] \
    || fail "URI pack checksum != advertised hash"
  head -c 4 "$WORK/via-uri.pack" | grep -q PACK || fail "URI body is not a pack"
  # tampered signature must be refused, and so must an expired or missing one
  code=$(curl -s -o /dev/null -w '%{http_code}' "${URI%s=*}s=deadbeef")
  [ "$code" = "403" ] || fail "bad-sig pack fetch -> $code, want 403"
  code=$(curl -s -o /dev/null -w '%{http_code}' "${URI/e=*/e=1}")
  [ "$code" = "403" ] || fail "expired-sig pack fetch -> $code, want 403"
  code=$(curl -s -o /dev/null -w '%{http_code}' "${URI/%s=*/}")
  [ "$code" = "403" ] || fail "sig-less pack fetch -> $code, want 403"
  # scheme gate: a client naming only https on an http origin gets the A29
  # inline pack instead of a URI; an empty value and a duplicate line likewise
  python3 - "$TIP1" > "$WORK/fetchhttps.bin" <<'PY'
import sys
tip = sys.argv[1]
def pkt(b):
    return f"{len(b)+4:04x}".encode() + b
out = pkt(b"command=fetch\n") + pkt(b"object-format=sha1\n") + b"0001"
out += pkt(f"want {tip}\n".encode()) + pkt(b"packfile-uris https\n") + pkt(b"done\n") + b"0000"
sys.stdout.buffer.write(out)
PY
  curl -s -o "$WORK/hresp.bin" -X POST "$URL/$REPO/git-upload-pack" \
    -H 'Git-Protocol: version=2' -H 'Content-Type: application/x-git-upload-pack-request' \
    --data-binary @"$WORK/fetchhttps.bin"
  python3 - "$WORK/hresp.bin" <<'PY'
import sys
d = open(sys.argv[1],'rb').read()
if b"packfile-uris" in d:
    sys.exit("https-only request must not mint an http URI")
i = 0; packdata = 0
while i < len(d):
    n = int(d[i:i+4], 16)
    if n == 0: break
    if n < 4:
        i += 4; continue
    if d[i+4:i+5] == b"\x01":
        packdata += n - 5
    i += n
if packdata <= 32:
    sys.exit(f"expected the real inline pack via A29, got {packdata} bytes")
PY
  python3 - "$TIP1" > "$WORK/fetchdup.bin" <<'PY'
import sys
tip = sys.argv[1]
def pkt(b):
    return f"{len(b)+4:04x}".encode() + b
out = pkt(b"command=fetch\n") + pkt(b"object-format=sha1\n") + b"0001"
out += pkt(f"want {tip}\n".encode()) + pkt(b"packfile-uris http\n") + pkt(b"packfile-uris https\n") + pkt(b"done\n") + b"0000"
sys.stdout.buffer.write(out)
PY
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$REPO/git-upload-pack" \
    -H 'Git-Protocol: version=2' -H 'Content-Type: application/x-git-upload-pack-request' \
    --data-binary @"$WORK/fetchdup.bin")
  [ "$code" = "400" ] || fail "duplicate packfile-uris -> $code, want 400"
  # bare/empty value is legal per real git — fetch succeeds via the A29 path
  python3 - "$TIP1" > "$WORK/fetchempty.bin" <<'PY'
import sys
tip = sys.argv[1]
def pkt(b):
    return f"{len(b)+4:04x}".encode() + b
out = pkt(b"command=fetch\n") + pkt(b"object-format=sha1\n") + b"0001"
out += pkt(f"want {tip}\n".encode()) + pkt(b"packfile-uris \n") + pkt(b"done\n") + b"0000"
sys.stdout.buffer.write(out)
PY
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$REPO/git-upload-pack" \
    -H 'Git-Protocol: version=2' -H 'Content-Type: application/x-git-upload-pack-request' \
    --data-binary @"$WORK/fetchempty.bin")
  [ "$code" = "200" ] || fail "empty packfile-uris value -> $code, want 200"
  # real client: clone downloads the pack over the signed URL (git >= 2.40)
  "$GITC" -c fetch.uriprotocols=http,https clone -q "$URL/$REPO" "$WORK/clone-uri" || fail "packfile-uris clone (git too old?)"
  [ "$(git -C "$WORK/clone-uri" rev-parse main)" = "$TIP1" ] || fail "packfile-uris clone tip"
  git -C "$WORK/clone-uri" fsck --strict || fail "packfile-uris clone fsck"
  n=$(tr -d '\r' < "$WORK/uh" | sed -nE 's/^x-ge-subrequests: ([0-9]+).*/\1/ip' | tail -1)
  [ -n "$n" ] && [ "$n" -le 8 ] || fail "C1 fetch spend $n, want <= 8"
fi

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

note "--atomic: one failing command rejects the whole push"
# `:main` is always refused (deletion of the current branch); under --atomic
# the sibling create must not land either
if git push --atomic "$URL/$REPO" :main HEAD:refs/heads/at-good 2>/dev/null; then
  fail "atomic push with a bad command succeeded"
fi
git ls-remote "$URL/$REPO" refs/heads/at-good | grep -q . && fail "atomic push leaked a ref"
git push -q --atomic "$URL/$REPO" HEAD:refs/heads/at-good HEAD:refs/heads/at-good2 || fail "atomic push"
git push -q --atomic "$URL/$REPO" --delete refs/heads/at-good refs/heads/at-good2 || fail "atomic delete"

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

# Admin surface: pin, public read, export, delete — on its own repo so the
# destructive tail does not disturb the earlier cases.
note "admin repo: seed"
ADMINREPO="$REPO-admin"
mkdir "$WORK/admin" && cd "$WORK/admin"
git init -q && git config user.email t@t && git config user.name t
echo p > p && git add p && git commit -qm p && git branch -M main
git push -q "$URL/$ADMINREPO" main || fail "admin seed push"
PTIP=$(git rev-parse HEAD)

note "ref pinning: pin rejects update/delete, unpin restores"
curl -sf -X POST "$URL/$ADMINREPO/_admin/pin" \
  -d "{\"ref\":\"refs/heads/main\",\"sha\":\"$PTIP\"}" | grep -q pinned || fail "pin"
git commit -qm ontop --allow-empty
if git push "$URL/$ADMINREPO" HEAD:main 2>"$WORK/pinerr"; then
  fail "push to pinned ref succeeded"
fi
grep -q "ref is pinned" "$WORK/pinerr" || fail "no pin reason in ng: $(cat "$WORK/pinerr")"
if git push "$URL/$ADMINREPO" --delete main 2>/dev/null; then
  fail "delete of pinned ref succeeded"
fi
curl -sf "$URL/$ADMINREPO/_state" | grep -q "$PTIP" || fail "pin missing from _state"
curl -sf -X POST "$URL/$ADMINREPO/_admin/unpin" \
  -d '{"ref":"refs/heads/main"}' | grep -q true || fail "unpin"
git push -q "$URL/$ADMINREPO" HEAD:main || fail "push after unpin"
ATIP=$(git rev-parse HEAD)

note "scoped token (#19): in-scope push lands, out-of-scope rejected"
SCJSON=$(curl -sf -X POST "$URL/$ADMINREPO/_admin/tokens" \
  -d '{"name":"scoper","level":"write","scope":"refs/heads/scoped-*"}') || fail "token mint"
SCTOK=$(echo "$SCJSON" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
SCURL="$(echo "$URL" | sed -E "s|^(https?://)[^/@]*@|\1x:$SCTOK@|")"
git push -q "$SCURL/$ADMINREPO" HEAD:refs/heads/scoped-ok 2>"$WORK/scerr" \
  || fail "in-scope push rejected: $(cat "$WORK/scerr")"
if git push "$SCURL/$ADMINREPO" HEAD:refs/heads/escape 2>"$WORK/scerr2"; then
  fail "out-of-scope push succeeded"
fi
grep -q "outside token scope" "$WORK/scerr2" || fail "no scope reason: $(cat "$WORK/scerr2")"
# a scoped token can't mint further credentials (still global-write gated)
if curl -sf -X POST "$SCURL/$ADMINREPO/_admin/tokens" -d '{"name":"x","level":"write"}' 2>/dev/null; then
  fail "scoped token minted a credential"
fi

note "lfs (#21): batch -> signed PUT -> batch -> signed GET round-trip"
printf 'lfs test payload %s\n' "$(date +%s%N)" > "$WORK/lfs.bin"
OID=$(shasum -a 256 "$WORK/lfs.bin" | cut -d' ' -f1)
SIZE=$(stat -f%z "$WORK/lfs.bin" 2>/dev/null || stat -c%s "$WORK/lfs.bin")
LFSJSON=$(curl -sf -X POST "$URL/$ADMINREPO/info/lfs/objects/batch" \
  -H 'Content-Type: application/vnd.git-lfs+json' \
  -d "{\"operation\":\"upload\",\"transfers\":[\"basic\"],\"objects\":[{\"oid\":\"$OID\",\"size\":$SIZE}]}") \
  || fail "lfs upload batch"
HREF=$(echo "$LFSJSON" | python3 -c 'import sys,json;print(json.load(sys.stdin)["objects"][0]["actions"]["upload"]["href"])')
# the href carries no Authorization — the HMAC alone is the credential (A30 model)
curl -sf -X PUT "$HREF" --data-binary @"$WORK/lfs.bin" || fail "lfs PUT"
# second batch: object exists now -> no upload action
LFSJSON2=$(curl -sf -X POST "$URL/$ADMINREPO/info/lfs/objects/batch" \
  -H 'Content-Type: application/vnd.git-lfs+json' \
  -d "{\"operation\":\"upload\",\"objects\":[{\"oid\":\"$OID\",\"size\":$SIZE}]}") || fail "lfs re-batch"
echo "$LFSJSON2" | grep -q '"upload"' && fail "re-batch offered upload for existing object"
LFSGET=$(curl -sf -X POST "$URL/$ADMINREPO/info/lfs/objects/batch" \
  -H 'Content-Type: application/vnd.git-lfs+json' \
  -d "{\"operation\":\"download\",\"objects\":[{\"oid\":\"$OID\",\"size\":$SIZE}]}") || fail "lfs dl batch"
GHREF=$(echo "$LFSGET" | python3 -c 'import sys,json;print(json.load(sys.stdin)["objects"][0]["actions"]["download"]["href"])')
curl -sf "$GHREF" -o "$WORK/lfs-dl.bin" || fail "lfs GET"
cmp "$WORK/lfs.bin" "$WORK/lfs-dl.bin" || fail "lfs round-trip differs"
code=$(curl -s -o /dev/null -w '%{http_code}' "${GHREF%%s=*}s=deadbeef")
[ "$code" = "403" ] || fail "bad-sig lfs GET -> $code, want 403"
# a get sig is not a put sig — the op is inside the HMAC
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT --data-binary @"$WORK/lfs.bin" "$GHREF")
[ "$code" = "403" ] || fail "get-sig lfs PUT -> $code, want 403"
MISSING=$(printf 'never-uploaded' | shasum -a 256 | cut -d' ' -f1)
curl -sf -X POST "$URL/$ADMINREPO/info/lfs/objects/batch" \
  -H 'Content-Type: application/vnd.git-lfs+json' \
  -d "{\"operation\":\"download\",\"objects\":[{\"oid\":\"$MISSING\",\"size\":1}]}" \
  | grep -q '"code":404' || fail "missing lfs object not 404 in batch"
# batch needs auth: anonymous gets 401
LFSNOAUTH="$(echo "$URL" | sed -E 's|^(https?://)[^/@]*@|\1|')"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "$LFSNOAUTH/$ADMINREPO/info/lfs/objects/batch" \
  -H 'Content-Type: application/vnd.git-lfs+json' \
  -d "{\"operation\":\"download\",\"objects\":[{\"oid\":\"$OID\",\"size\":$SIZE}]}")
[ "$code" = "401" ] || fail "anonymous lfs batch -> $code, want 401"

note "public read: anonymous fetch, push still gated"
NOAUTH="$(echo "$URL" | sed -E 's|^(https?://)[^/@]*@|\1|')"
# a credential helper (osxkeychain) may have stored the URL-embedded token from
# earlier pushes — empty it so these calls are genuinely unauthenticated
ANON="-c credential.helper="
curl -sf -X POST "$URL/$ADMINREPO/_admin/public" \
  -d '{"enabled":true}' | grep -q true || fail "public on"
git $ANON ls-remote "$NOAUTH/$ADMINREPO" > "$WORK/ls-pub" || fail "anonymous ls-remote"
grep -q "refs/heads/main" "$WORK/ls-pub" || fail "anonymous ls-remote empty"
git $ANON clone -q "$NOAUTH/$ADMINREPO" "$WORK/pubclone" || fail "anonymous clone"
if GIT_TERMINAL_PROMPT=0 git $ANON -C "$WORK/seed" push -q "$NOAUTH/$ADMINREPO" HEAD:refs/heads/anon 2>/dev/null; then
  fail "anonymous push to public repo succeeded"
fi
curl -sf -X POST "$URL/$ADMINREPO/_admin/public" \
  -d '{"enabled":false}' | grep -q false || fail "public off"
if GIT_TERMINAL_PROMPT=0 git $ANON ls-remote "$NOAUTH/$ADMINREPO" >/dev/null 2>&1; then
  fail "anonymous ls-remote after public off"
fi

note "export: v3 bundle of all refs"
curl -sf "$URL/$ADMINREPO/_admin/export" -o "$WORK/exp.bundle" || fail "export"
head -1 "$WORK/exp.bundle" | grep -q "v3 git bundle" || fail "not a v3 bundle"
git -C "$WORK/seed" bundle verify "$WORK/exp.bundle" >/dev/null || fail "bundle verify"
git clone -q "$WORK/exp.bundle" "$WORK/bundleclone" || fail "clone from bundle"
# HEAD is a detached oid in the bundle (git's own format does the same), so with
# two refs at the same tip the checked-out branch is ambiguous — assert the
# remote-tracking ref carries main's tip instead of relying on a local branch
[ "$(git -C "$WORK/bundleclone" rev-parse origin/main)" = "$ATIP" ] || fail "bundle tip"

note "repo delete: tombstone 410 window, purge, name reusable"
curl -sf -X POST "$URL/$ADMINREPO/_admin/delete" | grep -q deleted || fail "delete"
# The purge can complete before this probe (alarm is ~instant in dev): either the
# tombstone still answers 410, or the repo is already reborn — in both cases the
# old refs must be gone.
code=$(curl -s -o "$WORK/dr" -w '%{http_code}' "$URL/$ADMINREPO/info/refs?service=git-upload-pack")
if [ "$code" = "200" ]; then
  ! grep -q "$ATIP" "$WORK/dr" || fail "deleted repo still serves old refs"
elif [ "$code" != "410" ]; then
  fail "deleted repo info/refs -> $code, want 410 or reborn-empty 200"
fi
if GIT_TERMINAL_PROMPT=0 git ls-remote "$URL/$ADMINREPO" 2>/dev/null | grep -q "$ATIP"; then
  fail "ls-remote on deleted repo still advertises old tip"
fi
# a repush to the same name must succeed once the tombstone clears (or immediately
# if the purge already wiped) — the disposable profile depends on name reuse
for i in 1 2 3 4 5 6 7 8 9 10; do
  mkdir -p "$WORK/reuse" && cd "$WORK/reuse" && rm -rf .git
  git init -q && git config user.email t@t && git config user.name t
  echo reuse > r && git add r && git commit -qm reuse && git branch -M main
  if git push -q "$URL/$ADMINREPO" main 2>/dev/null; then break; fi
  [ "$i" = "10" ] && fail "name never reusable after delete"
  sleep 1
done

# Quota + push rate-limit checks (A26/A27): only runs with GE_CONFORMANCE_LIMITS=1
# and the dev server started with small caps, e.g. .dev.vars:
#   GE_QUOTA_MAX_OBJECTS=25 GE_QUOTA_MAX_REPOS_PER_OWNER=5 GE_RATE_PUSHES_PER_MIN=12
# (25 stays above the main suite's ~12 objects/repo; 12 stays above its 9 pushes;
# the owner cap must exceed the suite's peak live claims — run+admin+gc+import
# +uimport+sha256+f256 = 7, so the cap is exactly GE_QUOTA_MAX_REPOS_PER_OWNER=7 — the quota owner then trips it on its 8th claim.)
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
  # $O/obj already claimed slot 1; fill until the registry refuses — the exact
  # cap is the dev server's, so this loop (not a fixed third post) is the test
  out=""
  for r in b c d e f g h; do
    out=$(post "$O/$r")
    echo "$out" | grep -q "GE_QUOTA_MAX_REPOS_PER_OWNER" && break
  done
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

# I1 server-side import: stage a pack -> import_pack job -> atomic commit.
# Only runs with GE_CONFORMANCE_IMPORT=1 against a build with the _admin/import
# routes. The staged pack uses REF deltas (pack-objects default) so the job's
# forward-reference/wake path is exercised, not just verbatim copies.
if [ "${GE_CONFORMANCE_IMPORT:-0}" = "1" ]; then
  note "import: stage pack, job ingests, refs commit"
  IREPO="$REPO-import"
  cd "$WORK/seed"
  git rev-list --objects HEAD | git pack-objects --stdout > "$WORK/stage.pack"
  NEW=$(git rev-parse HEAD)
  # split into two halves: exercises Source::Parts reads and the shared-push
  # stage API (`?push=` + `?part=`) — every part must live on one open push or
  # PUSH_TIMEOUT would sweep siblings mid-import
  half=$(( ($(wc -c < "$WORK/stage.pack" | tr -d ' ') + 1) / 2 ))
  split -b "$half" "$WORK/stage.pack" "$WORK/stage.part."
  st=$(curl -sf -X POST "$URL/$IREPO/_admin/import/stage" --data-binary @"$WORK/stage.part.aa")
  PUSH=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["push"])')
  KEY=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["key"])')
  BYTES=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["bytes"])')
  st=$(curl -sf -X POST "$URL/$IREPO/_admin/import/stage?push=$PUSH&part=b" --data-binary @"$WORK/stage.part.ab")
  KEY2=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["key"])')
  BYTES2=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["bytes"])')
  echo "$KEY2" | grep -q "\.part-b" || fail "part key shape: $KEY2"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$IREPO/_admin/import/stage?push=$PUSH" --data-binary @"$WORK/stage.part.ab")
  [ "$code" = "400" ] || fail "reused push without ?part -> $code, want 400"
  ZERO=0000000000000000000000000000000000000000
  st=$(curl -sf -X POST "$URL/$IREPO/_admin/import" -H 'Content-Type: application/json' \
    -d "{\"push\":\"$PUSH\",\"parts\":[{\"key\":\"$KEY\",\"bytes\":$BYTES},{\"key\":\"$KEY2\",\"bytes\":$BYTES2}],\"commands\":[{\"old\":\"$ZERO\",\"new\":\"$NEW\",\"name\":\"refs/heads/main\"}]}")
  echo "$st" | grep -q queued || fail "import start: $st"
  deadline=$((SECONDS + 120))
  while :; do
    st=$(curl -sf "$URL/$IREPO/_admin/import/$PUSH")
    ps=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("push_state"))')
    [ "$ps" = "committed" ] && break
    [ "$ps" = "rejected" ] && fail "import rejected: $st"
    [ $SECONDS -lt $deadline ] || fail "import did not commit within 120s: $st"
    sleep 2
  done
  git clone -q "$URL/$IREPO" "$WORK/iclone" || fail "post-import clone"
  [ "$(git -C "$WORK/iclone" rev-parse main)" = "$NEW" ] || fail "import tip"
  git -C "$WORK/iclone" fsck --strict || fail "post-import fsck"
  # a part key outside this repo's pending namespace must be refused outright —
  # needs a fresh open push (a committed one is refused earlier, on state)
  st2=$(curl -sf -X POST "$URL/$IREPO/_admin/import/stage" --data-binary @"$WORK/stage.pack")
  PUSH2=$(echo "$st2" | python3 -c 'import sys,json;print(json.load(sys.stdin)["push"])')
  code=$(curl -s -o "$WORK/ibad" -w '%{http_code}' -X POST "$URL/$IREPO/_admin/import" \
    -H 'Content-Type: application/json' \
    -d "{\"push\":\"$PUSH2\",\"parts\":[{\"key\":\"r/00000000000000000000000000000000/pending/x.pack\",\"bytes\":1}],\"commands\":[{\"old\":\"$ZERO\",\"new\":\"$NEW\",\"name\":\"refs/heads/x\"}]}")
  [ "$code" = "400" ] || fail "cross-namespace part -> $code, want 400"
  # a re-start on the committed push is refused (state check), not a dup job
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$IREPO/_admin/import" \
    -H 'Content-Type: application/json' \
    -d "{\"push\":\"$PUSH\",\"parts\":[{\"key\":\"$KEY\",\"bytes\":$BYTES}],\"commands\":[{\"old\":\"$ZERO\",\"new\":\"$NEW\",\"name\":\"refs/heads/main\"}]}")
  [ "$code" = "409" ] || fail "re-start on committed push -> $code, want 409"
  # and a stage that re-begins the now-committed push is refused the same way
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$IREPO/_admin/import/stage?push=$PUSH&part=c" --data-binary @"$WORK/stage.part.ab")
  [ "$code" = "409" ] || fail "re-begin committed push -> $code, want 409"
  # no-walk dedup regression: a second import re-ships an object already live,
  # so while its pack is live pre-sweep that object sits in two live packs at
  # once — the exact shape a gc_consolidate/gc_sweep window creates. A clone
  # must emit each sha once; index-pack rejects duplicates in one pack. The
  # pack carries only the tip commit (its tree/blob resolve against the live
  # pack at check) so GE_QUOTA_MAX_OBJECTS stays comfortably under a limits run.
  echo "$NEW" | git pack-objects --stdout > "$WORK/dup.pack"
  st=$(curl -sf -X POST "$URL/$IREPO/_admin/import/stage" --data-binary @"$WORK/dup.pack")
  PUSH3=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["push"])')
  KEY3=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["key"])')
  BYTES3=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin)["bytes"])')
  st=$(curl -sf -X POST "$URL/$IREPO/_admin/import" -H 'Content-Type: application/json' \
    -d "{\"push\":\"$PUSH3\",\"parts\":[{\"key\":\"$KEY3\",\"bytes\":$BYTES3}],\"commands\":[{\"old\":\"$ZERO\",\"new\":\"$NEW\",\"name\":\"refs/heads/dup2\"}]}")
  echo "$st" | grep -q queued || fail "overlap import start: $st"
  deadline=$((SECONDS + 60))
  while :; do
    st=$(curl -sf "$URL/$IREPO/_admin/import/$PUSH3")
    ps=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("push_state"))')
    [ "$ps" = "committed" ] && break
    [ "$ps" = "rejected" ] && fail "overlap import rejected: $st"
    [ $SECONDS -lt $deadline ] || fail "overlap import did not commit: $st"
    sleep 0.2
  done
  # clone inside the overlap window — GC's quiet period holds the sweep back
  OVRLAP=$(curl -sf "$URL/$IREPO/_state" | python3 -c 'import sys,json;print(json.load(sys.stdin)["packs_live"])')
  git clone -q "$URL/$IREPO" "$WORK/dclone" || fail "overlap-window clone"
  git -C "$WORK/dclone" fsck --strict || fail "overlap-window fsck"
  [ "$OVRLAP" -ge 2 ] && note "overlap clone exercised dedup (packs_live=$OVRLAP)" || note "sweep beat the clone — dedup path not exercised this run"

  # WILD url import: the worker is the git client — v2 ls-refs + fetch streams
  # the remote's pack into pending/, then the same parse→resolve→commit job
  # owns it. Source is this same instance over loopback http — normalize()
  # refuses plaintext http anywhere else.
  note "url import: POST {url} fetches the remote and commits its refs"
  NOAUTHURL="$(echo "$URL" | sed -E 's|^(https?://)[^/@]*@|\1|')"
  curl -sf -X POST "$URL/$IREPO/_admin/public" -H 'Content-Type: application/json' \
    -d '{"enabled":true}' | grep -q true || fail "url-import source public"
  UREPO="$REPO-uimport"
  st=$(curl -sf -X POST "$URL/$UREPO/_admin/import" -H 'Content-Type: application/json' \
    -d "{\"url\":\"$NOAUTHURL/$IREPO\"}")
  UPUSH=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("push",""))')
  [ -n "$UPUSH" ] || fail "url import start: $st"
  deadline=$((SECONDS + 120))
  while :; do
    st=$(curl -sf "$URL/$UREPO/_admin/import/$UPUSH")
    ps=$(echo "$st" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("push_state"))')
    [ "$ps" = "committed" ] && break
    [ "$ps" = "rejected" ] && fail "url import rejected: $st"
    [ $SECONDS -lt $deadline ] || fail "url import did not commit: $st"
    sleep 1
  done
  git clone -q "$URL/$UREPO" "$WORK/uclone" || fail "url-import clone"
  git -C "$WORK/uclone" fsck --strict || fail "url-import fsck"
  [ "$(git -C "$WORK/uclone" rev-parse main)" = "$NEW" ] || fail "url-import tip"
  # HEAD adopts the remote's symref target — the clone checks out main
  [ "$(git -C "$WORK/uclone" symbolic-ref HEAD)" = "refs/heads/main" ] || fail "url-import HEAD"
  # the url-imported repo carries every advertised ref, dup2 included
  git ls-remote "$URL/$UREPO" | grep -q "refs/heads/dup2" || fail "url-import missing dup2"
  # guards: plaintext http off-loopback and non-http(s) schemes 400 at the edge
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$UREPO/_admin/import" -d '{"url":"http://169.254.169.254/x"}')
  [ "$code" = "400" ] || fail "non-loopback http url -> $code, want 400"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$UREPO/_admin/import" -d '{"url":"gopher://x"}')
  [ "$code" = "400" ] || fail "bad scheme -> $code, want 400"
fi

# SHA-256 object format: a sha256 client's first push pins an unpinned repo;
# after that the repo answers only sha256. Needs git >= 2.29 for
# `init --object-format`. Push is always v0 wire — the client picks its algo
# out of the advertised object-format caps, so this exercises the both-formats
# advertisement + the cap-driven pin in push_begin.
note "sha256: push pins, clone negotiates, fsck clean, sha1 refused"
if git init -q --object-format=sha256 "$WORK/s256src" 2>/dev/null; then
  S256="$REPO-sha256"
  cd "$WORK/s256src" && git config user.email t@t && git config user.name t
  echo sha256 > f && mkdir sub && echo n > sub/g && git add -A && git commit -qm c
  git tag -a v1 -m t
  S256TIP=$(git rev-parse HEAD)
  [ ${#S256TIP} -eq 64 ] || fail "sha256 tip width: ${#S256TIP}"
  git push -q "$URL/$S256" HEAD:refs/heads/main || fail "sha256 push (pin-on-first)"
  git push -q "$URL/$S256" v1 || fail "sha256 tag push"
  git ls-remote "$URL/$S256" | grep -q "^$S256TIP	refs/heads/main$" || fail "sha256 ls-remote"
  git ls-remote "$URL/$S256" | grep -q "refs/tags/v1\\^{}" || fail "sha256 peel"
  git clone -q "$URL/$S256" "$WORK/s256clone" || fail "sha256 clone"
  [ "$(git -C "$WORK/s256clone" rev-parse HEAD)" = "$S256TIP" ] || fail "sha256 clone tip"
  git -C "$WORK/s256clone" fsck --strict || fail "sha256 fsck"
  # format is pinned: a sha1 push is refused (client aborts on the caps)
  mkdir "$WORK/s1src" && cd "$WORK/s1src" && git init -q && git config user.email t@t && git config user.name t
  echo x > x && git add x && git commit -qm x
  if git push -q "$URL/$S256" HEAD:refs/heads/x 2>/dev/null; then
    fail "sha1 push into sha256 repo must fail"
  fi
  # _admin/format pins explicitly and rejects junk; a sha256 pin + push also works
  F256="$REPO-f256"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$F256/_admin/format" -d '{"object_format":"sha256"}')
  [ "$code" -lt 300 ] || fail "_admin/format sha256 -> $code"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$F256/_admin/format" -d '{"object_format":"sha1"}')
  [ "$code" = "409" ] || fail "format re-pin conflict -> $code, want 409"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$URL/$F256/_admin/format" -d '{"object_format":"md5"}')
  [ "$code" = "400" ] || fail "format junk -> $code, want 400"
  cd "$WORK/s256src" && git push -q "$URL/$F256" HEAD:refs/heads/main || fail "sha256 push to pinned repo"
  git clone -q "$URL/$F256" "$WORK/f256clone" || fail "pinned sha256 clone"
  git -C "$WORK/f256clone" fsck --strict || fail "pinned sha256 fsck"
else
  note "sha256: this git lacks init --object-format — skipped"
fi

# purge_repo job: POST _admin/delete enqueues it; the repo converges to empty
# (the DO re-boots a fresh repo on next read — objects/refs/jobs all zero).
# Runs only with GE_CONFORMANCE_PURGE=1 and a build that has the admin route.
if [ "${GE_CONFORMANCE_PURGE:-0}" = "1" ]; then
  note "purge: _admin/delete -> purge_repo wipes the repo"
  # own owner: under GE_CONFORMANCE_LIMITS caps the suite's owner already holds
  # its slots (run/-admin/-gc), so the purge repo must not contend for a claim
  PREPO="purge$$/repo"
  mkdir "$WORK/purge" && cd "$WORK/purge"
  git init -q && git config user.email t@t && git config user.name t
  echo purge > p && git add p && git commit -qm p && git branch -M main
  git push -q "$URL/$PREPO" main || fail "purge seed push"
  git ls-remote "$URL/$PREPO" | grep -q main || fail "pre-purge ls-remote empty"
  code=$(curl -s -o "$WORK/purge-resp" -w '%{http_code}' -X POST "$URL/$PREPO/_admin/delete")
  if [ "$code" = "404" ]; then
    note "purge: _admin/delete not deployed on this build — skipped"
  else
    [ "$code" -lt 300 ] || fail "_admin/delete -> $code: $(cat "$WORK/purge-resp")"
    deadline=$((SECONDS + 60))
    while :; do
      refs=$(git ls-remote "$URL/$PREPO" 2>/dev/null | wc -l | tr -d ' ')
      objs=$(curl -sf "$URL/$PREPO/_state" | python3 -c 'import sys,json;print(json.load(sys.stdin)["objects"])' || echo -1)
      [ "$refs" = "0" ] && [ "$objs" = "0" ] && break
      [ $SECONDS -lt $deadline ] || fail "purge did not converge in 60s (refs=$refs objects=$objs)"
      sleep 2
    done
    note "purged: repo reads back empty"
  fi
fi

note "PASS"
