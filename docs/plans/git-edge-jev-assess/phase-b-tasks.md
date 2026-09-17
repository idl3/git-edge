---
feature: git-edge-jev-assess
phase: b
tier: feature
autonomous: true
umbrella-branch: feat/git-edge-jev-assess-integration
complexity-budget:
  files: 5
  loc-delta: 120
adopted-patterns:
  - stub_json / _do/* proxy convention (edge/mod.rs, repo_do/mod.rs)
  - owner! claim:<repo> meta keys (repo_do/mod.rs)
  - tools/*.sh curl + credential-helper CLI style
---

# Phase B — sweep + docs

> **In plain English**: make the assessment usable at fleet scale — list an
> owner's repos, loop `/_admin/assess` over them from a shell script, and write
> down the endpoint contract and the data-egress note.
> **Scope**: `server/src/` owner-list route, `tools/ge-sweep.sh`, docs.
> **Design**: docs/design/git-edge-jev-assess.md
> **Branch**: feat/git-edge-jev-assess-phase-b (targets `feat/git-edge-jev-assess-integration`)

<!-- CP0 log: emitted by /100x:commit-plan from ~/.claude/plans/git-edge-jev-assess.md pass 2. B6 elided: Out of scope (README carries it). Codex: n/a-override carried. -->

## Status

| Task | Status |
|---|---|
| B1 | todo |
| B2 | todo |
| B3 | todo |
| — done | — |

## Audit item coverage

| Audit item | Tasks | Reuse ref |
|---|---|---|
| T1 metadata egress | B3 | documented egress note |
| S1 no registry | B2 | fixed question set consumed, not managed |

## Task list

### B1 — `GET /_owner/list` + `GET /<owner>/_admin/repos`

> **Goal**: the `owner!` registry DO returns the repo names it already tracks, exposed through the Worker as an admin route.
> **Files**: server/src/repo_do/mod.rs, server/src/edge/mod.rs
> **Acceptance**: `GET /<owner>/_admin/repos` (global write token) returns `{ "repos": [...] }` sourced from `SELECT key FROM meta WHERE key LIKE 'claim:%'`; empty owner returns `[]`.
> **Verification**: `cd server && cargo test`
> **Depends on**: none
> **Reversibility**: clean-revert

- DO side: one match arm in `owner_route` beside `/_owner/claim`; strip the `claim:` prefix in the response.
- Edge side: new route + `owner!` stub lookup mirroring `owner_claim` (`edge/mod.rs:754-772`).

### B2 — `tools/ge-sweep.sh`

> **Goal**: one shell command sweeps an owner's repos and prints a disposition-sorted triage table (`disposition | confidence | repo`).
> **Files**: tools/ge-sweep.sh
> **Acceptance**: `GE_TOKEN=... tools/ge-sweep.sh <owner>` lists repos via `/_admin/repos`, calls `/_admin/assess` per repo serially, prints the sorted table; repos with `answers: null` show as `unavailable` and don't abort the sweep.
> **Verification**: `tools/ge-sweep.sh <owner>` against a local `wrangler dev` with ≥2 pushed fixture repos
> **Depends on**: B1, A4
> **Reversibility**: clean-revert
> **E2E test**: the script itself against local dev; skip with `[e2e:skipped] reason:` when no dev server is reachable.

- Serial by default; document `xargs -P` for parallelism (assumption, pass 2).
- Credential discipline per `tools/git-edge-import.sh` — `GE_TOKEN` env, never in URLs.

### B3 — docs: contract + schema + egress note

> **Goal**: COMPATIBILITY.md documents `/_admin/assess` + `/_admin/repos` and the `ge-snapshot/v1` schema; README and the agent skill mention the endpoints and that metadata egresses to typesafe.ai.
> **Files**: COMPATIBILITY.md, README.md, .devin/skills/git-edge/SKILL.md
> **Acceptance**: docs name the auth level (global write token), the response shape, the snapshot schema field list, and the third-party egress fact — an operator can learn the feature exists without reading code.
> **Verification**: docs review — grep for `_admin/assess`, `ge-snapshot`, `typesafe` in all three files
> **Depends on**: A4, B1, B2
> **Reversibility**: clean-revert

## Dependencies between tasks

- B1 → B2 → B3 linear; B2 also depends on A4 (phase A).

## Review sign-off checklist

- [ ] Sweep tolerates a failing/unavailable assess on one repo without aborting
- [ ] Docs state plainly that commit subjects + ref names leave the deployment for typesafe.ai
- [ ] `ge-snapshot/v1` field list in docs matches the pinned schema test
