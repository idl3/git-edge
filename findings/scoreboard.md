# Scoreboard

Scores are 1 to 5 from the reviewer. Feasibility: runs on GA Cloudflare primitives today. Reliability: no data loss or split-brain under crashes and concurrent writers. Correctness: achieves the stated goal, not a lookalike.

| Verdict | Count |
|---|---|
| lands | 1 |
| lands with caveats | 33 |
| risky | 21 |
| does not land | 1 |

| # | Tier | Idea | Verdict | Feas | Rel | Corr | Effort | Proof | Review |
|---|---|---|---|---|---|---|---|---|---|
| 53 | foundation | The /info/refs?service= entrypoint and pkt-line codec | lands | 5 | 5 | 4 | days | [proof](../proofs/info-refs-endpoint.md) | [review](../reviews/info-refs-endpoint.md) |
| 1 | foundation | One Durable Object per repo as the ref authority | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/repo-do-ref-authority.md) | [review](../reviews/repo-do-ref-authority.md) |
| 2 | foundation | Refs in DO SQLite, objects in R2 | lands with caveats | 4 | 2 | 3 | weeks | [proof](../proofs/refs-sqlite-objects-r2.md) | [review](../reviews/refs-sqlite-objects-r2.md) |
| 4 | foundation | Packfile parsing in a Worker with a streaming inflater | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/streaming-pack-parser.md) | [review](../reviews/streaming-pack-parser.md) |
| 5 | foundation | Content-addressed R2 keys | lands with caveats | 4 | 4 | 4 | days | [proof](../proofs/content-addressed-r2-keys.md) | [review](../reviews/content-addressed-r2-keys.md) |
| 6 | foundation | Two-phase push | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/two-phase-push.md) | [review](../reviews/two-phase-push.md) |
| 7 | foundation | Precomputed pack slices for clone | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/precomputed-clone-pack.md) | [review](../reviews/precomputed-clone-pack.md) |
| 8 | foundation | Delta bases pinned per repo | lands with caveats | 4 | 4 | 3 | weeks | [proof](../proofs/pinned-delta-bases.md) | [review](../reviews/pinned-delta-bases.md) |
| 11 | foundation | Git LFS natively via presigned R2 URLs | lands with caveats | 4 | 3 | 4 | days | [proof](../proofs/native-lfs.md) | [review](../reviews/native-lfs.md) |
| 12 | foundation | Bundle-URI support | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/bundle-uri.md) | [review](../reviews/bundle-uri.md) |
| 13 | edge | Refs replicated to every region via KV and DO location hints | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/replicated-refs-edge.md) | [review](../reviews/replicated-refs-edge.md) |
| 14 | edge | Push-triggered CI as a DO alarm chain | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/alarm-chain-ci.md) | [review](../reviews/alarm-chain-ci.md) |
| 15 | edge | Live fetch over hibernating WebSockets | lands with caveats | 4 | 3 | 2 | weeks | [proof](../proofs/live-fetch-websocket.md) | [review](../reviews/live-fetch-websocket.md) |
| 19 | edge | Signed refs by default with append-only DO reflog | lands with caveats | 4 | 2 | 3 | weeks | [proof](../proofs/signed-reflog.md) | [review](../reviews/signed-reflog.md) |
| 20 | edge | Time-travel refs | lands with caveats | 4 | 4 | 4 | days | [proof](../proofs/time-travel-refs.md) | [review](../reviews/time-travel-refs.md) |
| 21 | edge | Snapshots via R2 object versioning of ref state | lands with caveats | 4 | 2 | 3 | days | [proof](../proofs/r2-versioned-snapshots.md) | [review](../reviews/r2-versioned-snapshots.md) |
| 26 | edge | Search index built on push (D1 FTS / Vectorize) | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/search-index-on-push.md) | [review](../reviews/search-index-on-push.md) |
| 28 | edge | Rate-limited, token-scoped remote URLs | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/scoped-token-remotes.md) | [review](../reviews/scoped-token-remotes.md) |
| 29 | edge | Push from a sibling workspace DO over RPC, no HTTP | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/tui-rpc-push.md) | [review](../reviews/tui-rpc-push.md) |
| 30 | wild | Ephemeral repos with a self-destruct alarm | lands with caveats | 4 | 2 | 3 | days | [proof](../proofs/ephemeral-repos.md) | [review](../reviews/ephemeral-repos.md) |
| 32 | wild | Agent-native protocol v2 commands (search, explain-diff, suggest-merge) | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/agent-native-commands.md) | [review](../reviews/agent-native-commands.md) |
| 34 | wild | Commit-as-event-stream | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/commit-event-stream.md) | [review](../reviews/commit-event-stream.md) |
| 38 | wild | Deduplication across all repos in one content-addressed bucket | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/global-dedup.md) | [review](../reviews/global-dedup.md) |
| 39 | wild | Merkle inclusion proofs on fetch | lands with caveats | 4 | 5 | 3 | weeks | [proof](../proofs/merkle-proofs.md) | [review](../reviews/merkle-proofs.md) |
| 41 | wild | Branch previews deployed as Workers on push | lands with caveats | 3 | 3 | 3 | weeks | [proof](../proofs/branch-preview-workers.md) | [review](../reviews/branch-preview-workers.md) |
| 43 | wild | Bisect on the server with parallel test Workers | lands with caveats | 4 | 2 | 3 | weeks | [proof](../proofs/server-side-bisect.md) | [review](../reviews/server-side-bisect.md) |
| 44 | wild | Ref leases | lands with caveats | 5 | 4 | 4 | days | [proof](../proofs/ref-leases.md) | [review](../reviews/ref-leases.md) |
| 45 | wild | Commit graph in Vectorize for semantic git log | lands with caveats | 4 | 3 | 2 | weeks | [proof](../proofs/vectorized-commit-graph.md) | [review](../reviews/vectorized-commit-graph.md) |
| 46 | wild | Time-boxed history with cold-storage checkpoints | lands with caveats | 3 | 3 | 3 | weeks | [proof](../proofs/time-boxed-history.md) | [review](../reviews/time-boxed-history.md) |
| 47 | wild | Pull-request review data as git objects under refs/reviews | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/reviews-as-refs.md) | [review](../reviews/reviews-as-refs.md) |
| 49 | wild | GitHub-compatible webhook payloads | lands with caveats | 4 | 3 | 3 | days | [proof](../proofs/github-webhook-compat.md) | [review](../reviews/github-webhook-compat.md) |
| 50 | wild | Storage tiering by heat | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/storage-tiering.md) | [review](../reviews/storage-tiering.md) |
| 54 | foundation | Auth and multi-tenancy: owner/repo routing to DO ids | lands with caveats | 5 | 4 | 4 | days | [proof](../proofs/auth-and-multitenancy.md) | [review](../reviews/auth-and-multitenancy.md) |
| 56 | edge | Want/have negotiation with a commit-graph in SQLite | lands with caveats | 4 | 3 | 3 | weeks | [proof](../proofs/want-have-negotiation.md) | [review](../reviews/want-have-negotiation.md) |
| 3 | foundation | Speak git protocol v2 only, translate v0 at the edge | risky | 4 | 3 | 2 | weeks | [proof](../proofs/protocol-v2-only.md) | [review](../reviews/protocol-v2-only.md) |
| 9 | foundation | Tiny in-DO object cache with alarm-driven eviction | risky | 4 | 4 | 2 | days | [proof](../proofs/in-do-object-cache.md) | [review](../reviews/in-do-object-cache.md) |
| 10 | foundation | Shallow and partial clone as first-class filters | risky | 3 | 4 | 3 | weeks | [proof](../proofs/partial-clone-filters.md) | [review](../reviews/partial-clone-filters.md) |
| 16 | edge | Branch-level Durable Objects for monorepos | risky | 4 | 3 | 2 | weeks | [proof](../proofs/branch-level-dos.md) | [review](../reviews/branch-level-dos.md) |
| 17 | edge | Server-side three-way merge in the Worker | risky | 4 | 3 | 2 | weeks | [proof](../proofs/server-side-merge.md) | [review](../reviews/server-side-merge.md) |
| 18 | edge | Server-side rebase and squash as protocol v2 extensions | risky | 4 | 2 | 2 | weeks | [proof](../proofs/server-side-rebase.md) | [review](../reviews/server-side-rebase.md) |
| 22 | edge | Copy-on-write forks | risky | 4 | 2 | 3 | weeks | [proof](../proofs/cow-forks.md) | [review](../reviews/cow-forks.md) |
| 23 | edge | Pre/post-receive hooks as Workers via service bindings | risky | 4 | 2 | 3 | weeks | [proof](../proofs/hooks-as-workers.md) | [review](../reviews/hooks-as-workers.md) |
| 24 | edge | Per-blob presigned direct upload for giant pushes | risky | 4 | 2 | 3 | weeks | [proof](../proofs/presigned-direct-upload.md) | [review](../reviews/presigned-direct-upload.md) |
| 25 | edge | Wasm git core (gitoxide/libgit2) for delta resolution and merge | risky | 4 | 3 | 3 | weeks | [proof](../proofs/wasm-git-core.md) | [review](../reviews/wasm-git-core.md) |
| 27 | edge | Diff API served with R2 range reads | risky | 4 | 3 | 2 | weeks | [proof](../proofs/diff-api-range-reads.md) | [review](../reviews/diff-api-range-reads.md) |
| 33 | wild | Semantic diffs via tree-sitter in Wasm | risky | 3 | 3 | 2 | weeks | [proof](../proofs/semantic-diffs.md) | [review](../reviews/semantic-diffs.md) |
| 35 | wild | Speculative packs | risky | 3 | 4 | 2 | weeks | [proof](../proofs/speculative-packs.md) | [review](../reviews/speculative-packs.md) |
| 36 | wild | Federated remotes via DO-to-DO gossip | risky | 3 | 2 | 3 | weeks | [proof](../proofs/federated-gossip.md) | [review](../reviews/federated-gossip.md) |
| 37 | wild | Encrypted-at-rest with client-held keys | risky | 4 | 2 | 3 | weeks | [proof](../proofs/client-key-encryption.md) | [review](../reviews/client-key-encryption.md) |
| 40 | wild | Zero-clone execution: repo as a virtual filesystem inside an agent DO | risky | 4 | 2 | 3 | weeks | [proof](../proofs/zero-clone-vfs.md) | [review](../reviews/zero-clone-vfs.md) |
| 42 | wild | Git as a database driver (ActiveRecord / JS ORM adapter) | risky | 4 | 2 | 2 | weeks | [proof](../proofs/git-as-db-driver.md) | [review](../reviews/git-as-db-driver.md) |
| 48 | wild | Cross-repo atomic pushes | risky | 3 | 2 | 2 | weeks | [proof](../proofs/cross-repo-atomic-push.md) | [review](../reviews/cross-repo-atomic-push.md) |
| 51 | wild | Blame that knows which agent wrote each line | risky | 3 | 3 | 2 | weeks | [proof](../proofs/agent-blame.md) | [review](../reviews/agent-blame.md) |
| 52 | wild | Offline-first browser client with OPFS and the same Wasm core | risky | 3 | 3 | 2 | months | [proof](../proofs/offline-browser-client.md) | [review](../reviews/offline-browser-client.md) |
| 55 | edge | GC and repack as a DO alarm | risky | 3 | 2 | 2 | weeks | [proof](../proofs/gc-and-repack-alarm.md) | [review](../reviews/gc-and-repack-alarm.md) |
| 31 | wild | Multi-writer CRDT branches | does not land | 3 | 2 | 2 | weeks | [proof](../proofs/crdt-branches.md) | [review](../reviews/crdt-branches.md) |

