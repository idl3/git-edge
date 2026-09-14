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

## Audit pass (A19)

A line-by-line review of all ~4,900 lines found eight issues, all fixed and verified:

- **Edge buffered entire fetch packs** — `upload_pack` reassembled the DO's streaming
  response into memory; a large clone would OOM the worker. Now streams through.
- **Body read unbounded** — `req.bytes()` held the whole body before the 1 MiB check;
  chunked bodies now stream through `BodyReader` (which also fixed gzipped fetch bodies,
  previously fed raw to the parser).
- **No pack-size ceiling** — pushes are now capped at 2 GiB compressed, 64 open pushes.
- **Ref names** — must be full refnames under `refs/`; `ok`/`ng` lines echo names with
  non-graphic bytes replaced, closing a response-injection path (verified: a name
  containing `\n` echoes as `?`).
- **Error leakage** — `Internal`/`Storage` detail (R2 keys, SQL errors) no longer reaches
  clients; full errors go to the worker log.
- **Constant-time token compare**, and a `reflog(at)` index for the janitor scan.

Reviewed and deliberately left: `include-tag` is parsed but unimplemented (tags still
arrive via explicit wants); only `blob:none`/`blob:limit` filters are supported — others
fail cleanly rather than silently misbehave.

Verified sound: all SQL parameterized; DO atomicity via the platform output gate;
the ≥5 MiB multipart rule is honored (`checkpoint` only fires at `MIN_PART`); the 2.5
connectivity check is transitive; job retry/backoff/repair and GC idempotence hold.

## Known production gaps (honest list)

- GC is exercised end to end: a force-push orphaning a pack triggers the full
  janitor → mark → consolidate → sweep chain on real alarms; the dead pack's rows and
  objects are reclaimed (`objects 10 → 3`), and a post-sweep clone passes `fsck --strict`.
  The 10-minute quiet / 1-hour grace windows are env-tunable for testing (A18).
- Two alarm-era bugs only surfaced when the chain first fired: `set_alarm` takes an offset
  from now, not an epoch timestamp (jobs landed ~56 years out — A16), and DO SQLite BLOBs
  deserialize as byte arrays, so `Vec<u8>` DTO fields need `serde_bytes` (A16).
- An all-dead candidate pack skips the repack build entirely and sweeps directly (A17).
- Local workerd's R2 simulates multipart uploads; abandoned-MPU behaviour on real R2 still
  needs a deployment check.
- Subrequest ceilings are guarded by `ReqBudget`, not verified against plan limits.
- No delta repack: packs stored are full-object; fetch sends what it has. Bandwidth-wise
  this is the documented trade of section 12.
- `report-status-v2`, atomic pushes, push-options: not advertised, by design (A12).

## Adversarial round 2 (findings/audit/*: security, concurrency, protocol, perf-scale)

Four sub-agent audits plus live git-2.54 probing. Every confirmed defect was fixed and
re-verified; the audit reports live in `findings/audit/`.

**Fixed and verified live:**

- Receive header restartable-parse + flush-only push (an up-to-date push is 200 + flush,
  not a protocol error).
- Fetch response grammar: `acknowledgments`/`ready` only when the client sent haves.
- Full v2 shallow matrix: `deepen`, `deepen-relative`, `deepen-since`, `deepen-not`
  (ref names resolved server-side), `shallow`, `unshallow`, plus `want-ref` and
  `include-tag`. Live-verified: `--depth`/`--deepen`/`--shallow-since`/
  `--shallow-exclude`/`--unshallow`/`--filter=blob:none`, all `fsck --strict`-clean.
- One-shot band-3 `ERR` then stream end (was an infinite retry loop).
- Torn-read guard: `plan_reads` asserts index-vs-bitmap consistency so a mid-fetch
  `gc_sweep` fails instead of corrupting the pack.
- Strict ingest parse for commits/tags; bounded fixpoint for forward `REF_DELTA`.
- Byte bounds everywhere: delta-chain 64 MiB, read batch 48 MiB, in-loop `MAX_MEM`,
  1M entries, 65 536 refs, 32 ref-prefix args, O(1) `SendSet::mark`.
- 7.4 commit-region prefetch (`PREFETCH` = 2 MiB each side; `objects(pack_id,offset)`
  index) — deep-history fetches no longer pay one subrequest per level.
- Janitor propagates R2 delete failures; `pushes(state)`/`packs(state)` indexed;
  `/_do/push/abort` closes post-begin failures; fetch/error router arms rearm alarms;
  GC slices heartbeat so `repair` can't strand live jobs.
- `.`/`..` route segments rejected; case-insensitive auth scheme; `x-ge-subrequests`
  on fetch responses; sanitized client-derived strings in `ERR`/`ng`/`unpack` lines.

**Still open (needs real Cloudflare deploy or a design decision):**

- R2 abandoned-MPU semantics and plan-limit enforcement — workerd simulates both;
  `wrangler deploy` against a real account is the only arbiter.
- `packfile-uris`, `sideband-all`, `no-done`, `object-format=sha256`, filters beyond
  `blob:none`/`blob:limit`, v0/v1 upload-pack negotiation — deliberately unadvertised;
  clients get a clean protocol error, not silent misbehavior.
