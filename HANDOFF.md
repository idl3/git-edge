# git-edge handoff — 2026-09-16 (I1 import shipped; C2+C1 in PRs)

Serverless Git smart-HTTP host: Rust/WASM Cloudflare Worker + per-repo SQLite
Durable Object + R2 packfiles. Repo: `idl3/git-edge`. Main is at `257184b`,
clean tree, all work merged (PRs #7–#16).

## Where things stand

All P0/P1 roadmap items are done. **C2 (A29), C1 (A30), and I1 (A31) are
implemented** — C2/C1 are PRs #17/#18 (both OPEN/CLEAN, stacked); I1 sits
uncommitted on the working branch pending the at-scale validation below:

- **C2 verbatim consolidated-pack fast path** (`consolidated_pack` +
  `VerbatimStream` in `repo_do/mod.rs`): plain-clone-shaped fetch + exactly
  one live pack covering all wants → stream that R2 object verbatim, one
  `bucket.get`, `x-ge-subrequests: 1`. **Re-benched on react: 263k-object
  clone in 18.7 s at 1 subrequest** (was HTTP 413 at the old wall).
- **C1 `packfile-uris`** (`pack_uri_response` + `sign.rs` + edge `pack_get`):
  opted-in clients (`fetch.uriprotocols`, git >= 2.40) instead get a signed
  `/_packs/<id>.pack?e=&r=&s=` URL — HMAC-SHA256 over `v1\nrepo_id\npack\nexp`
  keyed by `GE_URL_SIGNING_KEY`, 1h TTL. Inline `packfile` section is a legal
  empty pack; the `<hash>` token is the pack's real trailer SHA-1 (client
  verifies). Bandwidth fully bypasses the Worker. **React via URI: 14.8 s.**