## Reviewer summaries

### #1 One Durable Object per repo as the ref authority

The core claim holds: one DO per repo with a synchronous SQLite compare-and-swap reproduces exactly what receive-pack does (it passes the advertised old oid to ref_transaction_update), on GA primitives, with no split-brain under concurrent pushes. The proof code as written would likely reject branch creates/deletes (rowsWritten counts index writes) and can orphan a committed ref after a Worker crash because the CAS commits before objects are promoted out of pending/, so it needs those two fixes plus a chunked-body multipart path before it is a trustworthy foundation.

### #2 Refs in DO SQLite, objects in R2

The refs-in-SQLite / objects-in-R2 split is sound and built entirely on GA primitives; single-DO CAS makes concurrent pushes behave exactly as git expects and immutable sha keys make retries safe. The sketch as written would fail the first git clone (query string dropped, no HEAD, thin packs unresolved) and has two real object-deletion paths (shared prefix, sweeper racing live pushes), but each is a bounded fix rather than a redesign.

### #3 Speak git protocol v2 only, translate v0 at the edge

The design is the right one for Workers and every primitive it uses is GA, but the proof as written cannot complete a single git clone from a modern client (it sends an acknowledgments section after done) and its v0 shim only handles clones, so the 'thin shim, one code path' claim is unproven. The fixes are wire-format edits rather than architectural changes; expect days for v2 clone/fetch interop and weeks for a correct v0 multi_ack shim plus shallow/filter support.

