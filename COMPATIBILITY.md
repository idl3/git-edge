# git-edge — feature & compatibility reference

What git-edge is: a Git smart-HTTP server compiled to WASM on a Cloudflare
Worker. One SQLite Durable Object per repository holds refs, the object index,
and job state; R2 holds normalized packfiles. There is no always-on machine and
no upstream Git service in the request path. All claims below are verified
against a real stock `git` client on local workerd **and** against a live
Cloudflare deployment (`git-edge.grain.workers.dev`, Paid plan); see
`findings/implementation.md` for the test evidence.

## Transport & protocol versions

| Surface | Upstream git | git-edge | Status |
|---|---|---|---|
| Smart HTTP `GET info/refs?service=git-upload-pack` | v0/v1/v2 | v2 advertisement; v1 emits a `version 1` packet | ✅ |
| Smart HTTP `POST git-upload-pack` (fetch/clone) | v0/v1/v2 | **protocol v2 only** — v0/v1 POST → HTTP 400 `ERR protocol v2 required (git >= 2.26)` | ⚠️ git < 2.26 cannot fetch |
| Smart HTTP `info/refs?service=git-receive-pack` + `POST git-receive-pack` (push) | v0 | v0 receive-pack (same as upstream — git clients never speak v2 push) | ✅ |
| `git://` protocol | ✅ | — | ❌ HTTP(S) only |
| SSH transport | ✅ | — | ❌ HTTP(S) only |
| Dumb HTTP protocol | ✅ | — | ❌ |

**Client floor:** any git ≥ 2.26 (March 2020) clones, fetches, and pushes
normally. Older clients fail fetch with a clean protocol error, not a hang or
corruption.

## Advertised capabilities (v2)

```
version 2
agent=git-edge/0.1
ls-refs=unborn
fetch=shallow filter
object-format=sha1
```

receive-pack: `report-status delete-refs side-band-64k quiet ofs-delta
object-format=sha1`

## Fetch / clone (upload-pack)

| Feature | Upstream | git-edge | Notes |
|---|---|---|---|
| `want`/`have`/`done` negotiation | ✅ | ✅ | multi-round with ACK/NAK; `ready` gating matches git |
| Stateless multi-round fetch | ✅ | ✅ | verified with partial-ACK rounds |
| `want-ref` (fetch ref by name) | ✅ | ✅ parsed & resolved | `ref-in-want` is not advertised, so stock clients won't send it |
| `want` by arbitrary oid (`allowAnySHA1InWant` equivalent) | off by default | **always on** | required for promisor backfill; see caveats |
| `deepen <n>` | ✅ | ✅ | descends through client-shallow commits; deepens correctly from depth-1 clones |
| `deepen-since <ts>` | ✅ | ✅ | |
| `deepen-not <ref>` | ✅ | ✅ | ref search order (`mid` → `refs/tags/mid`); annotated tags peeled; unresolvable → protocol error |
| `deepen-relative` | ✅ | ✅ | depth counted beneath the old shallow boundary |
| `--unshallow` | ✅ | ✅ | `unshallow` emitted only when parents are actually delivered |
| `shallow`/`unshallow`/`shallow-info` lines | ✅ | ✅ | client-held commits never re-emitted as `shallow` |
| `filter blob:none` | ✅ | ✅ | promisor backfill works (verified checkout) |
| `filter blob:limit=<n>` | ✅ | ✅ | |
| `filter tree:<n>`, `sparse:oid`, `combine:`, `object:type=` | ✅ | — | clean `unknown argument` protocol error |
| `thin-pack` arg | ✅ | accepted | packs sent are self-contained — deltas never appear on the wire |
| `include-tag` | ✅ | ✅ | peeled tags of wanted commits only |
| `sideband-all` | ✅ | — | not advertised; plain side-band-64k semantics |
| `packfile-uris` (CDN offload) | ✅ | — | not advertised |
| `wait-for-done` | ✅ | — | not advertised |
| `no-done` | ✅ | tolerated, not advertised | parsed; we answer before `done` anyway |
| `object-info`, `bundle-uri`, `server-option` commands | ✅ | — | unknown command → protocol error |
| `session-id` | ✅ | ignored | parsed, not echoed |
| Fetch of hidden/unadvertised refs | ✅ | ✅ via `want-ref` (unadvertised) or raw oid | |

## Push (receive-pack)

