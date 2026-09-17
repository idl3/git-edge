# git-edge-jev-assess

Read-only operator-assessment endpoint for git-edge: `GET /<owner>/<repo>/_admin/assess`
composes a versioned `ge-snapshot/v1` document (state + refs + recent commit
subjects), asks TypeSafe's Jev model a fixed set of typed questions, and returns
typed answers plus a code-derived disposition.

- **Plan:** `~/.claude/plans/git-edge-jev-assess.md` (pass 2, confidence 93, autonomous)
- **Brainstorm:** `~/.claude/brainstorms/git-edge-extensions-jev.md`
- **Design:** `docs/design/git-edge-jev-assess.md`
- **Umbrella branch:** `feat/git-edge-jev-assess-integration`

## Phases

| Phase | Goal | Tracker | Status |
|---|---|---|---|
| A | `/_do/log` + `jev` client + `ge-snapshot/v1` + `/_admin/assess` + e2e smoke | [phase-a-tasks.md](./phase-a-tasks.md) | todo |
| B | `/_owner/list` + `/_admin/repos` + `tools/ge-sweep.sh` + docs | [phase-b-tasks.md](./phase-b-tasks.md) | todo |

## Out of scope

- Extension manifest, `on-push`/`on-schedule` bindings, Rules/Webhook evaluators.
- Auto-TTL, auto-pin, or any action taken on an assessment.
- Cron sweep worker + KV result store.
- Snapshot redaction flags beyond documentation.