### #4 Packfile parsing in a Worker with a streaming inflater

The mechanism is sound and every primitive it uses is GA on Cloudflare today; the proof is right that DecompressionStream cannot do per-object inflate and correctly substitutes node:zlib with consumed-byte reporting (workerd support unverified but with an easy fallback). As written it breaks on delete-only pushes, OOMs on multi-MB blobs via an array spread in sha1(), and flips refs without verifying tips exist; each fix is under a day, and a working receive-pack is roughly two weeks away.

### #5 Content-addressed R2 keys

The idea maps git's own content addressing onto R2 keys using only GA primitives (R2 put with sha1 integrity, head, DO SQLite) and is genuinely idempotent under mid-push crashes and concurrent pushes, with refs never moving on any failure path. Its weaknesses are integration contracts rather than design flaws: a GC grace window to avoid a HEAD-skip/delete race, reconciling the pending-prefix in two-phase-push, a ~5,000-object per-request ceiling from subrequest limits, and a ~50MB single-object memory ceiling.

### #6 Two-phase push

The design (ref CAS inside a single-threaded DO, content-addressed idempotent R2 writes, manifest-driven sweep) is the right shape and uses only GA primitives, but the proof as written loses data under a janitor-vs-new-push race and reports ok for refs it never moved. Both are small fixes; the remaining gaps (sideband, gzip request bodies, pack boundary detection) sit in sibling ideas but must be closed before any stock git client can push.