| Feature | Upstream | git-edge | Notes |
|---|---|---|---|
| Create / update / delete refs | ✅ | ✅ | incl. delete-only pushes (no PACK) |
| Compare-and-swap (`old-oid` guard, `--force-with-lease`) | ✅ | ✅ | stale push rejected; verified under concurrency |
| Thin packs (delta vs repo objects) | ✅ | ✅ | forward-**and** backward `REF_DELTA`, nested chains, `OFS_DELTA` |
| `report-status` | ✅ | ✅ | |
| `report-status-v2` | ✅ | accepted, not advertised | option lines not supported |
| `delete-refs`, `side-band-64k`, `quiet` | ✅ | ✅ | |
| `atomic` | ✅ | — | not advertised → client-side refusal |
| `push-options` (`-o`) | ✅ | — | not advertised → client-side refusal |
| Signed pushes (`push-cert`) | ✅ | — | |
| `shallow` lines from a shallow client | ✅ | parsed, ignored | push proceeds; see caveats |
| Push to unborn/empty repo | ✅ | ✅ | |
| Flush-only push ("Everything up-to-date") | ✅ | ✅ | |

## Refs & object model

| Feature | Status |
|---|---|
| Full ref namespace (`refs/heads`, `refs/tags`, `refs/remotes`, arbitrary) | ✅ |
| Annotated tags + `peel` in ls-refs | ✅ |
| Symrefs (`symrefs` arg, `HEAD`) | ✅ |
| Unborn `HEAD` (`ls-refs=unborn`) | ✅ |
| `ref-prefix` filtering (≤ 32 prefixes) | ✅ |
| SHA-1 object format | ✅ |
| SHA-256 repos | ❌ clean `object-format sha256 unsupported` error |
| `git fsck --strict` on clones | ✅ clean across all tests |

## Storage & maintenance model

| Property | Upstream `git http-backend` | git-edge |
|---|---|---|
| Pack storage | loose + packed on disk | normalized full-object packs in R2, indexed in per-repo SQLite |
| Wire deltas on fetch | yes (thin packs) | **never** — all entries sent as full objects (larger fetch bodies, simpler/constant memory) |
| Delta compression at rest | yes | no — deltas resolved at ingest, stored inflated |
| GC / repack | `git gc`, manual or auto | automatic mark→consolidate→sweep via DO alarms; `GC_QUIET=10 min`, 1 h grace before R2 deletion |
| Job slices | n/a | 20 s wall / 400 subrequest slices, resumable cursors, lease-fenced, dead-lettered |
| Quarantine on push | quarantine dir | `pending/` R2 key + `open` push row; ref update is a CAS transaction |

## Hard limits (vs upstream)

| Limit | git-edge | Upstream |
|---|---|---|
| Full object (blob/commit/tree/tag pushed whole) | **~2 GiB** (the pending-pack bound) — streamed verbatim, never materialized | none in git (GitHub: 100 MiB) |
| Delta *result* object | **16 MiB** — delta resolution materializes in isolate memory | none |
| Non-delta commit/tree/tag | 16 MiB (parsed for links; always tiny in practice) | none |
| Pack entry wire size (non-blob) | 32 MiB | none |
| Pending pack per push | 2 GiB | none (disk) |
| HTTP request body per push (Cloudflare platform) | **~100 MB** — larger pushes must be split (`git push <mid>:main` then `git push main`) | none |
| Objects per pack | 2,000,000 | none |
| Delta chain depth | 64 (git default: 50) | unbounded-ish |
| Aggregate delta-chain bytes | 64 MiB compressed / 128 MiB live | none |
| Refs per repo | 65,536 | none |
| Fetch commit-walk bound | 200,000 commits or 64 MiB mem → `ERR fetch too large` | none |
| Reachable objects per fetch | 1,000,000 | none |
| Push links (tree edges) per push | 1,000,000 | none |
| upload-pack request body | 1 MiB | ~unbounded |
| ls-refs prefixes | 32 | unbounded |
| Per-request budget | 9,000 subrequests / 240 s | n/a (self-hosted) |

Every limit fails with a clean `ERR`/`unpack`/`ng` line — never a hang, silent
truncation, or corrupted ref state.

## Caveats & known mismatches

1. **git < 2.26 cannot fetch.** Protocol v2 is mandatory on `git-upload-pack`.
   Push is unaffected (all gits speak v0 receive-pack anyway).
2. **Any reachable oid is fetchable** (`allowAnySHA1InWant` semantics always
   on). Required so partial-clone promisor backfill works. Upstream leaves it
   off by default — here it is a deliberate choice, not an oversight.
