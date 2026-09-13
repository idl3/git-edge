# Build order

Derived from the dependency graph and the verdicts. Each wave assumes the one before it is green against a stock git client.

## Wave 0: Handshake and spine

The smart-HTTP entrypoint, one DO per repo with ref CAS, refs in SQLite and objects in R2, the streaming pack parser, two-phase push and negotiation. Nothing else exists until git clone and git push work against this.

- [#53 The /info/refs?service= entrypoint and pkt-line codec](../proofs/info-refs-endpoint.md) · lands · days
- [#54 Auth and multi-tenancy: owner/repo routing to DO ids](../proofs/auth-and-multitenancy.md) · lands with caveats · days
- [#1 One Durable Object per repo as the ref authority](../proofs/repo-do-ref-authority.md) · lands with caveats · weeks
- [#2 Refs in DO SQLite, objects in R2](../proofs/refs-sqlite-objects-r2.md) · lands with caveats · weeks
- [#5 Content-addressed R2 keys](../proofs/content-addressed-r2-keys.md) · lands with caveats · days
- [#4 Packfile parsing in a Worker with a streaming inflater](../proofs/streaming-pack-parser.md) · lands with caveats · weeks
- [#6 Two-phase push](../proofs/two-phase-push.md) · lands with caveats · weeks
- [#56 Want/have negotiation with a commit-graph in SQLite](../proofs/want-have-negotiation.md) · lands with caveats · weeks
- [#3 Speak git protocol v2 only, translate v0 at the edge](../proofs/protocol-v2-only.md) · risky · weeks

## Wave 1: Clone at scale

Packs, GC and the fast clone paths. This is where the pack index and the alarm dispatcher earn their keep.

- [#55 GC and repack as a DO alarm](../proofs/gc-and-repack-alarm.md) · risky · weeks
- [#7 Precomputed pack slices for clone](../proofs/precomputed-clone-pack.md) · lands with caveats · weeks
- [#12 Bundle-URI support](../proofs/bundle-uri.md) · lands with caveats · days
- [#8 Delta bases pinned per repo](../proofs/pinned-delta-bases.md) · lands with caveats · weeks
- [#10 Shallow and partial clone as first-class filters](../proofs/partial-clone-filters.md) · risky · weeks
- [#11 Git LFS natively via presigned R2 URLs](../proofs/native-lfs.md) · lands with caveats · days
- [#13 Refs replicated to every region via KV and DO location hints](../proofs/replicated-refs-edge.md) · lands with caveats · days
- [#9 Tiny in-DO object cache with alarm-driven eviction](../proofs/in-do-object-cache.md) · risky · days

## Wave 2: Cheap wins on the DO

Everything here is a few extra SQLite rows in the same transaction as the ref CAS, or an outbox drained by an alarm. Days each, not weeks.

- [#44 Ref leases](../proofs/ref-leases.md) · lands with caveats · days
- [#20 Time-travel refs](../proofs/time-travel-refs.md) · lands with caveats · days
- [#19 Signed refs by default with append-only DO reflog](../proofs/signed-reflog.md) · lands with caveats · weeks
- [#21 Snapshots via R2 object versioning of ref state](../proofs/r2-versioned-snapshots.md) · lands with caveats · days
- [#28 Rate-limited, token-scoped remote URLs](../proofs/scoped-token-remotes.md) · lands with caveats · days
- [#34 Commit-as-event-stream](../proofs/commit-event-stream.md) · lands with caveats · days
- [#49 GitHub-compatible webhook payloads](../proofs/github-webhook-compat.md) · lands with caveats · days
- [#23 Pre/post-receive hooks as Workers via service bindings](../proofs/hooks-as-workers.md) · risky · weeks
- [#14 Push-triggered CI as a DO alarm chain](../proofs/alarm-chain-ci.md) · lands with caveats · weeks
- [#15 Live fetch over hibernating WebSockets](../proofs/live-fetch-websocket.md) · lands with caveats · weeks
- [#26 Search index built on push (D1 FTS / Vectorize)](../proofs/search-index-on-push.md) · lands with caveats · days
- [#30 Ephemeral repos with a self-destruct alarm](../proofs/ephemeral-repos.md) · lands with caveats · days
- [#29 Push from a sibling workspace DO over RPC, no HTTP](../proofs/tui-rpc-push.md) · lands with caveats · weeks
- [#50 Storage tiering by heat](../proofs/storage-tiering.md) · lands with caveats · weeks

## Wave 3: Needs the Wasm core

Merge, rebase, diff and blame all need real git object algebra. Land gitoxide in Wasm once, behind a host-does-IO boundary, and these unlock together.

- [#25 Wasm git core (gitoxide/libgit2) for delta resolution and merge](../proofs/wasm-git-core.md) · risky · weeks
- [#17 Server-side three-way merge in the Worker](../proofs/server-side-merge.md) · risky · weeks
- [#18 Server-side rebase and squash as protocol v2 extensions](../proofs/server-side-rebase.md) · risky · weeks
- [#27 Diff API served with R2 range reads](../proofs/diff-api-range-reads.md) · risky · weeks
- [#33 Semantic diffs via tree-sitter in Wasm](../proofs/semantic-diffs.md) · risky · weeks
- [#51 Blame that knows which agent wrote each line](../proofs/agent-blame.md) · risky · weeks
- [#40 Zero-clone execution: repo as a virtual filesystem inside an agent DO](../proofs/zero-clone-vfs.md) · risky · weeks
- [#52 Offline-first browser client with OPFS and the same Wasm core](../proofs/offline-browser-client.md) · risky · months
- [#39 Merkle inclusion proofs on fetch](../proofs/merkle-proofs.md) · lands with caveats · weeks

## Wave 4: Exotic, still reachable

Reviewed as risky or caveated, but each has a plausible route once the waves above are solid.

- [#32 Agent-native protocol v2 commands (search, explain-diff, suggest-merge)](../proofs/agent-native-commands.md) · lands with caveats · weeks
- [#45 Commit graph in Vectorize for semantic git log](../proofs/vectorized-commit-graph.md) · lands with caveats · weeks
- [#47 Pull-request review data as git objects under refs/reviews](../proofs/reviews-as-refs.md) · lands with caveats · weeks
- [#38 Deduplication across all repos in one content-addressed bucket](../proofs/global-dedup.md) · lands with caveats · weeks
- [#22 Copy-on-write forks](../proofs/cow-forks.md) · risky · weeks
- [#16 Branch-level Durable Objects for monorepos](../proofs/branch-level-dos.md) · risky · weeks
- [#24 Per-blob presigned direct upload for giant pushes](../proofs/presigned-direct-upload.md) · risky · weeks
- [#35 Speculative packs](../proofs/speculative-packs.md) · risky · weeks
- [#43 Bisect on the server with parallel test Workers](../proofs/server-side-bisect.md) · lands with caveats · weeks
- [#41 Branch previews deployed as Workers on push](../proofs/branch-preview-workers.md) · lands with caveats · weeks
- [#46 Time-boxed history with cold-storage checkpoints](../proofs/time-boxed-history.md) · lands with caveats · weeks
- [#48 Cross-repo atomic pushes](../proofs/cross-repo-atomic-push.md) · risky · weeks
- [#36 Federated remotes via DO-to-DO gossip](../proofs/federated-gossip.md) · risky · weeks
- [#37 Encrypted-at-rest with client-held keys](../proofs/client-key-encryption.md) · risky · weeks
- [#42 Git as a database driver (ActiveRecord / JS ORM adapter)](../proofs/git-as-db-driver.md) · risky · weeks

## Wave 5: Does not land as stated

Stock git rejects divergent pushes client-side, so a live CRDT branch cannot be driven by plain git. The closest thing that works is a CRDT document materialized into a normal branch by the DO.

- [#31 Multi-writer CRDT branches](../proofs/crdt-branches.md) · does not land · weeks

