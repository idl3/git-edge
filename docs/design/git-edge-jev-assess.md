# Design — git-edge-jev-assess

TODO: fill prose. Rubric stubs imported from plan risk candidates
(`~/.claude/plans/git-edge-jev-assess.md`, pass 2).

## Threat model

- T1: Repo metadata (commit subjects, ref names, file-ext histogram) egresses to typesafe.ai — endpoint is admin-gated and opt-in per call; documented; redact flag deferred to evaluator #2. [known]
- T2: Jev down/slow → assess fails — 8s timeout, zero retries, response still carries the snapshot with `answers: null`; the git path is never touched. [known]
- T3: Agent-generated commit subjects yield garbage confidence — `inconclusive` disposition below confidence threshold; sweep output shows confidence. [assumed]
- T4: Assessing a tombstoned or mid-import repo reads transient state — `disposition()` checks `deleted`/`marked`/`packs_ingesting` first and returns `skip`. [assumed]

## Performance findings

- P1: Endpoint latency dominated by Jev RTT — target p95 <10s end-to-end; measured via sweep output. [assumed]
- P2: `/_do/log` walk bounded by `GE_LOG_MAX_SUBJECTS` (default 20) across all tips, deduped. [known]

## Simplicity findings

- S1: No extension manifest/bindings — questions are a `const`; the registry earns itself at evaluator #2. [known]
- S2: No answer caching — on-demand endpoint; caching earns itself only if sweep goes scheduled. [assumed]

## Principles & Seams

- `ge-snapshot/v1` is the extension seam — a versioned document; evaluators bind to the doc, not internals.
- `jev(state, questions) -> typed answers` is the vendor seam — all egress confined to `src/jev.rs`.

## Unwind cost

~6 files, additive-only, no git-path change, no data migration — delete the
assess route, `jev` module, `/_do/log`, `/_owner/list`, and `ge-sweep.sh`.
