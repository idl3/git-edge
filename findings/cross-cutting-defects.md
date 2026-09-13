# Cross-cutting defects

The reviewers kept finding the same failure modes across unrelated ideas. These are platform truths about Workers and Durable Objects, not per-idea mistakes. Fix each once in the foundation and most of the per-idea caveats disappear.

## pkt-line and sideband framing details git actually checks

**Hit 37 of 56 proofs.**

Git checks exact bytes: pkt-line lengths include their own four hex digits, sideband frames must be at most 65,515 data bytes, report-status rides in band 1, and a v2 fetch with done omits the acknowledgments section.

**Fix once:** One shared pkt-line and sideband codec module, tested against a stock git client before any feature uses it.

Affected: [#1 repo-do-ref-authority](../reviews/repo-do-ref-authority.md), [#2 refs-sqlite-objects-r2](../reviews/refs-sqlite-objects-r2.md), [#3 protocol-v2-only](../reviews/protocol-v2-only.md), [#4 streaming-pack-parser](../reviews/streaming-pack-parser.md), [#6 two-phase-push](../reviews/two-phase-push.md), [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#8 pinned-delta-bases](../reviews/pinned-delta-bases.md), [#10 partial-clone-filters](../reviews/partial-clone-filters.md), [#13 replicated-refs-edge](../reviews/replicated-refs-edge.md), [#14 alarm-chain-ci](../reviews/alarm-chain-ci.md), [#15 live-fetch-websocket](../reviews/live-fetch-websocket.md), [#16 branch-level-dos](../reviews/branch-level-dos.md), [#17 server-side-merge](../reviews/server-side-merge.md), [#19 signed-reflog](../reviews/signed-reflog.md), [#20 time-travel-refs](../reviews/time-travel-refs.md), [#23 hooks-as-workers](../reviews/hooks-as-workers.md), [#25 wasm-git-core](../reviews/wasm-git-core.md), [#26 search-index-on-push](../reviews/search-index-on-push.md), [#28 scoped-token-remotes](../reviews/scoped-token-remotes.md), [#30 ephemeral-repos](../reviews/ephemeral-repos.md), [#31 crdt-branches](../reviews/crdt-branches.md), [#32 agent-native-commands](../reviews/agent-native-commands.md), [#34 commit-event-stream](../reviews/commit-event-stream.md), [#35 speculative-packs](../reviews/speculative-packs.md), [#36 federated-gossip](../reviews/federated-gossip.md), [#38 global-dedup](../reviews/global-dedup.md), [#39 merkle-proofs](../reviews/merkle-proofs.md), [#41 branch-preview-workers](../reviews/branch-preview-workers.md), [#44 ref-leases](../reviews/ref-leases.md), [#46 time-boxed-history](../reviews/time-boxed-history.md), [#48 cross-repo-atomic-push](../reviews/cross-repo-atomic-push.md), [#49 github-webhook-compat](../reviews/github-webhook-compat.md), [#50 storage-tiering](../reviews/storage-tiering.md), [#51 agent-blame](../reviews/agent-blame.md), [#52 offline-browser-client](../reviews/offline-browser-client.md), [#53 info-refs-endpoint](../reviews/info-refs-endpoint.md), [#56 want-have-negotiation](../reviews/want-have-negotiation.md)

## Janitor or GC sweeps race live pushes and delete referenced objects

**Hit 31 of 56 proofs.**

A janitor or repack computed an orphan set, awaited R2, and deleted an object that a concurrent push had just made reachable.

**Fix once:** Grace period by timestamp, refs_version check atomic with the delete, and never delete in the same alarm slice that computed the candidate set.

Affected: [#1 repo-do-ref-authority](../reviews/repo-do-ref-authority.md), [#2 refs-sqlite-objects-r2](../reviews/refs-sqlite-objects-r2.md), [#4 streaming-pack-parser](../reviews/streaming-pack-parser.md), [#5 content-addressed-r2-keys](../reviews/content-addressed-r2-keys.md), [#6 two-phase-push](../reviews/two-phase-push.md), [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#9 in-do-object-cache](../reviews/in-do-object-cache.md), [#10 partial-clone-filters](../reviews/partial-clone-filters.md), [#11 native-lfs](../reviews/native-lfs.md), [#12 bundle-uri](../reviews/bundle-uri.md), [#14 alarm-chain-ci](../reviews/alarm-chain-ci.md), [#17 server-side-merge](../reviews/server-side-merge.md), [#18 server-side-rebase](../reviews/server-side-rebase.md), [#20 time-travel-refs](../reviews/time-travel-refs.md), [#22 cow-forks](../reviews/cow-forks.md), [#23 hooks-as-workers](../reviews/hooks-as-workers.md), [#24 presigned-direct-upload](../reviews/presigned-direct-upload.md), [#25 wasm-git-core](../reviews/wasm-git-core.md), [#27 diff-api-range-reads](../reviews/diff-api-range-reads.md), [#29 tui-rpc-push](../reviews/tui-rpc-push.md), [#30 ephemeral-repos](../reviews/ephemeral-repos.md), [#32 agent-native-commands](../reviews/agent-native-commands.md), [#34 commit-event-stream](../reviews/commit-event-stream.md), [#37 client-key-encryption](../reviews/client-key-encryption.md), [#38 global-dedup](../reviews/global-dedup.md), [#40 zero-clone-vfs](../reviews/zero-clone-vfs.md), [#42 git-as-db-driver](../reviews/git-as-db-driver.md), [#44 ref-leases](../reviews/ref-leases.md), [#48 cross-repo-atomic-push](../reviews/cross-repo-atomic-push.md), [#50 storage-tiering](../reviews/storage-tiering.md), [#55 gc-and-repack-alarm](../reviews/gc-and-repack-alarm.md)

## Real pushes arrive as thin packs; every reader needs delta resolution

**Hit 27 of 56 proofs.**

Real pushes arrive as thin packs with ofs-delta and ref-delta entries. Many proofs assumed loose objects and had no delta resolution path.

**Fix once:** The streaming pack parser resolves deltas on ingest, and a pack index in SQLite lets every later reader find any object in a pack by range read.

Affected: [#2 refs-sqlite-objects-r2](../reviews/refs-sqlite-objects-r2.md), [#4 streaming-pack-parser](../reviews/streaming-pack-parser.md), [#5 content-addressed-r2-keys](../reviews/content-addressed-r2-keys.md), [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#8 pinned-delta-bases](../reviews/pinned-delta-bases.md), [#9 in-do-object-cache](../reviews/in-do-object-cache.md), [#22 cow-forks](../reviews/cow-forks.md), [#25 wasm-git-core](../reviews/wasm-git-core.md), [#26 search-index-on-push](../reviews/search-index-on-push.md), [#27 diff-api-range-reads](../reviews/diff-api-range-reads.md), [#28 scoped-token-remotes](../reviews/scoped-token-remotes.md), [#29 tui-rpc-push](../reviews/tui-rpc-push.md), [#33 semantic-diffs](../reviews/semantic-diffs.md), [#34 commit-event-stream](../reviews/commit-event-stream.md), [#35 speculative-packs](../reviews/speculative-packs.md), [#36 federated-gossip](../reviews/federated-gossip.md), [#37 client-key-encryption](../reviews/client-key-encryption.md), [#38 global-dedup](../reviews/global-dedup.md), [#39 merkle-proofs](../reviews/merkle-proofs.md), [#40 zero-clone-vfs](../reviews/zero-clone-vfs.md), [#41 branch-preview-workers](../reviews/branch-preview-workers.md), [#45 vectorized-commit-graph](../reviews/vectorized-commit-graph.md), [#47 reviews-as-refs](../reviews/reviews-as-refs.md), [#50 storage-tiering](../reviews/storage-tiering.md), [#52 offline-browser-client](../reviews/offline-browser-client.md), [#54 auth-and-multitenancy](../reviews/auth-and-multitenancy.md), [#55 gc-and-repack-alarm](../reviews/gc-and-repack-alarm.md)

## DO input gates open during R2 awaits, so check-then-act races

**Hit 15 of 56 proofs.**

Durable Objects are single-threaded, but the input gate only holds across storage awaits. Any await on R2 or fetch lets another request interleave, so a ref read before an R2 call and a ref write after it is a classic lost update.

**Fix once:** Do all R2 work first, then run the ref compare-and-swap inside one synchronous transactionSync with no await in it. Reserve blockConcurrencyWhile for GC-style sweeps.

Affected: [#2 refs-sqlite-objects-r2](../reviews/refs-sqlite-objects-r2.md), [#6 two-phase-push](../reviews/two-phase-push.md), [#11 native-lfs](../reviews/native-lfs.md), [#15 live-fetch-websocket](../reviews/live-fetch-websocket.md), [#18 server-side-rebase](../reviews/server-side-rebase.md), [#19 signed-reflog](../reviews/signed-reflog.md), [#21 r2-versioned-snapshots](../reviews/r2-versioned-snapshots.md), [#22 cow-forks](../reviews/cow-forks.md), [#24 presigned-direct-upload](../reviews/presigned-direct-upload.md), [#31 crdt-branches](../reviews/crdt-branches.md), [#35 speculative-packs](../reviews/speculative-packs.md), [#40 zero-clone-vfs](../reviews/zero-clone-vfs.md), [#42 git-as-db-driver](../reviews/git-as-db-driver.md), [#48 cross-repo-atomic-push](../reviews/cross-repo-atomic-push.md), [#55 gc-and-repack-alarm](../reviews/gc-and-repack-alarm.md)

## Proofs disagree on R2 key layout and object encoding

**Hit 15 of 56 proofs.**

The proofs disagreed on whether R2 holds zlib loose objects, raw content with metadata, or packs, and on key layout. Ideas that were individually fine could not read each other's bytes.

**Fix once:** Write one object storage spec: key layout, body encoding, metadata fields, and a pack index table shape. Every idea reads through the same object reader.

Affected: [#5 content-addressed-r2-keys](../reviews/content-addressed-r2-keys.md), [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#9 in-do-object-cache](../reviews/in-do-object-cache.md), [#10 partial-clone-filters](../reviews/partial-clone-filters.md), [#18 server-side-rebase](../reviews/server-side-rebase.md), [#21 r2-versioned-snapshots](../reviews/r2-versioned-snapshots.md), [#22 cow-forks](../reviews/cow-forks.md), [#26 search-index-on-push](../reviews/search-index-on-push.md), [#31 crdt-branches](../reviews/crdt-branches.md), [#33 semantic-diffs](../reviews/semantic-diffs.md), [#42 git-as-db-driver](../reviews/git-as-db-driver.md), [#43 server-side-bisect](../reviews/server-side-bisect.md), [#47 reviews-as-refs](../reviews/reviews-as-refs.md), [#54 auth-and-multitenancy](../reviews/auth-and-multitenancy.md), [#55 gc-and-repack-alarm](../reviews/gc-and-repack-alarm.md)

## A Durable Object has exactly one alarm slot; siblings clobber each other

**Hit 10 of 56 proofs.**

A Durable Object has exactly one alarm. The janitor, the repack, the CI chain, the KV publisher and the lease expirer all called setAlarm and silently cancelled each other.

**Fix once:** One alarm dispatcher per DO: a jobs table in SQLite with next_run_at, and a single alarm() that pops the earliest row and re-arms for the next.

Affected: [#9 in-do-object-cache](../reviews/in-do-object-cache.md), [#20 time-travel-refs](../reviews/time-travel-refs.md), [#23 hooks-as-workers](../reviews/hooks-as-workers.md), [#25 wasm-git-core](../reviews/wasm-git-core.md), [#30 ephemeral-repos](../reviews/ephemeral-repos.md), [#35 speculative-packs](../reviews/speculative-packs.md), [#44 ref-leases](../reviews/ref-leases.md), [#48 cross-repo-atomic-push](../reviews/cross-repo-atomic-push.md), [#49 github-webhook-compat](../reviews/github-webhook-compat.md), [#55 gc-and-repack-alarm](../reviews/gc-and-repack-alarm.md)

## git sends chunked and gzipped request bodies

**Hit 8 of 56 proofs.**

git gzips small POST bodies and sends chunked transfer for pushes over 1 MiB, so there is no content-length to hand R2.

**Fix once:** Honor Content-Encoding at the edge and ingest packs through R2 multipart upload with buffered parts of at least 5 MiB.

Affected: [#1 repo-do-ref-authority](../reviews/repo-do-ref-authority.md), [#3 protocol-v2-only](../reviews/protocol-v2-only.md), [#5 content-addressed-r2-keys](../reviews/content-addressed-r2-keys.md), [#6 two-phase-push](../reviews/two-phase-push.md), [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#28 scoped-token-remotes](../reviews/scoped-token-remotes.md), [#39 merkle-proofs](../reviews/merkle-proofs.md), [#41 branch-preview-workers](../reviews/branch-preview-workers.md)

## 1,000 subrequests per invocation; per-object R2 calls blow the cap

**Hit 7 of 56 proofs.**

R2 binding calls count as subrequests, capped at 1,000 per invocation. A blobless checkout, a 1,000-file workspace push, or a pack rebuild all issue one R2 call per object.

**Fix once:** Batch objects into packs and read them with coalesced range reads. Spread multi-thousand-object jobs across alarm slices with a durable cursor.

Affected: [#7 precomputed-clone-pack](../reviews/precomputed-clone-pack.md), [#10 partial-clone-filters](../reviews/partial-clone-filters.md), [#29 tui-rpc-push](../reviews/tui-rpc-push.md), [#35 speculative-packs](../reviews/speculative-packs.md), [#41 branch-preview-workers](../reviews/branch-preview-workers.md), [#43 server-side-bisect](../reviews/server-side-bisect.md), [#52 offline-browser-client](../reviews/offline-browser-client.md)

## ctx.id.name is undefined inside a DO made via idFromName

**Hit 5 of 56 proofs.**

Inside a DO created via idFromName, ctx.id.name is undefined. Proofs that built R2 prefixes from it wrote to objects/undefined.

**Fix once:** Persist owner and repo into SQLite on the first request and read them from there.

Affected: [#8 pinned-delta-bases](../reviews/pinned-delta-bases.md), [#21 r2-versioned-snapshots](../reviews/r2-versioned-snapshots.md), [#30 ephemeral-repos](../reviews/ephemeral-repos.md), [#36 federated-gossip](../reviews/federated-gossip.md), [#48 cross-repo-atomic-push](../reviews/cross-repo-atomic-push.md)

