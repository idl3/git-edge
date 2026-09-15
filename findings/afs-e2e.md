# artifact-fs e2e against git-edge

We ran Cloudflare's artifact-fs end-to-end suite against a local git-edge
(`wrangler dev`). Verdict first: **git-edge already speaks everything
artifact-fs needs.** The full FUSE suite passes — 23 tests, 0 failures —
plus a manual commit-push-pull cycle through the live mount.

The only code-level surprise was in artifact-fs itself, not git-edge: its
e2e suite self-destructs under `go test` (details below). One local
workaround was needed to run it.

## Environment

- Host: macOS arm64, Docker Desktop 29.6.2, no macFUSE, no Go toolchain.
- git-edge: `wrangler dev --port 8795` serving the prebuilt worker
  (`build/worker/shim.mjs` from the main checkout; `worker-build` not
  needed since no source changes). `.dev.vars` copied in; tokens
  `GE_READ_TOKEN`/`GE_WRITE_TOKEN` as usual.
- FUSE runs in a Linux container: `docker run --device /dev/fuse
  --cap-add SYS_ADMIN --security-opt apparmor:unconfined`. Direct
  `mount(2)` as root works; `fusermount3` (Debian `fuse3`) is installed
  as the fallback path jacobsa/fuse wants.
- Image: `golang:1.26-bookworm` + `fuse3 git ca-certificates`, repo
  copied in, `go build ./cmd/artifact-fs`. Note the repo's own
  `.dockerignore` excludes the root `e2e*_test.go` files — bind-mount
  the clone (`-v /tmp/artifact-fs:/src`) when running tests.
- Containers reach the dev server at `host.docker.internal:8795`.
- artifact-fs @ `2b87a48` (main, 2026-09).

## Test repo

`afs/test`, seeded to mirror `createLocalTestRepo` exactly (the suite
asserts on specific paths): README.md / LICENSE-MIT / SECURITY.md,
`package.json`, `packages/{wrangler,miniflare,vitest-pool,workers-shared}`,
a symlink (`manifest-link`), an exec bit (`bin/run.sh`), a gitlink
(`vendor/dependency`, mode 160000 — push accepted), plus ~5.4 MB of blobs
(`big.bin` 4 MiB random, `data/large.txt` ~2.3 MB) to exercise lazy
hydration. 6 commits on `main`.

Seeding gotcha: `git add -A` after `update-index --cacheinfo` *deletes*
the gitlink (no on-disk dir) — commit it last or re-add it.

## Credentials

Two working patterns:

- **Inline** `http://test:<token>@host/afs/test` — allowed for sync
  `add-repo`; artifact-fs strips the creds and injects a one-shot
  `GIT_CONFIG_*` credential helper (see `credentialEnv`,
  internal/gitstore/gitstore.go:1976).
- **Ambient** (needed for `--async` and `--prepared-gitdir`, which reject
  inline creds): `git config --global credential.helper store` +
  `~/.git-credentials` containing `http://test:<token>@host:8795`. This
  is also what makes in-mount `git` operations (promisor backfill, push)
  authenticate, since `remote.origin.url` stores the stripped URL.

Per-repo tokens minted via `POST /afs/test/_admin/tokens` (gated on the
global write token) worked for both levels: `read` for clone/hydration,
`write` for the push test.

## Suite result (verbatim)

```
AFS_RUN_E2E_TESTS=1 \
AFS_E2E_REPO="http://host.docker.internal:8795/afs/test" \
AFS_FSMONITOR_EXE=/usr/local/bin/artifact-fs \
go test -v -run "TestE2E|TestFUSEMountSmoke" -count=1 -timeout 30m .
```

