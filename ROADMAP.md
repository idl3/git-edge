# git-edge roadmap

Target profile: **agent-built small and disposable apps** — repos up to a few
hundred MB and tens of thousands of commits, high churn (create/push/dispose),
API-driven workflows, minimal ceremony. Not a monorepo host, not a GitHub
replacement.

## Can real repos fit? (benchmarked)

Public repos staged-pushed to a local worker via `tests/bench/oss-bench.sh`
(<60 MiB / <30k objects per slice), then cloned back:

| Repo | Commits | Objects | Pack size | Import | Clone | Result |
|---|---|---|---|---|---|---|
| sinatra/sinatra | 4,684 | 22.6k | 8 MiB | 1 push, 11 s | 2 s, fsck clean | fits |
| expressjs/express | 6,169 | 32.5k | 11 MiB | 2 pushes, 15 s | 8 s, fsck clean | fits |
| vitejs/vite | 9,678 | 110k | 75 MiB | 4 pushes, 337 s | 133 s, fsck clean | fits |
| facebook/react | 21,698 | 263k | 1,078 MiB | 25 pushes, 337 s | **413** | import fits, clone over budget |
| rails/rails | 99,661 | 787k | 308 MiB | 39 pushes, 644 s | **413** | import fits, clone over budget |
| microsoft/TypeScript | 39,366 | 945k | 2,805 MiB | stopped | — | **import infeasible** — single commit introduces 222k objects; can't stage below one-commit granularity |