### #7 Precomputed pack slices for clone

The design is the right shape for Workers (DO decides, R2 range-read streams through a sideband reframer, v2 fetch with done and no haves legitimately skips acks), and every primitive it needs is GA. As written the proof code cannot complete one clone (pkt-line length bug, wrong blobless trailer hash) and the pack builder, where the real feasibility risk lives, is deferred to another idea; fix those and it lands with lifecycle and cost caveats.

### #8 Delta bases pinned per repo

The pack-format reasoning (REF_DELTA reuse, mixed-source entries, streaming SHA-1 trailer) is correct and every primitive is GA, and because pins/deltas are a pure cache over R2 no crash or concurrent-writer path loses data or splits refs. Two concrete defects remain: ctx.id.name is undefined inside the DO so the cold path is broken as written, and the hand-waved clientHas check emits thin deltas that stock git rejects for shallow and partial-clone clients.

### #9 Tiny in-DO object cache with alarm-driven eviction

The cache skeleton, alarm-driven eviction, and durability story are sound (immutable content-addressed rows, refs untouched, no data-loss path under crash or concurrent misses), and every primitive is GA. But the miss path assumes an R2 body encoding (zlib + numeric kind + size metadata) that no sibling proof writes, so the spliced pack entries are unreadable by git, and the unbounded memory tier can OOM the DO; both fixes are small but absent.

### #10 Shallow and partial clone as first-class filters

The SQLite-planner / R2-streamer split is the right architecture and the v2 section layout, side-band framing, PACK entry headers, and filter semantics are essentially correct, but the proof cannot survive a real `git checkout` after a blob:none clone (one batched request needing thousands of R2 GETs against a 1,000-subrequest cap) and cannot clone a repo with annotated tags. Read-only path so no split-brain or data-loss risk; the missing fan-out is the actual engineering and is not shown.

### #11 Git LFS natively via presigned R2 URLs

A genuine LFS batch API with basic transfer, presigned R2 PUT/GET, a signed sha256 checksum header, and a verify hook, built entirely on GA R2/DO primitives with payloads never touching the Worker. Three small but real defects (verify must upsert, oid must be validated before key derivation, edge router unshown) and an alarm-starvation bug stand between the proof and a stock git-lfs client pushing and pulling within days.

### #12 Bundle-URI support

The wire format and Cloudflare primitives are correct and GA, and the design is a thin layer over the precomputed clone pack, so it lands for clients that opt in. Two real bugs must be fixed first: the header must be written from the pack's snapshotted tips (not live refs) or the bundle is dead on arrival after any intervening push, and pruned bundles must actually be deleted from R2.

### #13 Refs replicated to every region via KV and DO location hints

The single-writer DO plus versioned KV snapshot rendered as ls-refs pkt-lines is sound and uses only GA primitives, but the proof code has an indefinite-staleness publish bug, a broken CAS rejection path, and omits HEAD/symref so clone cannot check out. Fixing those is days of work; what remains is inherently an eventually-consistent edge cache (not a replica) that still sends info/refs to the DO unless served statically.

### #14 Push-triggered CI as a DO alarm chain

The alarm-chain mechanics (running row + watchdog lease + bounded attempts, verdict published as a git blob under refs/ci/<sha> readable via ls-refs) are correct and buildable on GA DO/R2 primitives, and a real git client would interoperate. The central "CI can never be lost" claim is false as written because the enqueue is not atomic with the ref flip across two DOs, and finish() can strand a run; both are fixable with an outbox row and reordering.

### #15 Live fetch over hibernating WebSockets

All primitives are GA and the notification-only variant (nudge, then plain HTTP `git fetch`) is sound: refs never split-brain because the single DO does a synchronous SQL CAS. The in-socket packfile path as written does not interoperate with git's fetch-pack (extra acknowledgments section on a `done` request), loses ref moves across reconnects/evictions, and overflows the 2 KiB attachment cap, so it needs those three fixes plus flow control before it is more than a demo.

### #16 Branch-level Durable Objects for monorepos

The one-DO-per-namespace design with per-shard CAS and shared R2 objects is buildable on GA primitives and genuinely isolates readers of a quiet branch from a busy one when the client sends a fully-qualified ref-prefix. As written it breaks ordinary clone and fetch (default prefixes route to a nonexistent shard), drops HEAD/symrefs, has a crash window that silently hides new namespaces, and keeps a root-DO write on every push, so it needs a routing redo before any real-git test.

### #17 Server-side three-way merge in the Worker

