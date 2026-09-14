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

## Round 3 — adversarial review of PR #3 (second pass)

Parallel reviewers attacked the round-2 diff itself. Findings fixed and verified:

- **Nested forward REF_DELTA**: a REF_DELTA whose base is later in the pack *and*
  inside another delta chain now defers via `Base::Await`/`Step::Await` — waiters are
  keyed by base id and woken exactly once (O(n), not O(k^2) fixpoint). Verified live:
  pack `delta->deferred->delta->base` pushed, cloned byte-exact, `fsck --strict` clean.
  Delta cycles reject cleanly with `unpack missing base`.
- **idx vs offset invariant**: deferred entries append out of physical order, so
  `Index::entries_of` now sorts by `offset`; `commits_in_range` got
  `ORDER BY o.offset`; `read_entries` already sorts internally. `idx` remains the
  bitmap position token only.
- **Janitor poisoned-delete**: per-key failure handling — one bad R2 delete no longer
  wedges the phase; `swept_at` only after a confirmed delete; deterministic
  `ORDER BY` so the same row can't sit at the head forever; failures surface as the
  slice's recorded error.
- **Repair resurrection**: stranded `running` rows now consume an attempt and dead-end
  at the threshold instead of retrying at every boot; dead maintenance jobs keep their
  `last_error` in `meta` (`dead.<kind>`) before the row is deleted.
- **Schema migration**: `PRAGMA table_info`-checked `ALTER TABLE` for late columns
  (`pushes.swept_at`, `packs.dead_at`, `jobs.started_at`) on DOs booted under older DDL.
- **Push/commit liveness**: `/_do/push/commit` failures also abort the open push
  (idempotent); the `ingesting`->`live` pack transition is bound to `push_id` so one
  push's commit can't promote another push's pack; `begin_build` retry dead-marks the
  previous `gc.new_pack` row (was an orphan the janitor deliberately never touches).
- **Protocol**: `Git-Protocol: version=1` now emits the `version 1` packet;
  `deepen-not` resolves unqualified names via git's ref search order
  (`mid` -> `refs/tags/mid`); `info/refs`/`_state` errors are plain text per contract.
- **Auth**: empty presented tokens never match; `GE_WRITE_TOKEN` unset no longer 500s
  read-only deployments.

## Round 4 — load verification on merged main (workerd, local)

| Workload | Result |
|---|---|
| Push 20k-commit linear history (60k objects) | 8.8 s |
| Clone 20k commits | 4.3 s — `x-ge-subrequests: 5/9000` (prefetch + contiguous regions) |
| Clone `--depth 15000` of 20k | 1.4 s, 1 shallow boundary, fsck clean |
| Push 200 MiB / 10,207 objects (incl. 10k-file tree) | 21 s |
| Clone same | 6.3 s, byte-identical, fsck clean |
| 4× parallel 20k clones | 6.9 s total |
| 5× parallel pushes, distinct refs | 0.38 s, all committed |
| 5× parallel CAS-divergent pushes | all correctly rejected |
| GC on 60k-object repo + concurrent clone | mark→sweep < 4 s; clone clean mid-sweep |
| 100 MiB blob push | clean `object too large (16 MiB max)` rejection |
| Nested forward REF_DELTA pack (delta→deferred→delta→base) | resolved, byte-exact |
| REF_DELTA cycle | clean `unpack missing base`, no hang |
| `Git-Protocol: version=1` | `version 1` packet emitted |
| `deepen-not mid` (unqualified) | resolves via ref search order; cuts correctly |
| Multi-round fetch (partial ACKs) | negotiates, correct pack |

No worker errors/panics in the log across all of the above.

## Round 5 — post-merge full-surface audit (PR #4)

Three reviewers swept merged main (scale/limits, jobs/reliability, protocol).
Findings fixed, verified, and merged as `788f192`:

- **GC data-loss race**: a duplicate or reclaimed `gc_mark` slice could read a
  bitmap, set a bit, and overwrite a newer bitmap — losing live-object marks and
  sweeping reachable objects. `commit_ids` now re-reads and OR-merges the stored
  bitmap inside the same sync span.