3. **No wire deltas, ever.** `thin-pack`/`ofs-delta` requests are accepted but
   responses carry full objects only. Legal per protocol; fetch bodies are
   larger than a deltifying server's. This is what makes constant-memory
   streaming possible.
4. **`--atomic` and `-o <push-option>` refuse client-side.** Capabilities are
   not advertised, so the client errors before the request — clean, but these
   workflows are unavailable.
5. **Shallow-push tracking is not maintained.** `shallow` lines from a shallow
   client are parsed and ignored; the server does not remember that a pushed
   history was truncated. Consequence is benign (objects are stored; the
   connectivity rule still requires referenced bases to exist).
6. **v0/v1 fetch, `tree:`/`sparse:`/`combine:` filters, `packfile-uris`,
   `sideband-all`, `object-info`, `bundle-uri`, sha256** — deliberately
   unadvertised; clients get a protocol error, never silent misbehavior.
7. **Auth is static tokens, but now per-repo too.** `GE_READ_TOKEN`/
   `GE_WRITE_TOKEN` remain the deployment-wide admin credentials (HTTP Basic or
   Bearer). Per-repo tokens are minted via `POST /:owner/:repo/_admin/tokens`
   `{name, level}` (global-write-token only — repo credentials cannot mint more),
   listed via `GET`, revoked via `DELETE /_admin/tokens/<id>`. Only sha1 hashes
   are stored; a token is shown once at creation. Read tokens get 403 on push.
   Still no anonymous access, per-branch permissions, or user accounts.
8. **No LFS.** Full objects now stream verbatim up to the 2 GiB pending-pack
   bound, so ordinary large blobs are fine — but anything pushed *as a delta*
   whose result exceeds 16 MiB is still rejected (`unpack object too large`),
   and a REF_DELTA whose *base* is a streamed >16 MiB object is rejected with
   `delta base <oid> exceeds 16 MiB`. Note `git push --no-thin` does **not**
   prevent a client from sending REF_DELTA (verified on git 2.54) — the
   reliable workaround is pushing the object undeltified, e.g.
   `git -c core.bigFileThreshold=1 push`. The ~100 MB platform body cap
   applies per request. Very large assets should still live outside git.
9. **No hooks, repo rename/delete, or web UI.** A repo is created by pushing to
   it; `/_state` (write-auth) and `/_admin/tokens` are the only introspection /
   management endpoints. Basic request metrics (op, status, ms, subrequests per
   repo) are emitted to Analytics Engine when the `GE_METRICS` binding exists.
10. **Real-deploy verified:** R2 multipart semantics, DO alarms, and Paid-plan
    subrequest limits are all confirmed against a live deployment
    (`git-edge.grain.workers.dev`); the ~100 MB body cap is real.

## Verified performance envelope (local workerd)

| Workload | Result |
|---|---|
| Push 20k-commit linear history (60k objects) | 8.8 s |
| Clone 20k commits | 4.3 s — 5/9000 projected subrequests |
| Clone `--depth 15000` | 1.4 s, fsck clean |
| Push 200 MiB / 10,207 objects | 21 s |
| 4× parallel 20k clones | 6.9 s total |
| 5× parallel CAS-divergent pushes | all correctly rejected |
| GC during concurrent clone (60k objects) | mark→sweep < 4 s, clone clean |
| Nested forward `REF_DELTA` pack | resolved, byte-exact |
| Cyclic `REF_DELTA` pack | `unpack missing base` in ~60 ms |
| Push 30 MiB single blob (prod) | 9 s, clone fsck-clean, byte-identical |
| Push 20 MiB blob → GC consolidate → clone (local) | streamed verbatim copy, fsck clean |
| Push ~95 MiB pack (prod) | 35 s — near the ~100 MB platform body cap |
| Push ~130 MiB pack (prod) | HTTP 413 at the zone before app code — split the push |

## Interoperability test matrix (git 2.54, live)

`clone`, `clone --depth`, `clone --filter=blob:none` + promisor checkout,
`fetch`, `fetch --depth/--deepen/--shallow-since/--shallow-exclude`,
`--unshallow`, `push` (initial, thin-delta incremental, branch, tag,
delete-only, force-with-lease CAS), `ls-remote`, multi-round negotiation —
all pass, all clones `fsck --strict` clean. Full details:
`findings/implementation.md`, `tests/conformance/run.sh`.