The push-option wire handling and tree-level three-way merge are sound and use only GA Workers/DO/R2 primitives, but the proof code cannot land a single non-fast-forward merge against its own dependencies because the merge objects are never registered with the DO before commit() and are invisible to the janitor. Fixable with an object-registration step and sideband-wrapped reporting, roughly weeks of work on top of the sibling push pipeline.

### #18 Server-side rebase and squash as protocol v2 extensions

The protocol claim holds: git >= 2.18 accepts a bare 'version 2' advertisement, ignores the unknown rebase=squash/rebase-status capability lines, and the CAS-guarded ref flip in the single repo DO prevents split-brain under concurrent rebases and pushes. The rebase engine itself is a sketch: the squash path crashes, rebase-status is unimplemented, the alarm job queue can starve or strand jobs, and without diff3 it is a tree-level cherry-pick loop rather than a rebase, so weeks of work separate it from a version that rebases a branch with both-side file edits.

### #19 Signed refs by default with append-only DO reflog

The server-signed, hash-chained reflog written in the same SQLite transaction as the ref CAS and periodically anchored to R2 is sound and built entirely on GA primitives (DO SQLite, alarms, WebCrypto Ed25519, R2). But the proof code races concurrent pushes across non-storage awaits (breaking the CAS it depends on and forking the chain), and its description of the `git push --signed` wire format would reject real signed pushes; both are days-scale fixes, with full push-cert verification adding 1-2 weeks.

### #20 Time-travel refs

The mechanism is sound: the reflog row commits in the same DO SQLite transactionSync as the ref CAS, `refs/at/<epoch>/<name>` is a valid refname, and protocol v2 lets the server advertise a synthetic ref in ls-refs and then serve an ordinary `want <sha>`, all on GA primitives with no data-loss or split-brain path. It is not shippable as written because `want-ref` (ref-in-want) is unhandled, the single DO alarm collides with the GC alarm, and there are pkt-line byte-length and same-second duplicate-name bugs, plus an expiry/GC dangling-advertisement window; all are days-scale fixes.

### #21 Snapshots via R2 object versioning of ref state

All primitives are GA and the commit-then-snapshot ordering respects the receive-pack ack contract, but the proof misreads DO input gates: concurrent pushes can leave LATEST pointing at a stale snapshot after an acknowledged push, and keying the R2 prefix on ctx.id means the backup is unreachable after a namespace recreate. Fixes are small (mutex or max-key restore, key on ctx.id.name, include HEAD), so it lands with caveats in days.

### #22 Copy-on-write forks

The wire mechanism is sound: an index-only fork that advertises the parent's tips gets real push dedup from git itself, resolves thin-pack REF_DELTA bases across prefixes correctly, and assembles packs indistinguishable from a single-prefix repo, with clean crash recovery via idempotent paged inserts. But as specified it loses data on a schedule because the GC it depends on deletes the loose keys the fork points to after repack, the layer key is a mutable repo name, and pins cover only page-0 tips, so it lands only after gc-and-repack-alarm gains pin/pack-offset support and layers become immutable DO ids.

### #23 Pre/post-receive hooks as Workers via service bindings

The skeleton is right (pre-receive outside the transaction, CAS re-check inside, post-receive outbox committed atomically with the ref flip, all on GA DO/alarm/service-binding primitives), and git push interop via ng-lines is sound in principle. As written it clobbers the two-phase-push janitor alarm on a shared DO, can corrupt the push report with an ordinary multi-line hook message, and leaves the pending-object fetch that real hooks need undesigned, so it is risky until those three are fixed.

### #24 Per-blob presigned direct upload for giant pushes

All primitives are GA and using a blob-filtered pack as the manifest is the right design, but the janitor's R2 delete combined with open DO input gates during head() produces a committed ref with a missing blob in two ordinary interleavings, so the proof's central reliability claim is wrong as written. The fixes are small; the real cost is a bespoke remote helper that drives receive-pack directly, which is weeks of work and means stock git never benefits.

### #25 Wasm git core (gitoxide/libgit2) for delta resolution and merge

The host-does-I/O, pure-bytes Wasm boundary on a gitoxide subset is sound and every Cloudflare primitive used is GA, with no data-loss or split-brain path since refs stay in the DO and R2 keys are content-addressed. As written, though, the ofs-delta chain resolution contradicts its own parser dependency, Wasm memory leaks per isolate, and two infinite-alarm stall paths plus a janitor alarm collision mean it needs three concrete fixes before it lands.

### #26 Search index built on push (D1 FTS / Vectorize)

All primitives are GA and the durable job-row + alarm + tree-diff + FTS5 upsert loop is sound, with zero impact on the git wire protocol since indexing runs after report-status is flushed. As written it livelocks on large jobs, conflates branches, and stalls on poison jobs, so it is a last-pushed-branch grep until four small fixes land, and it silently depends on the object store keeping every object loose in R2.