- **Job fencing**: outcome writes fence on a stable `lease` token (not
  `started_at` — `heartbeat` rewrites it, which would break every heartbeat).
  GC slices heartbeat during long consolidation so `repair` can't strand them;
  a resurrection cap stops deterministically-failing GC chains restarting
  forever; `alarm` swallows dispatch errors per contract 4.4; `rearm` runs on
  fetch error paths.
- **Unbounded memory under adversarial packs**: client-declared compressed
  entry size is capped before reads (padded zlib → ~2 GiB `read_range` alloc);
  nested `REF_DELTA` recursion shares a live-bytes counter + depth cap;
  `IndexSink.links` enforced at extend-time (giant tree → link-vector spike);
  fetch tree/base expansion shares `Rc` bases (~260 MB spike removed); pending
  MPU aborted on any post-create error in `stream_to_pending`/`finish`.
- **GC consolidation**: `build` flushes per read-chunk instead of buffering a
  ~1.4 GiB batch; `ORDER BY idx` makes replay byte-identical so staged index
  rows can't carry wrong offsets.
- **Shallow-fetch semantics**: walk descends *through* client-shallow commits in
  `deepen`/`deepen-since`/`deepen-not` modes (depth-1 clone can deepen below its
  boundary — verified); `unshallow` only when parents are now delivered;
  client-held commits never emitted as `shallow`; relative deepen counts depth
  beneath the old boundary; `deepen`+`deepen-since`/`deepen-not` rejected;
  `deepen-not` peels annotated tags and errors on unresolvable names;
  `acknowledgments` omitted when `done` was sent (contract rule 3);
  `x-ge-subrequests` propagated through the edge response.
- Verified live: cyclic `REF_DELTA` → `unpack missing base` in ~60 ms, no hang;
  conformance `== PASS`; full shallow battery green.

**Genuinely left (unchanged from round 3):** real-deploy items only — R2
abandoned-MPU semantics and plan-limit enforcement require a real Cloudflare
account; `packfile-uris`/`sideband-all`/`no-done`/sha256/non-blob filters/v0-v1
negotiation remain deliberately unadvertised with clean protocol errors.

## Round 6 — real Cloudflare deploy (git-edge.grain.workers.dev)

Deployed 2026-09-15. Measured on the real platform:

| Check | Result |
|---|---|
| Full conformance suite vs production | `== PASS` |
| Push 12k commits / clone back | 11.3 s / 4.3 s, `fsck --strict` clean |
| Push ~95 MiB pack (1 MiB objects) | 35.5 s, committed |
| Push ~130 MiB pack | **HTTP 413 — the ~100 MB zone body cap is real** |
| Push with one 120 MiB object | 413 — our `object too large` limit surfaces mid-upload as a client-side hangup (should return report-status `ng` instead) |
| Subrequest model | Paid plan = 10,000/invocation; our 9,000 budget fits. Free = 1,000 internal-service calls + 10 ms CPU — Paid-only as designed |
| R2 multipart on real R2 | works — pushes/clone verified |
| DO alarms on real infra | jobs schedule correctly (janitor 15-min cadence); firing is platform-guaranteed |

**New real-deploy findings:**
- Push bodies are capped at ~100 MB by the zone before our code runs. Workaround:
  push in stages (`git push <sha>:main` then `git push main` — incremental packs
  only carry new objects) or use LFS (not yet implemented).
- Mid-upload rejection (`object too large`) reports as "remote hung up" on the
  client. A valid `ng` report-status response would surface the real reason.

## Round 7 — pass-through ingest, per-repo tokens, metrics, error UX

Follow-up feature round, verified on local workerd and re-verified on
production (`git-edge.grain.workers.dev`):