| Test | Result |
|---|---|
| TestE2EAsyncPreparedGitDirBlocksUntilReady | PASS (1.75s) |
| TestE2EAsyncPreparedGitDirFailureThenRetry | PASS (1.14s) |
| TestE2EBenchmarkRepos | SKIP (needs AFS_RUN_E2E_BENCH=1) |
| TestE2EGitCleanState | PASS (1.61s) |
| TestE2EGitStatusDetectsSameSizeRewriteAfterMtimeRestore | PASS |
| TestE2EGitStatusPorcelain | PASS |
| TestE2EGitRenameTrackedDirectory | PASS |
| TestE2EGitCheckoutBranchSwitch | PASS |
| TestE2EGitCheckoutConflictKeepsLocalChanges | PASS |
| TestE2EGitCheckoutRestorePath | PASS |
| TestE2EGitMergeAndRebase | PASS |
| TestE2EGitCommitModifyTrackedFile | PASS |
| TestE2EGitCommitPreservesUnstagedAfterStagedCommit | PASS |
| TestE2EGitCommitTrackedRename | PASS |
| TestE2EGitCommitTrackedDelete | PASS |
| TestE2EGitCommitExecutableBit | PASS |
| TestE2EGitCommitSymlink | PASS |
| TestE2EGitResetHardCleanAndStash | PASS |
| TestE2EGitPullFastForward | SKIP (skipped by design under AFS_E2E_REPO; covered manually below) |
| TestE2EGitPush | SKIP (same; covered manually below) |
| TestFUSEMountSmoke | PASS |
| TestE2E (26 fs/* and git/* subtests: ls, cat, stat, mkdir, rename, unlink, truncate, log, diff, add, reset, status, commit) | PASS (1.37s) |
| TestE2EFilesystemDirectoryMoveWorkflows (6 subtests) | PASS |
| TestE2EFilesystemDirectoryRenamePersistsAcrossRestart | PASS |
| TestE2EFilesystemDirectoryRenameConcurrentAccess | PASS |
| TestE2EVerifiedSource (`--require-commit` acquisition) | PASS |

`ok github.com/cloudflare/artifact-fs 43.434s`

## Manual coverage of the skipped ops

The suite skips push/pull against a real remote. Done by hand with the
write-level per-repo token:

- `git push origin main` through the mount: `5ce4958..7834fab  main ->
  main`, RC=0 — receive-pack works end-to-end through artifact-fs.
- `git pull --ff-only` through the mount after a host-side push:
  `20bf627..e2afb8d  main -> origin/main`, fast-forward applied and the
  new file hydrated and readable.
- `artifact-fs status --name afs-test` reports `state=mounted`,
  `hydrated_blobs=13 hydrated_bytes=6294802`, `prepare_error=none`.
- `artifact-fs list-repos`, `daemon`, `unmount` paths exercised via CLI.

## Protocol evidence on the wire (GIT_TRACE)

- Clone: `git clone --filter=blob:none --no-checkout --single-branch
  --branch main` → v2 `fetch` with `filter blob:none` → thin pack
  indexed with `--promisor`.
- Refresh: `git fetch --filter=blob:none --no-tags origin
  +refs/heads/main:refs/remotes/origin/main` (~30 ms warm).
- Lazy hydration: `git fetch origin --no-tags --no-write-fetch-head
  --recurse-submodules=no --filter=blob:none --stdin` — the promisor
  backfill path for arbitrary object IDs (allowAnySHA1InWant). Served
  correctly; per-request latency 25–400 ms in `wrangler dev`.
- Explicit arbitrary-sha fetch from the host:
  `git fetch origin f2df2381...` → `cat-file -t` → `commit`. Works.

## Worker log audit

Across ~19k requests in the dev log: `GET info/refs` 401→200 pairs (the
normal basic-auth challenge dance — git sends the first request
unauthenticated, gets `WWW-Authenticate`, retries with the credential
helper) and `POST git-upload-pack`/`git-receive-pack` 200s. **No non-200
git-protocol responses in the clean runs.** A handful of `ProxyWorker:
Network connection lost` entries appear only around the fork-bomb
container teardown (see below) — client-side aborts, not server errors.

## Two upstream artifact-fs issues found (not git-edge)

1. **The e2e suite fork-bombs under `go test`.**
   `ConfigureStatusOptimization` writes an fsmonitor hook script that
   execs `os.Executable()` (internal/gitstore/gitstore.go:1856). Under
   `go test` that is the *test binary*; `git status` in the mount invokes
   the hook → the test binary re-runs the entire suite → each new mount's
   `git status` spawns another suite. Hundreds of mounts/clones within
   minutes; the first run "hung" for 19 min (test timeout) purely from
   self-inflicted load, and the container could not even be killed
   (wedged FUSE tasks — Docker Desktop can't reap them without a daemon
   restart). This will reproduce on any platform, including macOS/macFUSE
   — the suite appears to have never been run since the fsmonitor hook
   landed.

   **Workaround used** (3-line local patch, in the throwaway clone only):
   honor `AFS_FSMONITOR_EXE` in `ConfigureStatusOptimization`, pointed at
   the real `/usr/local/bin/artifact-fs`. This keeps the real fsmonitor
   code path exercised — the hook ran on every `git status` in the suite.
   Worth upstreaming a proper fix (e.g. resolve the CLI binary, or ship a
   `TestMain`/argv interception like `GO_WANT_HELPER_PROCESS`).

2. **`batch size resolution` warning is cosmetic.** Every prepare logs
   `WARN batch size resolution failed ... could not fetch <blob> from
   promisor remote` — expected by design (`GIT_NO_LAZY_FETCH=1` on
   `cat-file --batch-check`; sizes resolve on demand via getattr). Not an
   error, but alarming in logs; git-edge could note it in docs.

## git-edge-side gaps found

**None.** Every operation the suite exercises — v2 advertisement,
`blob:none` clone/fetch, promisor backfill by OID, receive-pack,
ref update under an artifact-fs-managed gitdir, symlink/exec/gitlink
tree entries — was served correctly. The 30 s fetch timeouts observed
during the first run were caused by the fork bomb saturating the stack,
not by git-edge (warm fetches measure ~30 ms once the suite runs clean).

## Caveats / merge-risk notes

- Tested against `wrangler dev` (miniflare DO + local R2), not deployed
  Workers — cold-start DO latency will make hydration slower in
  production, but the protocol path is identical.
- Two `--rm` containers from the fork-bomb runs are still listed by
  `docker ps` and cannot be killed (`cannot kill container ... did not
  receive an exit event`) — stuck FUSE tasks. They're idle (~0% CPU);
  reclaim them with a Docker Desktop restart when convenient.
- `docker exec` cannot enter a container with live FUSE mounts
  (`setns` fails) — debug via the daemon's stderr log + wrangler log
  instead.
- The e2e suite's local-default path (`createLocalTestRepo`) was not run
  here; we exercised only the `AFS_E2E_REPO` remote path — which is the
  one that matters for git-edge.

## Recommendation: document `git-edge mount`

Yes — publish it. The recipe that works today:

```bash
# per-repo read token
TOKEN=$(curl -s -X POST "$GE/o/r/_admin/tokens" \
  -u test:$GE_WRITE_TOKEN -d '{"name":"afs","level":"read"}' | jq -r .token)

# ambient creds (required for --async; also keeps remote.origin.url clean)
git config --global credential.helper store
echo "https://gitedge.example.com" > ~/.git-credentials   # host: user=test password=$TOKEN

artifact-fs add-repo --name r \
  --remote "https://gitedge.example.com/o/r" --ref refs/heads/main \
  --mount-root /tmp/mnt
artifact-fs daemon --root /tmp/mnt
```

Linux needs `fuse3`; macOS needs macFUSE; containers need
`--device /dev/fuse --cap-add SYS_ADMIN`. Docs should mention: inline
`user:token@` in the URL works for one-shot `add-repo` but is rejected
for `--async`; prefer a credential helper. Hydration per blob is one
`fetch` round-trip — great for agent/CI short-lived checkouts, chatty
for `git status` on large repos (artifact-fs's own fsmonitor hook
mitigates this once their e2e bug is fixed upstream).

## Reproduce

```bash
# host
cd server && wrangler dev --port 8795            # git-edge
git clone https://github.com/cloudflare/artifact-fs /tmp/artifact-fs
# patch: AFS_FSMONITOR_EXE override in internal/gitstore/gitstore.go

# container
docker build -f Dockerfile -t afs-e2e /tmp/artifact-fs   # golang:1.26 + fuse3 + git
docker run --rm --device /dev/fuse --cap-add SYS_ADMIN \
  --security-opt apparmor:unconfined \
  -e GE_TOKEN=<per-repo-token> -e AFS_FSMONITOR_EXE=/usr/local/bin/artifact-fs \
  -v /tmp/artifact-fs:/src afs-e2e bash -c '
    git config --global credential.helper store
    echo "http://test:$GE_TOKEN@host.docker.internal:8795" > ~/.git-credentials
    cd /src && AFS_RUN_E2E_TESTS=1 \
      AFS_E2E_REPO=http://host.docker.internal:8795/afs/test \
      go test -v -run "TestE2E|TestFUSEMountSmoke" -count=1 -timeout 30m .'
```
