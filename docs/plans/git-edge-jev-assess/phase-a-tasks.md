---
feature: git-edge-jev-assess
phase: a
tier: feature
autonomous: true
umbrella-branch: feat/git-edge-jev-assess-integration
complexity-budget:
  files: 4
  loc-delta: 400
adopted-patterns:
  - env.secret() credential reads (auth/mod.rs)
  - stub_json / _do/* proxy convention (edge/mod.rs, repo_do/mod.rs)
  - env_i64 GE_* config knobs (repo_do/mod.rs)
  - NEW: worker outbound fetch for vendor egress (no existing precedent)
---

# Phase A — snapshot + assess endpoint

> **In plain English:** build the read-only `/_admin/assess` endpoint — gather a
> repo's stats and recent commit subjects into one versioned document, ask the
> Jev decision model a fixed set of questions about it, and return the answers
> plus a triage label.
> **Scope:** `server/src/` only — new `/_do/log` DO route, new `jev` client
> module, snapshot composer, assess route, and an e2e smoke script.
> **Design:** docs/design/git-edge-jev-assess.md
> **Branch:** feat/git-edge-jev-assess-phase-a (targets `feat/git-edge-jev-assess-integration`)

<!-- CP0 log: emitted by /100x:commit-plan from ~/.claude/plans/git-edge-jev-assess.md pass 2. B6 elided: none (5 tasks + populated rubric). Codex: n/a-override (plan pass 1–2). -->

## Status

| Task | Status |
|---|---|
| A1 | done |
| A2 | todo |
| A3 | todo |
| A4 | todo |
| A5 | todo |
| — done | — |

## Audit item coverage

| Audit item | Tasks | Reuse ref |
|---|---|---|
| T1 metadata egress | A4 | documented in B3; admin-gated |
| T2 Jev down/slow → answers:null | A2, A4 | `secret_opt` absent-key shape |
| T3 noisy subjects → inconclusive | A4 | confidence threshold in `disposition()` |
| T4 tombstoned/mid-import → skip | A3, A4 | `marked`/`deleted`/`packs_ingesting` fields from `/_do/state` |
| P1 p95 <10s | A2, A4 | `GE_JEV_TIMEOUT_MS` via `env_i64` pattern |
| P2 bounded log walk | A1 | `GE_LOG_MAX_SUBJECTS` via `env_i64` pattern |
| S1 no manifest/DSL | A4 | `ASSESS_QUESTIONS` const |
| S2 no answer caching | — | deferred by design |

## Task list

### A1 — `/_do/log`: recent commit subjects across all ref tips

> **Goal**: the repo DO serves a bounded list of recent commit subjects (`{sha, subject}`) covering every live branch tip, deduped, newest-first.
> **Files**: server/src/repo_do/mod.rs
> **Acceptance**: `GET /_do/log` returns ≤ `GE_LOG_MAX_SUBJECTS` (default 20) subjects unioned over all `refs/heads/*` tips; a repo whose HEAD sits on a stale branch still surfaces subjects from other tips; empty repo returns `[]` not an error.
> **Verification**: `cd server && cargo test` and `cargo check --target wasm32-unknown-unknown`
> **Depends on**: none
> **Reversibility**: clean-revert

- Add `env_i64("GE_LOG_MAX_SUBJECTS", 20)` knob beside the existing `gc_quiet_ms`/`env_ms` helpers.
- Union-walk every live `refs/heads/*` tip via `gix-traverse`; parse subjects via `gix-object`; dedupe by object id; stop at the cap.
- Route in the `/_do/*` match set beside `/_do/state` (GET).

<details><summary>Mechanics</summary>

Commits live in packs + loose objects already readable through the store layer
(`gix-pack`, `gix-object` are in `Cargo.toml`). The walk is a bounded BFS over
parent edges from each tip; a `HashSet<ObjectId>` handles dedupe. Subject = first
line of the commit message. Order by committer timestamp descending where
available; tip order is a fine tiebreak.

</details>

### A2 — `jev` module: typed System One client

> **Goal**: a `jev::evaluate(state, questions)` function POSTs one fan-out request to `api.typesafe.ai/v1/systemone` and returns typed answers (choice/score/noul + probabilities + confidence) or a structured error.
> **Files**: server/src/jev.rs, server/src/lib.rs
> **Acceptance**: request serializes to the System One wire shape (`state`, `model`, `questions`); response deserializes `answers` per question id; timeout (`GE_JEV_TIMEOUT_MS`, default 8000) and HTTP errors return `Err` — never panic, never retry.
> **Verification**: `cd server && cargo test` (serde round-trip + error mapping against a canned response body)
> **Depends on**: none
> **Reversibility**: clean-revert

- Reads `env.secret("TYPESAFE_API_KEY")` via the `secret_opt` shape in `auth/mod.rs:19`; absent key → typed `Unavailable` error, not a panic.
- First use of outbound `worker::Fetch` in this codebase — keep ALL egress inside this module so the seam stays narrow.
- Zero retries; the caller treats failure as `answers: null`.

### A3 — `enrich()`: `ge-snapshot/v1` composer

> **Goal**: a pure function combines `/_do/state` + `/_do/log` output into the versioned `ge-snapshot/v1` JSON document (schema field set pinned by test).
> **Files**: server/src/edge/assess.rs, server/src/edge/mod.rs
> **Acceptance**: output carries `schema: "ge-snapshot/v1"`, state counters, refs, `subjects[]`, and a file-extension histogram; a unit test pins the field set so accidental schema drift fails loudly.
> **Verification**: `cd server && cargo test`
> **Depends on**: A1
> **Reversibility**: load-bearing — this document is the extension seam; fields are additive-only from here on.

- Snapshot includes `marked`, `deleted`, `packs_ingesting` verbatim — the disposition combiner needs them for the `skip` short-circuit.
- File-ext histogram derives from the root tree of the default branch where cheap; empty on failure — it is a hint, not a gate.

### A4 — `GET /_admin/assess`: route + questions + disposition

> **Goal**: an admin-gated endpoint returns `{ snapshot, answers, disposition, questions_version }`; with Jev unreachable or unconfigured it returns HTTP 200 with `answers: null` and an `error` field.
> **Files**: server/src/edge/mod.rs, server/src/edge/assess.rs
> **Acceptance**: `authenticate_admin` gates the route (same as `/_admin/export`); recorded Jev fixture produces a deterministic disposition; missing key → `answers: null` + `error`; `deleted`/`marked`/mid-import snapshot → `disposition: "skip"` without calling Jev.
> **Verification**: `cd server && cargo test` (fixture + tombstone + missing-key cases)
> **Depends on**: A2, A3
> **Reversibility**: clean-revert

- `ASSESS_QUESTIONS` const + `QUESTIONS_VERSION` string stamped on every response (decision D8).
- `disposition()` is plain code over typed answers: `skip` → `investigate` (abuse >0.9 & conf >0.7) → `page-operator` (health <0.5) → `ttl-candidate` (disposable >0.8) → `inconclusive` (kind conf <0.5) → `ok`.
- Register in the route match beside `state_probe`/`token_create`; admin-surface label `"admin"`.

### A5 — e2e smoke: `tests/e2e/assess.sh`

> **Goal**: a script boots the worker locally, pushes a fixture repo, curls `/_admin/assess`, and asserts the snapshot + `answers: null` shape (no API key needed).
> **Files**: tests/e2e/assess.sh
> **Acceptance**: exits 0 with asserted JSON shape on a live `wrangler dev`; prints `[e2e:skipped] reason:` and exits 0 when wrangler/miniflare is absent.
> **Verification**: `tests/e2e/assess.sh`
> **Depends on**: A4
> **Reversibility**: clean-revert
> **E2E test**: the script itself — it IS the operator-surface check.

- Follow `tools/git-edge-import.sh` conventions: `GE_TOKEN` env, credential-helper discipline, no tokens in URLs.

## Dependencies between tasks

- A1, A2 are independent — parallel-safe.
- A3 needs A1 (log payload shape).
- A4 needs A2 + A3.
- A5 needs A4.

## Out of scope

> **Out of scope:** see [README](./README.md#out-of-scope). Additionally: no
> answer caching, no per-call question overrides, no `/_admin/repos` listing
> (that is Phase B).

## Review sign-off checklist

- [ ] `ge-snapshot/v1` field set pinned by test and additive-only going forward
- [ ] Jev egress confined to `src/jev.rs`; zero retries; 8s cap
- [ ] Tombstoned/mid-import repos never reach Jev (`skip` path tested)
- [ ] Missing `TYPESAFE_API_KEY` is a clean `answers: null`, not a 5xx