- **Pass-through ingest for large full objects**: blobs over 16 MiB no longer
  inflate in isolate memory. Pass A relaxes the object cap for `Header::Blob`
  entries only; pass B copies the entry's `varint header + zlib body` verbatim
  from the pending pack to the normalized pack in ≤ 8 MiB `read_range`
  fragments (`PackWriter::raw_extend`/`raw_entry_done`), while a resumable zlib
  stream re-inflates purely to compute the object id. Full blobs are now
  bounded by the 2 GiB pending-pack ceiling. Delta *results* and non-blob full
  objects keep the 16 MiB cap — they still materialize for link extraction /
  delta application. A delta naming a streamed blob as base fails cleanly at
  `Window::entry`'s wire bound or `decode_mini`'s alloc cap.
- **Fetch fragmentation**: `coalesce` could emit a `Read` larger than the
  8 MiB window (one big entry, or a merge extending past it), and `pack_chunk`
  materialized `r.len` in one `read_range`. Reads are now hard-capped at
  WINDOW; oversized entries are emitted as fragment reads — `ents` carries
  byte ranges, so output stays byte-identical and the trailer hash is correct.
- **GC streamed copy**: `build` splits each chunk by wire length — entries
  ≤ 8 MiB ride `read_entries` as before; larger ones stream fragment-by-fragment
  into the writer via `raw_extend` + `drain_parts`. Drained parts are recorded
  in `gc_parts` only inside `flush()`'s span, alongside the WriterCkpt that
  accounts for their bytes — recording earlier would break resume numbering.
  Mid-entry yield is safe: nothing commits until a checkpoint and replay is
  deterministic.
- **Mid-upload error UX**: post-header receive-pack failures now drain the
  request body (bounded at 2 GiB) before answering — previously Cloudflare
  reset the connection mid-upload and the client saw a transport error instead
  of `unpack <reason>`/`ng`. Verified: a delta result over 16 MiB returns HTTP
  200 + `unpack object too large` + `ng refs/heads/main unpack failed` on
  production.
- **Per-repo tokens**: `tokens(id, hash, level, name, created_at)` table in the
  DO; edge `authenticate` tries global secrets first, then one `/_do/auth`
  stub call keyed by `sha1(token)` — raw tokens never cross the stub boundary
  and only hashes are stored. Admin surface: `POST /_admin/tokens {name,level}`
  (mints `ge_<64 hex>`, shown once), `GET /_admin/tokens` (list), `DELETE
  /_admin/tokens/<id>` (revoke) — all gated on the global write token only, so
  repo credentials can't mint more credentials. A repo read token gets 403 on
  push, 401 on other repos and after revoke. Principal in the reflog is the
  token's `name`.
- **Analytics Engine**: `GE_METRICS` binding (dataset `git_edge`) — one
  datapoint per edge request: index = `owner/repo`, blob = op
  (info-refs/fetch/push/state/admin/other), doubles = status, ms to response,
  subrequests used. Absent binding (local dev) is a no-op.

Verified: conformance `== PASS` locally and on prod; 30 MiB blob push (9 s) +
clone fsck-clean byte-identical on prod; 20 MiB blob through GC consolidate
locally (streamed copy, fsck clean); token create/use/403/401/revoke on prod;
delta-over-cap `unpack`/`ng` report-status on prod.

## Round 6 — post-feature audit (5 parallel auditors + adversarial executor)

Findings fixed this round:

- **GC cursor claimed unwritten entries (P0, fixed).** `pos.idx` advanced a
  whole planned batch at *planning* time; a checkpoint after the first chunk
  persisted a cursor past uncopied entries — replay skipped them, `count !=
  expected` wedged consolidate into a rebuild loop. `pos` now advances per
  completed entry; `pos.frag` records mid-entry fragment progress (serde
  `default` keeps old checkpoints parseable).
- **Non-uniform MPU parts (P0, fixed).** `checkpoint()` drained the whole
  variable-size buffer as one part; R2 (and miniflare) reject `complete()`
  when non-final parts differ in size — `BadUpload` 10048, retry/rebuild
  loop forever. `checkpoint()` now drains exactly `PART` (8 MiB) per call,
  `sha1` feeds only at upload so `WriterCkpt` describes the durable prefix,
  and `flush()` rewinds the *persisted* cursor (ci, idx, frag, count) to the
  entry containing the durable byte — buffered bytes are never claimed.
