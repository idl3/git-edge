---
name: git-edge
description: Use when cloning from, pushing to, importing into, or managing repositories on a git-edge deployment — the serverless Git smart-HTTP host (Cloudflare Worker + per-repo SQLite Durable Object + R2 packfiles). Covers URL shape, credential setup, per-repo token minting, the staged-push recipe for large imports, the public/delete/pin/export lifecycle, _state introspection, and known limits.
---

# git-edge

Serverless Git smart-HTTP host. A repo is created by pushing to it — there is no
provisioning step, no web UI, no hooks. Protocol v2 fetch (git ≥ 2.26 required),
v0 receive-pack.

## URL shape

```
https://<host>/<owner>/<repo>
```

e.g. `http://localhost:8787/acme/webapp` or
`https://git-edge.<acct>.workers.dev/acme/webapp`. The repo does not exist until
the first push lands.

## Credentials — never put tokens in URLs

`https://user:token@host/...` embeds the token in `git` argv — visible to every
user on the machine via `ps`. Use a credential helper or askpass instead:

```bash
export GE_TOKEN=ge_...           # or the deployment's write token
git -c credential.helper= \
    -c 'credential.helper=!f() { echo username=edge; echo "password=$GE_TOKEN"; }; f' \
    clone https://<host>/<owner>/<repo>
```

or persist per-host once:

```bash
printf 'protocol=https\nhost=<host>\nusername=edge\npassword=%s\n' "$GE_TOKEN" | git credential approve
```

The password is the token; the username is only a label (it is recorded as the
reflog principal). `GIT_ASKPASS` equivalent: point it at a script that prints
`$GE_TOKEN`. For curl/scripts, `Authorization: Bearer <token>` also works.

## Per-repo tokens

The deployment-wide secrets (`GE_READ_TOKEN` / `GE_WRITE_TOKEN` in `.dev.vars`)
are admin credentials. Mint scoped per-repo tokens with the global write token:

```bash
# create — token value shown ONCE: {"id","token":"ge_<64hex>","level","name"}
curl -u "edge:$GE_ADMIN_TOKEN" -X POST \
  https://<host>/<owner>/<repo>/_admin/tokens \
  -d '{"name":"ci","level":"write"}'        # level: read | write

curl -u "edge:$GE_ADMIN_TOKEN" https://<host>/<owner>/<repo>/_admin/tokens   # list (ids, never secrets)
curl -u "edge:$GE_ADMIN_TOKEN" -X DELETE \
  https://<host>/<owner>/<repo>/_admin/tokens/<id>                           # revoke
```

Only the global write token can mint; a repo token cannot mint more tokens. A
`read` token clones/fetches but gets 403 on push. Cap: 256 tokens per repo.

## Repo lifecycle (global write token)

```bash
R=https://<host>/<owner>/<repo>

# anonymous read toggle — clone/fetch/export need no credential; pushes stay
# token-gated either way. NOTE: a presented-but-bad token still gets 401, so a
# stale credential in a helper breaks "anonymous" access — clone truly
# anonymously with `git -c credential.helper= clone $R`
curl -u "edge:$GE_ADMIN_TOKEN" -X POST $R/_admin/public -d '{"enabled":true}'

# export: v3 git bundle of all live refs (read-level auth; anonymous if public)
curl -u "edge:$GE_TOKEN" $R/_admin/export -o repo.bundle
git clone repo.bundle dst          # bundles are clonable directly

# pin/unpin a ref — pushes to a pinned ref get `ng "ref is pinned"`
curl -u "edge:$GE_ADMIN_TOKEN" -X POST $R/_admin/pin \
  -d '{"ref":"refs/heads/main","sha":"<sha>"}'
curl -u "edge:$GE_ADMIN_TOKEN" -X POST $R/_admin/unpin -d '{"ref":"refs/heads/main"}'

# delete: tombstones immediately (every route answers 410), a purge_repo job
# then wipes DO storage + R2 packs; the name is reusable once purge lands
curl -u "edge:$GE_ADMIN_TOKEN" -X POST $R/_admin/delete
```

## Importing a repo larger than ~80 MiB — staged push

The Cloudflare zone caps each request body at ~100 MB, so one giant push fails
with HTTP 413. Push commit ranges oldest→newest instead; each push carries only
the objects introduced in that range:

```bash
git clone --bare <src> src.git
BR=$(git -C src.git symbolic-ref --short HEAD)   # usually main or master
git -C src.git rev-list --first-parent --reverse "$BR" > commits
# pick boundary shas so each slice stays under ~60 MiB of new object bytes and
# well under ~100k objects (the per-request wall clock is 240 s; ~10k objects
# took ~20 s in benchmarks)
git -C src.git push "$REMOTE" "$(sed -n "${HI}p" commits):refs/heads/$BR"
# ...repeat oldest→newest; last push must be the branch tip
```

`--first-parent` matters: it makes every boundary sha a strict descendant of
the previous one, so each push is a clean fast-forward. Plain `--topo-order`
can land a boundary on a merge's side-branch commit — pushing it moves the ref
sideways and is rejected non-fast-forward.

`tools/git-edge-import.sh` automates this slice-and-push loop (plus resume,
`--all-branches`, `--dry-run`, and 429 retry):

```bash
GE_TOKEN=<write-token> tools/git-edge-import.sh <local-repo-or-url> <edge-url>
```

Then verify: clone back and `git fsck --strict`. Old repos may carry
pre-existing fsck findings (e.g. `zeroPaddedFilemode`) — compare against fsck
of the source, not against empty output.

## Introspection

```bash
curl -u "edge:$GE_TOKEN" https://<host>/<owner>/<repo>/_state   # any write-level credential
```

Write-gated (a repo `write` token suffices; `_admin/*` routes do not — they
want the global write token). Returns `refs`, `objects`, `packs_live`/`packs_ingesting`/
`packs_dead`, `pushes`, `jobs_queued`/`jobs_running`/`jobs_dead`, `marked`,
`tokens`, `pins` (list of `{ref,sha}`), `public`, `deleted`, `rate_rows`,
`refs_memo_hits`/`refs_memo_misses`. `jobs_dead > 0` means a maintenance job
wedged. Pushes are CAS; a stale push is rejected — refetch and retry, don't
`--force` blindly.

## Known limits (clean errors, not corruption)

| Limit | Value | Symptom / workaround |
|---|---|---|
| Request body per push | ~100 MB (zone) | HTTP 413 → stage the push (above) |
| Per-request budget | 9,000 subrequests / 240 s | `unpack request budget exhausted` → smaller slices |
| Inflated non-blob object / delta result | 16 MiB | `object too large (16 MiB max)` / `delta base <oid> exceeds 16 MiB` → `git -c core.bigFileThreshold=1 push` sends it as a full blob. **`git push --no-thin` does NOT prevent REF_DELTA on git 2.54** |
| Full blob (pushed whole) | ~2 GiB | streamed verbatim — fine |
| Fetch walk | 200k commits / 1M objects | `ERR fetch too large for this server; clone instead` |
| Push links (tree edges) per push | 1,000,000 | `push references too many objects` → smaller slices (seen on repos with giant trees) |
| Push rate | 30/min per credential per repo (`GE_RATE_PUSHES_PER_MIN`) | HTTP 429 + `Retry-After` → wait and retry |
| Quotas | 2M objects / 4 GiB stored per repo, 50 repos/owner (`GE_QUOTA_MAX_*`) | `unpack objects N > GE_QUOTA_MAX_OBJECTS=M` etc. — the message names the cap |
| Full clone scale | ~110k objects verified, ~260k fails | `fetch request budget exhausted` → HTTP 413 mid-walk — index reads (commits+trees) spend the budget, so `--filter=blob:none` does NOT help. Imports past this size work; only the clone back fails |
| Client floor | git ≥ 2.26 | v0/v1 fetch → `ERR protocol v2 required` |

Each push lands as its own pack; GC consolidates after a ~10 min quiet window
(`GE_GC_QUIET_MS`). Give a fresh staged import a few minutes before cloning.

No LFS, no `--atomic`, no push-options, no hooks, no rename. GC is automatic
(mark→consolidate→sweep via DO alarms, 1 h grace before R2 deletion).

Full reference: `COMPATIBILITY.md`; evidence: `findings/implementation.md`;
end-to-end example: `tests/conformance/run.sh`.
