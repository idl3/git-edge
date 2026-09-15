# git-edge roadmap

Target profile: **agent-built small and disposable apps** — repos up to a few
hundred MB and tens of thousands of commits, high churn (create/push/dispose),
API-driven workflows, minimal ceremony. Not a monorepo host, not a GitHub
replacement.

## Can real repos fit? (benchmarked 2026-09-15)

Two production Rails repos pushed to a local worker, staged by commit ranges:

| Repo | Commits | Objects | Pack size | Import | Clone | Result |
|---|---|---|---|---|---|---|
| atlas-core `master` | 5,376 | 150k (all refs) | 153 MiB | 4 pushes, ~63 s | 11.6 s, fsck clean | fits |
| grain-core `main` | 6,570 | 403k (all refs) | 434 MiB | 8 pushes, ~26 s | 20.3 s, fsck clean | fits |

Every structural limit has 5–25× headroom at this scale: 200k-commit fetch
walk, 1M objects per fetch, 2M objects per pack, 2 GiB pending pack, 65k refs.
The binding constraint is the ~100 MB per-request body cap on initial import —
worked around today by staged pushes.

The benchmark also flushed out two real bugs (both fixed in this branch):

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
| 3 | **Repo delete** | "disposable" is literal — there is currently *no* way to delete a repo, its DO storage, or its R2 packs. `POST /:o/:r/_admin/delete` → mark refs gone, enqueue janitor sweep of packs + objects + tokens | M |
| 4 | **Bulk import path** | staged `git push` works but is a client-side workaround. Options: (a) `git-edge-import` CLI that auto-slices a local repo into <80 MB pushes — zero server work; (b) `POST /_admin/import` accepting an R2-uploaded bundle/pack — eliminates the body cap entirely | S–M |
| 5 | **Anonymous/public read** | agents share read-only links constantly; today every clone needs a token. Per-repo `public` flag on the tokens API, read-side only | S |
| 6 | **Dead-job / GC alerting** | `jobs_dead>0` in `_state` is the wedge signal; emit it to Analytics Engine (or poll from a cron Worker) so a livelocked repo pages instead of festering | S |
| 7 | **Job-lifecycle metrics** | artifact-fs's NDJSON schema (phase/state/attempt/duration/retryable) is the model: emit mark/consolidate/sweep outcome datapoints, not just request-level — makes a wedged GC visible without `_state` polling | S |
| 8 | **Agent skill** | ship `.devin/skills/git-edge` (or AGENTS.md section): clone/push URLs, `_admin/tokens` minting, staged-push recipe for >80 MiB, credential-helper config (never tokens in URLs — process-listing leak), `_state` introspection | S |

## Priority 1 — production hardening

| # | Item | Why | Size |
|---|---|---|---|
| 9 | ~~Repo quota + abuse limits~~ | done — `GE_QUOTA_MAX_REPOS_PER_OWNER` (50) via `owner!<o>` registry DO, `GE_QUOTA_MAX_OBJECTS` (2M)/`GE_QUOTA_MAX_BYTES` (4 GiB) at `commit_push` (A17) | M |
| 10 | ~~Rate limiting~~ | done — sliding-window `rate` table check in `push_begin`, `GE_RATE_PUSHES_PER_MIN` (30), 429 + `Retry-After` (A18); zone rules remain the heavy hammer | S |
| 11 | **Export endpoint** | `GET /:o/:r/_admin/export` → streams a `git bundle` of live refs. Disposability = easy in *and* easy out; also the backup story | M |
| 12 | **`x-ge-subrequests` on error responses** | audit P3 — errors currently drop the accounting header | XS |
| 13 | **Lease-overlap hardening** | audit P3 — a `running` job requeued after 60 s can overlap its original slice; heartbeats narrow it, fencing is the guard. Tighten or document | S |
| 14 | **`coalesce` `unwrap_or(u32::MAX)`** | audit P3 — a value bounded by WINDOW should fail loudly, not saturate | XS |
| 15 | ~~ls-refs caching~~ | done — DO memoizes the refs snapshot + `/_do/refs`/`/_do/ls-refs` bytes on `refs_version` (A19); `_state` reports hits/misses | S |
| 16 | **Ref pinning** | `POST /_admin/pin {ref, sha}` — frozen refs that reject updates; the server-side analog of artifact-fs's `--require-commit` verified acquisition, for deploy-snapshot workflows | S |

## Priority 2 — worthwhile, not blocking

| # | Item | Why | Size |
|---|---|---|---|
| 17 | **`--atomic` push** | single-ref agent pushes don't need it; multi-ref CI flows do | M |
| 18 | **Repo rename / owner move** | cosmetic; delete+repush covers it | S |
| 19 | **Per-ref token scopes / deploy keys** | read/write covers the profile; per-branch ACLs are the GitHub-shaped feature nobody asked for yet | M |
| 20 | **Custom domain + Cloudflare Access** | zero-code auth upgrade if a zone exists | S |
| 21 | **Git LFS** | the structural answer for >100 MB assets; the profile rarely needs it — revisit when a real workload does | L |
| 22 | **Streaming no-walk clone** | removes the 200k-commit bound; irrelevant below it | M |

## Wild bucket — parked, worth remembering

- **`packfile-uris` offload** — advertise pack segments as R2 URLs (public or
  presigned) so fetch bandwidth bypasses the Worker entirely. Cheap to try,
  big subrequest/bandwidth win. Protocol v2 supports it today.
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