### #27 Diff API served with R2 range reads

The one-range-read-per-pack-entry plus delta-opcode decoding mechanism is sound and every primitive is GA, but the sibling foundation stores resolved loose objects and non-delta packs, so the fast path never fires without a delta-producing repack that nobody has built. Even with it, the zero-read answer is a compression edit script rather than a git diff, and any unified diff still reconstructs the base, leaving a weaker lookalike of the stated goal.

### #28 Rate-limited, token-scoped remote URLs

The HMAC-in-path, parse-commands-before-PACK, single-DO-counter design is sound and uses only GA primitives, and the rate limit is genuinely exact rather than eventually consistent. As written it breaks on two real wire cases (shallow pkt-lines from depth-1 CI checkouts, and a locked-reader bug in the pack handoff) and burns budget on crashes, but all fixes are hours of work on top of the sibling proofs, not a redesign.

### #29 Push from a sibling workspace DO over RPC, no HTTP

The core mechanism (typed DO RPC replacing git-receive-pack, loose objects written straight to a shared R2 bucket, single-threaded ref CAS plus reflog in repo-DO SQLite with output gating) is sound on GA Cloudflare primitives and survives crash and concurrent-writer scenarios without data loss or split-brain. As written, the proof produces fsck-invalid trees for any nested workspace and ignores the 1000-subrequest cap, so it is a snapshot-commit builder that needs roughly two weeks of tree/chunking/GC-grace work before it lands.

### #30 Ephemeral repos with a self-destruct alarm

The alarm-chained, cursor-resumable R2 wipe under an epoch-scoped prefix is the right design and uses only GA Cloudflare primitives, with correct ref-CAS/liveness atomicity and clean v0 smart-HTTP interop. As pasted, though, the code would 500 after expiry (deleteAll drops the schema), mis-prefix objects (ctx.id.name is undefined in-DO), and leak R2 storage when a repo is re-created mid-deletion; each is a small fix but they must land before the mechanism works.

### #31 Multi-writer CRDT branches

All primitives are GA, but the proof's git-client story is contradicted at the wire: git rejects divergent pushes client-side, and the --force path uses the advertised tip as the diff base, actively reverting other writers' work. The DO merge loop also loses updates under concurrent materialization and post-compaction replay; an agent-POST-only variant with a per-branch mutex, lamport comparison, and base = pushed commit's parent is buildable in weeks, but it is a serialized LWW merge service rather than the multi-writer CRDT claimed.

### #32 Agent-native protocol v2 commands (search, explain-diff, suggest-merge)

The v2 extension is protocol-correct: real git ignores the extra capability lines, the request/response pkt-line framing mirrors fetch, and no ref is ever written so there is no split-brain path. As written, though, it is a weaker lookalike: the merge suggestion cannot actually be fetched (wrong R2 prefix, no any-sha1-in-want), merge-base is a stub, the search index drifts on force-push, and AI context limits are unhandled; each fix is days, none needs a new primitive.

### #33 Semantic diffs via tree-sitter in Wasm

All Cloudflare primitives used (DO SQLite, CompiledWasm import, R2 get, DecompressionStream, cpu_ms limits, alarms) are GA and the cache is crash- and race-safe because it is purely content-addressed, but the Wasm build recipe is unverified and the object-read path only works if blobs are stored as loose zlib objects rather than pack entries. The diff algorithm as written is a lookalike of the stated goal: rename detection can never fire, out-of-definition changes vanish, and container nodes double-report, so it needs a rewrite of defs/matching before it delivers function-level diffs.

### #34 Commit-as-event-stream

The transactional-outbox-in-DO plus alarm plus Queues design is the right serverless shape and uses only GA Cloudflare primitives, with sound ref CAS under concurrent pushes. As written it double-reports commits on multi-ref pushes, returns report-status in a form that breaks side-band clients, only recognizes undeltified commits, and leans on write coalescing for alarm durability; a few days of fixes on top of a working pack parser make it real.

### #35 Speculative packs

The core insight is sound: protocol-v2 `ready` makes the fetch response a fixed wrapper around a deterministic (have,want) pack, so pre-building it in an alarm is legitimate and its failure mode is a plain cache miss with no data-loss or ref split-brain path. As written it does not interoperate with any real git client (sideband `0004` bug), is keyed on the wrong thing (`wants[0]`), and its per-object R2 prewarm hits the 1000-subrequest cap; fix those three and it lands as a modest win for single-branch, non-shallow trackers only.

### #36 Federated remotes via DO-to-DO gossip

The atomic outbox + at-least-once alarm + (origin, seq) dedupe mechanics are sound and buildable on GA Durable Object, R2 and Workers primitives, but the proof code is runtime-fatal as written (ctx.id.name is undefined inside a DO), has an out-of-order ref-regression race, and its anti-entropy does not cover transitive peers. What it delivers is a hub-less mesh of remote-tracking refs that a plain git clone never sees, which is a weaker lookalike of "a mesh of mirrors".

