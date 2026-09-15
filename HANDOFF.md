# git-edge handoff — 2026-09-15

Serverless Git smart-HTTP host: Rust/WASM Cloudflare Worker + per-repo SQLite
Durable Object + R2 packfiles. Repo: `idl3/git-edge`. Main is at `257184b`,
clean tree, all work merged (PRs #7–#15).

## Where things stand

All P0/P1 roadmap items are done except server-side import (#4b — deliberately
deferred). Shipped this session: repo delete+purge, public read, ref pinning,
bundle export, job metrics + dead-job alerting, lease fencing, quotas, rate
limits, ls-refs memoization, `x-ge-subrequests` on errors, agent skill
(`.devin/skills/git-edge/SKILL.md`), `tools/git-edge-import.sh`,
`tests/bench/oss-bench.sh`. artifact-fs e2e: 23 pass / 0 fail / 3 skip —
protocol-compatible today.

## The two measured ceilings (next work)

| Wall | Evidence | Fix order |
|---|---|---|
| **Clone** dies at `Error::Budget`→413 between 110k–263k objects (vite ✓ / react ✗ / rails ✗). `blob:none` doesn't help — spend is R2 entry reads, sqlite is free | benchmark round 10 | **C2 then C1** |
| **Import** can't stage a single commit with >ingest-budget objects (TypeScript: one commit = 222k objects) | `git-edge-import.sh` unsplittable-slice error | **I1** (I2 as interim) |

**Design doc: `findings/scale-ceilings.md`** — read first. Recommended next
implementation is **C2 (verbatim consolidated-pack fast path)**: when
`packs_live:1` covers the full-clone send set, stream that pack verbatim
instead of per-object `read_entries`. Verified premise: react sits at one live
pack post-GC. Then **C1 (packfile-uris)** with signed `/packs/<key>` URLs.

### C2 pointers

- Fetch path: `server/src/repo_do/mod.rs` `fetch_v2_inner` →
  `generate::send_set` → `FetchStream` over `set.reads`.
- Budget: `server/src/lib.rs` `ReqBudget` — `charge(1)` per R2 op;
  `PAID_SUBREQUESTS = 9000`, 240s.
- Gate to plain full clone only: wants=tip, empty haves, no filter/shallow/
  deepen args. Superset objects are legal but must not violate shallow/filter
  contracts.
- Stream via `bucket.get(key)` → `body` — ~1 subrequest regardless of size.

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