- **I1 server-side resumable import** (`jobs/import.rs`, A31):
  `POST /_admin/import/stage` streams a pack part into `pending/` and opens a
  push; further parts reuse it via `?push=<id>&part=<n>` (idempotent re-begin —
  required, since only the job's own push gets heartbeat protection from the
  janitor's PUSH_TIMEOUT sweep); `POST /_admin/import {push, parts, commands}`
  queues an `import_pack` job; `GET /_admin/import/<push>` reports
  phase/objects. The job rides the
  normal push lifecycle — pass A parses a persistent `import_toc` off the
  staged parts, pass B normalizes entries into a resumable output MPU
  (`import_parts` etags + `WriterCkpt`; `import_open` shadow rows name the
  mid-entry resume point), 2.5 links live in `push_links` (no 1M cap), and the
  final slice runs `commit_push` with atomic semantics. Kills the
  single-commit ingest ceiling (TypeScript's 222k-object commit).

**I1 correctness model, compressed**: durable = uploaded parts only.
`objects`/`push_links` rows may run ahead inside a slice; every resume
re-derives — delete objects rows past `WriterCkpt.pos`, restore fully-durable
unposted rows from `import_open` shadow cols, frag-resume the one straddling
writer (`skip = durable - off`), retry parked REF_DELTA entries rather than
trusting cursor wake bookkeeping. `end <= durable` OR an objects row = done —
covers dup-sha dedup and posted-but-durable cases.

**Bugs the smoke run flushed out** (all fixed): TOC loop gated on the flushed
counter so `count < TOC_FANOUT` packs over-parsed past the trailer; a
checkpoint-time `DELETE >durable` killed just-posted rows every slice (resume
owns that delete, not the checkpoint); the queue scan's `idx > 0` skipped
entry 0; `PackWriter::resume` at `pos=0` dropped the 12-byte pack header
(clone died on `pack signature mismatch`) — resume now re-emits it.

**Bugs the 462k react run flushed out** (all fixed): the checkpoint reused a
stale `c.st` instead of the live `out.snapshot()`, freezing the durable prefix
while mid-slice `flush_if_full` uploads piled into `import_parts` — a
zero-progress livelock; dead slices left stale etags past `pos/PART` that a
resume would have assembled into the final MPU alongside the rewritten parts —
resume now truncates the list to the live prefix and `flush_ckpt` prunes rows
beyond it; each
`import/stage` call minted its own `open` push, so 16 sibling part pushes hit
PUSH_TIMEOUT mid-import and the janitor swept the source parts from under the
running job — fixed by shared-push staging (`?push=&part=`) + a janitor
prefix delete of `pending/<push>.`.

**Verified live**: full conformance incl. `GE_CONFORMANCE_IMPORT=1` PASS
(import → commit → clone → fsck). In flight at handoff: 462k-object react
pack (1.1 GiB, 17 staged parts) — resolve phase crossing slices cleanly.

- **C2 verbatim consolidated-pack fast path** (`consolidated_pack` +
  `VerbatimStream` in `repo_do/mod.rs`): plain-clone-shaped fetch + exactly
  one live pack covering all wants → stream that R2 object verbatim, one
  `bucket.get`, `x-ge-subrequests: 1`.
- **C1 `packfile-uris`** (`pack_uri_response` + `sign.rs` + edge `pack_get`):
  opted-in clients (`fetch.uriprotocols`, git >= 2.40) instead get a signed
  `/_packs/<id>.pack?e=&r=&s=` URL — HMAC-SHA256 over `v1\nrepo_id\npack\nexp`
  keyed by `GE_URL_SIGNING_KEY`, 1h TTL. Inline `packfile` section is a legal
  empty pack; the `<hash>` token is the pack's real trailer SHA-1 (client
  verifies). Bandwidth fully bypasses the Worker.

**Latent bug C2 exposed and fixed**: `PackWriter::append_stored` hashed each
entry eagerly AND again at part upload, so every GC-consolidated pack stored
a wrong trailer. Walking-path fetches rebuild trailers and never noticed;
verbatim streaming made it visible (`index-pack`: SHA1 mismatch). One-line
fix in `store/mod.rs`; `GE_CONFORMANCE_GC=1` is the regression check.

Verified live: base suite + `GE_CONFORMANCE_GC=1 GE_CONFORMANCE_URIS=1` PASS
on wrangler dev — including a real git 2.55 clone over the signed URI.

## Remaining ceilings (post-I1)

| Wall | Evidence | Status |
|---|---|---|
| **Clone** 413 between 110k–263k objects | benchmark round 10 | **cleared** — react 263k in 18.7 s @ 1 subrequest (A29); URI-offloaded in 14.8 s (A30) |
| **Import** single commit > ingest budget (TS: 222k objects) | unsplittable-slice error | **cleared by I1** — server-side job slices the staged pack; 462k-object react pack in flight |
| **Multi-pack / pre-GC fetch** still walks the index | residual case neither A29 nor A30 covers | open — matters for the window between import and consolidation |

## Next work

- **Re-bench rails post-C2/C1** (787k objects — biggest cloneable repo) once
  consolidated; confirm the react number generalizes.
- **TypeScript via I1** — the original parity target: stage `ts-all.pack`,
  `/_admin/import`, confirm the 222k-object commit lands, then clone.
- P2 leftovers: #18 rename, #19 per-ref scopes, #20 domain/Access, #21 LFS,
  #22 no-walk clone — all deliberate defers, see ROADMAP.
- Then the WILD section (TTL repos, GitHub-URL import, synthetic-ref seeding).

## Environment / workflow

- Build: `cargo check --target wasm32-unknown-unknown` in `server/`
  (`worker-build` lives in `~/.cargo/bin`).
- Dev server: `cd server && wrangler dev --port 8794` (wrangler `--var` uses
  `KEY:VALUE` colon syntax, not `=`).
- Conformance: `tests/conformance/run.sh` against a running dev server; flags
  `GE_CONFORMANCE_GC=1 GE_CONFORMANCE_PURGE=1 GE_CONFORMANCE_LIMITS=1
  GE_CONFORMANCE_URIS=1`, `GE_REPO` for owner prefix, `EDGE_BASE`,
  `EDGE_TOKEN`. URIS needs `GE_URL_SIGNING_KEY` in `.dev.vars` and git >= 2.40
  as `GE_GITBIN` (Apple git 2.39 lacks packfile-uris; Homebrew git works).
- **Workflow rules**: commit/push/PR via `/atlas-engineering:commit-push-pr`;
  no PR template → fallback Summary/Changes/Testing format. Do NOT run
  `cargo fmt` globally — repo is not rustfmt-clean, it creates noise. Never
  put tokens in git URLs (credential helper / `GE_TOKEN` env only).
- Owner quota (50 repos default) accumulates across conformance runs — use a
  unique `GE_REPO` owner or expect registry-DO claim exhaustion.

## Local state on the old machine (won't transfer)

- `wrangler dev` on :8794 still running with imported repos (react at
  `packs_live:1` — handy for C2 verification; re-import takes ~6min via
  `tools/git-edge-import.sh` or `tests/bench/oss-bench.sh`).
- `/tmp/ts-bare.git` — TypeScript bare clone (2.8GB) for import-wall tests.
- `/tmp/ge-bench-*.log`, `/tmp/ge-bench-dev.log` — bench + worker logs.
- Two wedged Docker containers from AFS runs — `docker restart` daemon to
  reclaim.

## Backlog pointers

- `ROADMAP.md` — P2 #23/#24 are the clone fixes above; wild bucket has TTL
  repos, GitHub-URL import, synthetic-ref seeding (I2).
- Open research: upstream the artifact-fs `AFS_FSMONITOR_EXE` fsmonitor
  fork-bomb fix to `cloudflare/artifact-fs` (context in `findings/afs-e2e.md`).