### #37 Encrypted-at-rest with client-held keys

The DO-holds-hashes / R2-holds-ciphertext mechanics are buildable today on GA primitives and refs cannot split-brain thanks to CAS inside transactionSync, but as written the system is unauthenticated, lets anyone overwrite referenced ciphertexts, and has a janitor sketch that either leaks or deletes live data. It meets the literal goal only by abandoning the git wire protocol entirely, and the remote helper that would make a real git client usable is not written.

### #38 Deduplication across all repos in one content-addressed bucket

The dedup layer itself is sound and built entirely on GA primitives (DO SQLite, R2 head/put/get, CompressionStream deflate, subtle SHA-1); in append-only mode it has no data-loss or split-brain path under crashes or concurrent same-SHA pushes because put precedes the index row and identical bytes under one key are idempotent. It falls short on the wire (the shown fetch response lacks pkt-line/sideband framing), has no safe reclamation story, leaks existence via timing, and does not demonstrate that dropping deltas for cross-repo dedup actually saves space.

### #39 Merkle inclusion proofs on fetch

The mechanism is cryptographically sound and operationally safe: it is a read-only walk over immutable content-addressed R2 objects that yields a real Merkle inclusion proof rooted at a client-trusted commit, with no data-loss or split-brain path. But the shown code cannot ship as written (single pkt-line per object breaks framing on every object over 64 KB, whole objects are buffered in a 128 MB isolate, packs are deferred), real git clients simply ignore the command rather than use it, and the assurance delivered is a convenience RPC over what git's content addressing already provides rather than a new trust boundary.

### #40 Zero-clone execution: repo as a virtual filesystem inside an agent DO

The lazy content-addressed VFS on DO SQLite + R2 uses only GA primitives and the loose-object parsing is correct, but the proof only works on a repo that has never received a packed push and has a GC race that can publish a commit referencing a deleted blob plus a lost-write interleaving in commit(). It delivers clone-free read/edit, not execution, and needs a pack/delta reader, a correct commit() (tree sort order, headers, modes), a GC lease, and rebase-on-CAS-fail before it lands.

### #41 Branch previews deployed as Workers on push

The alarm-as-post-receive-hook plus tree-to-multipart WfP upload is correct and buildable on GA Cloudflare APIs today, and git 2.4x push interop is unaffected because everything happens after report-status. It only works for small pre-built module trees, and the proof code carries two real bugs (.one() on empty table, non-atomic ref/deploy INSERT), a name-collision design hole, an unbudgeted R2 subrequest cap, and a TLS gap on two-level wildcard hostnames.

### #42 Git as a database driver (ActiveRecord / JS ORM adapter)

All primitives are GA and the loose-object bytes are nearly git-correct, but the proof's core claim that the DO serializes saves without locks is false under input-gate semantics: the ref read and write straddle R2 awaits, so concurrent saves race, the optimistic lock is a no-op, and a mid-migration crash bakes a half-rewritten table into main. With a synchronous CAS, persisted migration state, git-order tree sorting, and mirror refresh on push it lands as a git-backed document store in weeks; as written it is risky.

### #43 Bisect on the server with parallel test Workers

Every primitive (DO SQLite, alarms, service bindings, WfP, R2 get, DecompressionStream deflate) is GA and the SQLite state machine is crash-safe, but the proof code has a session-starvation bug, an unbounded retry loop, and no real tenant isolation for object reads. No git wire protocol is touched so stock clients are unaffected; the result is a first-parent-only, k<=6 parallel bisect that is a modest improvement over local `git bisect run` rather than the claimed leap.

### #44 Ref leases

Enforcing leases inside the DO's single synchronous transactionSync that already does the ref CAS is the correct design: all primitives are GA, crashes cannot split refs from leases, and concurrent pushers serialize cleanly with lazy expiry making the alarm mere garbage collection. Remaining gaps are integration details (push-options advertisement, alarm multiplexing, pending/ orphan cleanup, principal-level holder identity) rather than design flaws, so it lands with caveats in a few days on top of two-phase-push.

### #45 Commit graph in Vectorize for semantic git log

All primitives are GA and the off-request-path alarm design never endangers refs or objects, but the proof has a fatal id-length bug and a queue-wedging error path, and it honestly delivers a semantic commit-search endpoint over messages plus path names rather than the promised `git log --semantic` with diffs. Fix the two blockers, add reachability filtering and deletion, and shard the index, and the re-scoped feature lands in weeks on top of the foundation ideas.

### #46 Time-boxed history with cold-storage checkpoints