- **Meta crash masked real errors (P1, fixed).** `RepoDo::meta()` called
  `cursor.one()` on possibly-empty results; the missing `gc.fails` row threw
  uncatchably across the JS/WASM boundary ("Critical error"), hiding the
  root error and looking like an isolate crash. `meta()` now delegates to
  `meta_opt()` — a missing key is a catchable storage error.
- **Delta-base prefetch shadowed the actionable error (P1, fixed).** The
  external-base prefetch called `decode_entry` on a streamed >16 MiB base,
  throwing `object too large` before `external()`'s friendly message; bases
  over `MAX_OBJ` are now excluded from prefetch. Verified locally:
  `unpack delta base <sha> exceeds 16 MiB; push the object as a full blob
  (e.g. git -c core.bigFileThreshold=1 push)` — and the workaround push
  lands byte-identical. Note `git push --no-thin` still sends REF_DELTA on
  git 2.54 — the message no longer suggests it.
- **Auth hardening (P1/P2, fixed).** `token_hash` returns `Result` (a hash
  failure can no longer collapse to an empty hash); a `ge_`-prefix prefilter
  rejects non-token strings before the DO lookup; token levels must be
  exactly `read`/`write`; admin token bodies are bounded (64 KiB).
- **Drain deadline (P2, fixed).** `BodyReader::drain` now races each read
  against `worker::Delay` — a stalled client can't hold the isolate on I/O
  await forever.

Adversarial executor (independent, final binary): 16 MiB boundary push
byte-exact; delta-result-over-cap `unpack`/`ng` clean; REF_DELTA against
streamed bases fails deterministically in all four variants (external,
in-pack, forward-ref, oversized-window); token API edges all correct;
CAS failure yields `ng` report-status; `clone --filter=blob:none` backfills
the 17 MiB blob byte-identical.

Verified after fixes: conformance `== PASS` on the fixed binary; fresh
22 MiB + 18 MiB blob repo consolidated by GC (`packs_live 2 -> 1`, uniform
8 MiB parts in miniflare's R2 state) with a byte-identical fsck-clean clone.

Post-fix verification, continued:

- **Wedged-repo self-heal — verified.** The repo wedged under the pre-fix
  binary (non-uniform MPU parts, `gc_consolidate` at attempts 6) recovered
  unattended on the fixed binary: `BadUpload` → `gc.fails`≥2 → Rebuild →
  fresh uniform-part MPU → `complete()` → sweep. Final state: 5 dead packs
  (2 sources + 3 failed builds), 1 live consolidated pack (7 objects,
  17.8 MB), `git clone` + `fsck --strict` clean. Recovery took the
  predicted couple of backoff cycles with no manual intervention.
- **Production GC consolidation on real R2 — verified** (deployment
  `a25913fb`). After the two test packs aged past the 1-hour
  `GE_GC_GRACE_MS` window, a push re-armed `gc_mark` (+10 min quiet);
  the chain ran mark (`marked=2`) → consolidate → sweep in ~60 s on real
  R2: `packs_live 3 → 2` (consolidated pack + the grace-protected new
  push pack), `packs_dead 0 → 2`, zero dead jobs. The uniform-part MPU
  `complete()` that previously livelocked under `BadUpload` now succeeds
  against real R2. Post-consolidation `git clone` is `fsck --strict`
  clean and byte-identical to the source working tree.
- Earlier observation: on first deploy the chain fired on schedule but
  correctly no-op'd — packs inside the grace window are never marked, so
  a fresh repo consolidates nothing until packs are 1 h old.

Remaining known gaps (documented, not blocking):
- `jobs` repair requeues `running` rows after the 60 s lease; two
  overlapping executions could interleave awaited writes. Heartbeats at
  every checkpoint narrow the window; lease fencing remains the guard.
- Error responses still drop `x-ge-subrequests` (P3 observability).
- `coalesce` keeps a defensive `unwrap_or(u32::MAX)` on a value bounded by
  WINDOW (P3); a future invariant change should fail loudly instead.
