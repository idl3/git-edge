# git-edge handoff — 2026-09-15 (C2 shipped)

Serverless Git smart-HTTP host: Rust/WASM Cloudflare Worker + per-repo SQLite
Durable Object + R2 packfiles. Repo: `idl3/git-edge`. Main is at `257184b`,
clean tree, all work merged (PRs #7–#15).

## Where things stand

All P0/P1 roadmap items are done except server-side import (#4b — deliberately
deferred). **C2 (verbatim consolidated-pack fast path, ROADMAP #23) is done**:
a plain-clone-shaped fetch whose wants resolve into the repo's single live
pack streams that pack verbatim — one R2 GET, `x-ge-subrequests: 1` —
gated on empty haves and no filter/shallow/deepen args so a superset pack
can't violate those contracts. See `repo_do/mod.rs` `consolidated_pack` +
`VerbatimStream`; CONTRACTS A29.

**Latent bug C2 exposed and fixed**: `PackWriter::append_stored` hashed each
entry eagerly AND again at part upload (flush/checkpoint/finish are the only
feeders by design), so every GC-consolidated pack stored a wrong trailer.
Walking-path fetches never saw it (they rebuild the trailer); the verbatim
path made it visible — `git fsck` caught it as an SHA1 mismatch. One-line
fix in `store/mod.rs`; `GE_CONFORMANCE_GC=1` is the regression check
(post-GC clone + fsck through C2).

Verified live: full suite + `GE_CONFORMANCE_GC=1` PASS — post-GC repo at
`packs_live:1` clones at 1 subrequest and passes `fsck --strict`.

## The two measured ceilings (next work)

| Wall | Evidence | Fix order |
|---|---|---|
| **Clone** dies at `Error::Budget`→413 between 110k–263k objects (vite ✓ / react ✗ / rails ✗). `blob:none` doesn't help — spend is R2 entry reads, sqlite is free | benchmark round 10 | ~~C2~~ **C1 next** |
| **Import** can't stage a single commit with >ingest-budget objects (TypeScript: one commit = 222k objects) | `git-edge-import.sh` unsplittable-slice error | **I1** (I2 as interim) |

**Design doc: `findings/scale-ceilings.md`** — read first. Next
implementation is **C1 (packfile-uris)** with signed `/packs/<key>` URLs:
offloads the pack bytes off the Worker's subrequest/wall-clock budget
entirely for clients that advertise the capability (git >= 2.41ish,
off by default — most clients still take the C2 path). Then **I1**
server-side import jobs.

### C1 pointers

- C2's `consolidated_pack` gate + pack lookup is the same coverage test C1
  wants — a `packfile-uris` capable request advertises the URI instead of
  streaming.
- Response shape: `wire::write_fetch_prelude` + a `packfile-uris` section
  before `packfile` (protocol v2 fetch section ordering — check git docs).
- Signing: see design doc for the `/packs/<key>` URL scheme; tokens stay
  out of URLs, sign server-side.
- Re-bench react/rails post-C2 first — C2 may already clear the measured
  wall for consolidated repos; C1's remaining value is pre-GC and
  multi-pack cases.

## Environment / workflow

- Build: `cargo check --target wasm32-unknown-unknown` in `server/`
  (`worker-build` lives in `~/.cargo/bin`).
- Dev server: `cd server && wrangler dev --port 8794` (wrangler `--var` uses
  `KEY:VALUE` colon syntax, not `=`).
- Conformance: `tests/conformance/run.sh` against a running dev server; flags
  `GE_CONFORMANCE_GC=1 GE_CONFORMANCE_PURGE=1 GE_CONFORMANCE_LIMITS=1`,
  `GE_REPO` for owner prefix, `EDGE_BASE`, `EDGE_TOKEN`.
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