A server-imposed shallow horizon over a precomputed R2 clone pack is a real, in-spec mechanism that makes fresh clones one R2 stream, but it is a lookalike of the stated squash and the proof as written breaks every clone after a push and every push from a shallow clone. Both fixes are localized (gate the hot path on tip equality; parse shallow lines in receive-pack), so it lands with caveats in a few weeks on top of the pack-builder ideas.

### #47 Pull-request review data as git objects under refs/reviews

The core idea is sound and needs only GA primitives: review events as fast-forward commits under refs/reviews/* are fetchable, mirrorable, and never checked out by stock git. The proof code as written would not yield a fetchable ref inside the sibling stack (R2 byte format/key layout and commit-graph tables disagree with its dependencies), and it lacks idempotency, CAS retry, and a push-safe seq derivation; fixing those is days, a usable feature is weeks.

### #48 Cross-repo atomic pushes

The coordinator is a sound textbook 2PC on GA Durable Object primitives (durable decide-guard, participants never decide alone, idempotent commit), but the participant's prepare() has a real lost-update race across an R2 await, the recovery path is broken by ctx.id.name being undefined, and the single-alarm slot collides with GC. It delivers all-or-nothing (not isolated) cross-repo ref updates for a custom multipart client only; no real git client can reach it, so it is a weaker lookalike of the stated goal and needs weeks of work on top of the pack parser and single-repo push foundations.

### #49 GitHub-compatible webhook payloads

Outbox-row-in-same-SQLite-transaction plus alarm drain is the right pattern and uses only GA Cloudflare primitives, and it never touches the git wire protocol, so git clients are unaffected. It produces a GitHub-shaped push event most verifiers accept rather than the claimed byte-identical payload, and the drain needs four small fixes (setAlarm inside the transaction, tolerate deleted hooks, ORDER BY, dual sha1/sha256 signatures) before it is dependable.

### #50 Storage tiering by heat

The R2-canonical / DO-as-cache design is sound and cannot lose repo data or split-brain refs, and every primitive it uses is GA today. But the proof code has a sweep that permanently stalls after the first crashed re-class, a re-class path that cannot run inside an alarm at pack sizes, and it delivers "large cold packs to IA plus a small-object DO cache" rather than per-object heat tiering.

### #51 Blame that knows which agent wrote each line

The trailer-plus-attestation half is git-native, cheap, and interoperates with real git (unknown v2 capability is ignored; trailers are standard). The blame half as written crashes on cold calls, is memory-unbounded, can permanently cache wrong attributions, and misattributes exactly the merge-commit workflow agents most commonly use, so it needs a rewrite of blame() before it delivers the stated goal.

### #52 Offline-first browser client with OPFS and the same Wasm core

All Cloudflare and browser primitives are GA and the server side is unchanged, so C git interop holds by construction and ref CAS on both hosts avoids split-brain; but the proof code as written cannot detect a successful push, cannot discover refs to fetch, ignores the DO subrequest cap, and writes OPFS objects non-atomically. It is an offline commit-and-sync transport, not the offline-first client claimed, and reaching that goal is gated on a complete wasm-git-core (months).

### #53 The /info/refs?service= entrypoint and pkt-line codec

The handshake and pkt-line codec use only GA Workers/DO-SQLite/stream primitives on a read-only path with no data-loss or split-brain surface, and the v0 and v2 advertisement wire shapes match what git 2.4x's remote-curl and fetch-pack actually check, so a real client will proceed to the smart POST path. The remaining work is trimming over-advertised capabilities that downstream handlers must honor, adding error handling for eviction/disconnect, and the small version=1 fidelity fix.

### #54 Auth and multi-tenancy: owner/repo routing to DO ids

All primitives (Workers, crypto.subtle HMAC, DO SQLite, idFromName, jurisdiction, KV) are GA, the DO is the single ACL authority with idempotent writes, and the 401/403 choreography matches what git's remote-curl and http.c actually do. It achieves the stated goal but leaks private-repo existence to anonymous callers, names DOs differently from its sibling proofs, and misdescribes git's large-push auth behavior; fixing those is a few days of work.

### #55 GC and repack as a DO alarm

The design (index-only mark, resumable phased alarm, timestamp grace period, content-addressed sweep) is the right shape for serverless git GC and uses only GA primitives, but the code as written cannot run against the repos its dependencies build, has a real dangling-ref race in sweep, and has several ways to wedge GC permanently. Fixes are bounded and well-understood but none are done, and the produced pack is never wired to a consumer.

### #56 Want/have negotiation with a commit-graph in SQLite

The mechanism is sound and genuinely git's own generation-ordered two-colour walk plus a provably-superset introduced-object union, all on GA DO SQLite with zero R2 reads during negotiation. However the proof code as written does not interoperate with real git (sideband frame overflow) and its push side breaks on non-topological pack order, so treat it as a validated design rather than tested code; fixes are days, the tree-diff and scaling work around it are weeks.