**The binding constraints are now mapped.** Import scales far past the profile
(rails' 787k objects landed fine in 39 slices); the walls that bite are:

- **~100 MB request body cap** on initial import — staged pushes (or the
  `tools/git-edge-import.sh` slicer) are the workaround.
- **Single-commit object explosions can't be staged** — TypeScript carries a
  commit that alone introduces ~222k objects; staged-push granularity is one
  commit, so no client-side slicing can get it under the ingest budget. This
  is the concrete case for server-side import (`POST /_admin/import`).
- **Per-request subrequest budget on clone/fetch** — somewhere between
  110k objects (vite ✓) and 263k objects (react ✗) a full clone exhausts the
  request budget and the DO returns `Error::Budget` → HTTP 413 mid-walk.
  `blob:none` does not help: the budget is spent on object-index reads
  (commits + trees), not blob bytes. Everything else has 5–25× headroom at
  profile scale (200k-commit walk, 1M objects/fetch, 2M objects/pack,
  2 GiB pending pack, 65k refs).

For the small-app profile — typically well under 50k objects — both clone and
import sit comfortably inside limits. Repos in the react/rails class import
fine but need a lift on the fetch budget before they can be cloned.

The original (private-repo) benchmark flushed out two real bugs, both long fixed:

- **Clone 413 on wide histories** — `read_entries` bounds one call at 48 MiB of
  coalesced spans; a 10k-tree chunk or a multi-pack commit prefetch can exceed
  it. `read_entries_chunked` now splits the set and retries the halves.
- **Dangling `HEAD` on non-`main` first push** — `meta.head` was hardcoded to
  `refs/heads/main`; a `master`-first repo cloned without checkout. `commit_push`
  now adopts an existing branch when the configured head is dangling.

## Priority 0 — required for the profile

| # | Item | Why | Size |
|---|---|---|---|
| 1 | ~~Fetch read-batch splitting~~ | done — was a hard clone failure at 70k+ objects | S |
| 2 | ~~HEAD adoption on first push~~ | done — every scaffolded repo pushes `master` | XS |
| 3 | ~~**Repo delete**~~ | done — `POST /_admin/delete` tombstones (410), `purge_repo` wipes R2+DO, name reusable after | M |
| 4 | ~~**Bulk import path**~~ | done via option (a) — `tools/git-edge-import.sh` auto-slices a repo into <SLICE_MIB pushes with resume, `--all-branches`, `--dry-run`; option (b) `POST /_admin/import` (R2 bundle/pack) remains the structural fix | S–M |
| 5 | ~~**Anonymous/public read**~~ | done — `meta.public` flag via `POST /_admin/public`; anonymous reads only when no credential presented | S |
| 6 | ~~**Dead-job / GC alerting**~~ | done — `blob3=dead` job datapoints + `jobs_dead` gauge per alarm pass; alert wiring documented (A18) | S |
| 7 | ~~**Job-lifecycle metrics**~~ | done — `job_event` datapoints: kind/event/outcome/error-class/attempt/duration (A17) | S |
| 8 | **Agent skill** | ship `.devin/skills/git-edge` (or AGENTS.md section): clone/push URLs, `_admin/tokens` minting, staged-push recipe for >80 MiB, credential-helper config (never tokens in URLs — process-listing leak), `_state` introspection | S |

## Priority 1 — production hardening

| # | Item | Why | Size |
|---|---|---|---|
| 9 | ~~Repo quota + abuse limits~~ | done — `GE_QUOTA_MAX_REPOS_PER_OWNER` (50) via `owner!<o>` registry DO, `GE_QUOTA_MAX_OBJECTS` (2M)/`GE_QUOTA_MAX_BYTES` (4 GiB) at `commit_push` (A26) | M |
| 10 | ~~Rate limiting~~ | done — sliding-window `rate` table check in `push_begin`, `GE_RATE_PUSHES_PER_MIN` (30), 429 + `Retry-After` (A27); zone rules remain the heavy hammer | S |
| 11 | ~~**Export endpoint**~~ | done — `GET /_admin/export` streams a real v3 `git bundle`; verify + clone-from-bundle pass (A25) | M |
| 12 | ~~**`x-ge-subrequests` on error responses**~~ | done — request-wide `Spend` tally + `ReqBudget::reporting`; `respond` stamps every error (and non-streamed success), DO stamps its own errors, `from_do_response` folds DO spend in | XS |
| 13 | ~~**Lease-overlap hardening**~~ | done — heartbeat is a lease CAS; stale slices bail as `stale` without consuming attempts (A19) | S |
| 14 | ~~**`coalesce` `unwrap_or(u32::MAX)`**~~ | done — loud `Error::Limit` → 413 (A21) | XS |
| 15 | ~~ls-refs caching~~ | done — DO memoizes the refs snapshot + `/_do/refs`/`/_do/ls-refs` bytes on `refs_version` (A28); `_state` reports hits/misses | S |
| 16 | ~~**Ref pinning**~~ | done — `pins` table + `/_admin/pin|unpin`; push to a pinned ref gets `ng "ref is pinned"` (A24) | S |

## Priority 2 — worthwhile, not blocking

| # | Item | Why | Size |
|---|---|---|---|
| 17 | **`--atomic` push** | single-ref agent pushes don't need it; multi-ref CI flows do | M |
| 18 | **Repo rename / owner move** | cosmetic; delete+repush covers it | S |
| 19 | **Per-ref token scopes / deploy keys** | read/write covers the profile; per-branch ACLs are the GitHub-shaped feature nobody asked for yet | M |
| 20 | **Custom domain + Cloudflare Access** | zero-code auth upgrade if a zone exists | S |
| 21 | **Git LFS** | the structural answer for >100 MB assets; the profile rarely needs it — revisit when a real workload does | L |
| 22 | **Streaming no-walk clone** | removes the 200k-commit bound; irrelevant below it | M |
| 23 | **Verbatim consolidated-pack fast path** | post-GC one pack covers all reachable objects → stream it verbatim as the fetch response; ~1 subrequest per R2 `get` instead of thousands of entry reads. Lifts the 110k–263k-object clone wall for default clients. Design: `findings/scale-ceilings.md` C2 | M |
| 24 | **`packfile-uris` offload** | same coverage test as #23 but hands the client a signed `/packs/<key>` URL — clone spend ~10 subrequests, bandwidth bypasses the Worker. Opt-in (`fetch.uriprotocols` defaults empty). Design: `findings/scale-ceilings.md` C1 | M |

## Wild bucket — parked, worth remembering

- **Synthetic-ref object seeding** — split an oversize commit's objects across
  staging refs so the real push dedups against them; client-side fix for
  TypeScript-class imports, zero server changes (`findings/scale-ceilings.md` I2).
- **GitHub URL import** — `POST /_admin/import {url}` server-side clones a
  public GitHub repo. No client staging at all; Worker outbound fetch has no
  body cap on the *response* side. Would obsolete item 4 for public sources.
- **TTL repos** — `expires_at` on meta; janitor sweeps expired repos. The
  purest expression of "disposable".
- **Bundle/PR-preview conventions** — `refs/pr/*` namespace with automatic
  retention policy.
- **Signed-push / commit-signature policy** — enforce `gpgsig` on push for
  release branches.
- **SHA-256 object format**, **v0/v1 fetch** (pre-2.26 clients),
  **`tree:`/`combine:` filters**, **`object-info`**, **`bundle-uri`** —
  deliberately unadvertised; add when a client actually needs them.
- **Jurisdiction-pinned DOs** — EU repos in EU DOs if compliance demands it.
- **`git-edge mount` via artifact-fs** — Cloudflare's artifact-fs is a FUSE
  lazy-mount client that works with *any* git remote, and git-edge already
  speaks everything it needs (v2, `blob:none`, promisor backfill,
  `allowAnySHA1InWant`). Documenting the mount path — or running their e2e
  suite with `AFS_E2E_REPO` pointed at git-edge — is the cheapest
  lazy-checkout story available; building our own mount client is not.

## Explicit non-goals

- Monorepo-scale hosting (multi-GB packs, million-commit walks)
- Web UI / code browsing / issue tracking — this is a storage service
- Hooks (`pre-receive`, `post-receive`) — no user code runs in the DO
- Self-serve multi-tenant SaaS — auth is tokens, not accounts
