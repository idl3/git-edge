# git-edge handoff — 2026-09-15 (C2+C1 shipped)

Serverless Git smart-HTTP host: Rust/WASM Cloudflare Worker + per-repo SQLite
Durable Object + R2 packfiles. Repo: `idl3/git-edge`. Main is at `257184b`,
clean tree, all work merged (PRs #7–#16).

## Where things stand

All P0/P1 roadmap items are done except server-side import (#4b — deliberately
deferred). **C2 (A29) and C1 (A30) are done** — see PR #17:

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

## The two measured ceilings (next work)

| Wall | Evidence | Fix order |
|---|---|---|
| **Clone** dies at `Error::Budget`→413 between 110k–263k objects (vite ✓ / react ✗ / rails ✗). `blob:none` doesn't help — spend is R2 entry reads, sqlite is free | benchmark round 10 | ~~C2~~ ~~C1~~ — **re-bench** |
| **Import** can't stage a single commit with >ingest-budget objects (TypeScript: one commit = 222k objects) | `git-edge-import.sh` unsplittable-slice error | **I1 next** (I2 as interim) |

**Design doc: `findings/scale-ceilings.md`**. Next implementation is **I1
server-side import jobs**. Before that, re-bench react/rails post-C2: a
consolidated repo should now clone in tens of subrequests — confirm the wall
actually moved, and measure what pre-GC/multi-pack clones still cost (that's
the residual case neither C1 nor C2 covers).

### I1 pointers

- `POST /_admin/import {r2_key}` + resumable `import_pack` job on the
  alarm/jobs model (`jobs/mod.rs`, `purge_repo` is the proven shape); each
  alarm slice gets a fresh request budget.
- Client side: `tools/git-edge-import.sh` chunked `/_admin/import-part`
  uploads under the 100 MB cap (or S3 multipart to R2 staging).
- Reuse `pack/ingest.rs` resolve+normalize pipeline across slices —
  checkpointed like `gc_consolidate`'s `WriterCkpt`/`Pos` machinery.

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
