# Implementation pass — a working edge git server (2026-09-14)

The contract is now code. `server/` is a Rust crate compiled to wasm32, deployed as a
Cloudflare Worker with one SQLite Durable Object per repo and R2 for pack bytes. It is not
a sketch: `tests/conformance/run.sh` exercises it end to end with a real git client, and
this repository itself has been pushed into the server and cloned back with a clean
`git fsck --strict`.

## What the conformance suite proves

- v2 `ls-refs` on empty and populated repositories; v0/v1 receive-pack advertisement.
- Initial push; incremental push where git sends a thin pack whose deltas resolve against
  objects already in R2 (two-pass ingest: stream to `pending/`, resolve, normalize to a
  full-object pack, index, commit refs by CAS).
- A 6 MiB binary through the multipart writer, byte-identical after clone.
- Branch create and delete; annotated-tag-style ref push; delete-only push with no PACK.
- `git clone` — v2 `fetch` command parsed, send-set walked from the SQLite index, pack
  generated and streamed over sideband-64k with exact count and trailer.
- Incremental `git fetch` — have/want send-set against stored objects.
- Ref-update CAS rejects a stale `old` (git reports `failed to update ref`-class `ng`).
- The post-header error arm: a malformed pack returns HTTP 200 carrying
  `unpack <err>` + `ng <ref> unpack failed`, never a bare 5xx git would discard.
- `Content-Encoding: gzip` request bodies through `DecompressionStream`.

## What only a real client could have caught

Three of the four bugs found in this pass were invisible to the proof reviews because the
proofs specified *intent* and the wire required a specific *byte sequence*:

1. Report-status needed a flush packet inside the sideband stream; without it git applies
   the push and then dies "remote end hung up unexpectedly". The contract now says so (A11).
2. `report-status-v2` advertises an option-line grammar we don't emit; stopped advertising
   it (A12).
3. The objects-index reader query must key its result map on object sha (A13).

The fourth was dependency drift: `wasm-streams` had to match `worker`'s version or
wasm-bindgen rejected the bundle (A14), plus ~15 real signature fixes across the pinned
gitoxide and workers-rs APIs — exactly the cross-proof drift the reviewers predicted.

## Where this sits against the 56-idea study

The foundation spine (ideas 1-14) is now demonstrated, not just argued: a repo lives in a
DO+R2 with no GitHub anywhere in the path. The edge tier (forks-as-refs, sessions, diffs,
reviews, GC under real load) keeps its proof status — the designs compile against the same
contract the server now implements, but they are not yet endpoints.

## Known production gaps (honest list)

- GC is written and wired to alarms but has not reclaimed a real pack under test yet.
- Local workerd's R2 simulates multipart uploads; abandoned-MPU behaviour on real R2 still
  needs a deployment check.
- Subrequest ceilings are guarded by `ReqBudget`, not verified against plan limits.
- No delta repack: packs stored are full-object; fetch sends what it has. Bandwidth-wise
  this is the documented trade of section 12.
- `report-status-v2`, atomic pushes, push-options: not advertised, by design (A12).
