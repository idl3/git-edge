# Adversarial audit brief — git-edge server

You are attacking a production Rust→wasm32 git smart-HTTP server. Your job is to find
REAL bugs — exploitable, corrupting, or spec-violating — not style nits.

## The system
- `server/src/` (~4,900 lines): Cloudflare Worker (`edge/`, `auth/`, `wire/`),
  one SQLite Durable Object per repo (`repo_do/`, `jobs/`, `store/` — SQLite index +
  DO output-gate atomicity), R2 for pack bytes (`pack/`, multipart writers).
- `CONTRACTS.md` is the binding spec (sections + A1-A19 amendments).
- `tests/conformance/run.sh` is the known-good path — attack what it does NOT cover.
- Live dev server may be running at http://test:write-test-token@localhost:8787
  (Basic auth; read token `read-test-token`). Try live probes; if exec is denied,
  report the attack as a documented reproducer instead.
- DO semantics to exploit: each fetch() is single-threaded but awaits yield between
  DO events; writes commit when the handler returns a response (Err propagating as an
  exception rolls back). Storage/Internal errors are deliberately propagated for that.

## What counts as a finding
A concrete attack or input sequence with: file:line, the violated invariant, expected
vs actual behavior, and a reproducer (curl/python/git command or precise reasoning).
Severity: CRITICAL (data loss/corruption/auth bypass), HIGH (DoS, wrong data to client),
MEDIUM (spec violation git tolerates), LOW.

## Rules
- Do NOT modify server/src. Write your report to findings/audit/<your-surface>.md.
- Every claim must cite file:line you actually read. No speculation without a trace.
- Report "no finding" honestly — an area that survives attack is a result too.
