# Plain explainers

One page per idea, written for a reader who has never used git. Each page has an analogy, a diagram, the verdict, and every problem explained with its fix. Read [how-to-read.md](../findings/how-to-read.md) first.


## foundation

- [#1 One Durable Object per repo as the ref authority](./repo-do-ref-authority.md). lands with caveats.
- [#2 Refs in DO SQLite, objects in R2](./refs-sqlite-objects-r2.md). lands with caveats.
- [#3 Speak git protocol v2 only, translate v0 at the edge](./protocol-v2-only.md). risky.
- [#4 Packfile parsing in a Worker with a streaming inflater](./streaming-pack-parser.md). lands with caveats.
- [#5 Content-addressed R2 keys](./content-addressed-r2-keys.md). lands with caveats.
- [#6 Two-phase push](./two-phase-push.md). lands with caveats.
- [#7 Precomputed pack slices for clone](./precomputed-clone-pack.md). lands with caveats.
- [#8 Delta bases pinned per repo](./pinned-delta-bases.md). lands with caveats.
- [#9 Tiny in-DO object cache with alarm-driven eviction](./in-do-object-cache.md). risky.
- [#10 Shallow and partial clone as first-class filters](./partial-clone-filters.md). risky.
- [#11 Git LFS natively via presigned R2 URLs](./native-lfs.md). lands with caveats.
- [#12 Bundle-URI support](./bundle-uri.md). lands with caveats.
- [#53 The /info/refs?service= entrypoint and pkt-line codec](./info-refs-endpoint.md). lands.
- [#54 Auth and multi-tenancy: owner/repo routing to DO ids](./auth-and-multitenancy.md). lands with caveats.

## edge

- [#13 Refs replicated to every region via KV and DO location hints](./replicated-refs-edge.md). lands with caveats.
- [#14 Push-triggered CI as a DO alarm chain](./alarm-chain-ci.md). lands with caveats.
- [#15 Live fetch over hibernating WebSockets](./live-fetch-websocket.md). lands with caveats.
- [#16 Branch-level Durable Objects for monorepos](./branch-level-dos.md). risky.
- [#17 Server-side three-way merge in the Worker](./server-side-merge.md). risky.
- [#18 Server-side rebase and squash as protocol v2 extensions](./server-side-rebase.md). risky.
- [#19 Signed refs by default with append-only DO reflog](./signed-reflog.md). lands with caveats.
- [#20 Time-travel refs](./time-travel-refs.md). lands with caveats.
- [#21 Snapshots via R2 object versioning of ref state](./r2-versioned-snapshots.md). lands with caveats.
- [#22 Copy-on-write forks](./cow-forks.md). risky.
- [#23 Pre/post-receive hooks as Workers via service bindings](./hooks-as-workers.md). risky.
- [#24 Per-blob presigned direct upload for giant pushes](./presigned-direct-upload.md). risky.
- [#25 Wasm git core (gitoxide/libgit2) for delta resolution and merge](./wasm-git-core.md). risky.
- [#26 Search index built on push (D1 FTS / Vectorize)](./search-index-on-push.md). lands with caveats.
- [#27 Diff API served with R2 range reads](./diff-api-range-reads.md). risky.
- [#28 Rate-limited, token-scoped remote URLs](./scoped-token-remotes.md). lands with caveats.
- [#29 Push from a sibling workspace DO over RPC, no HTTP](./tui-rpc-push.md). lands with caveats.
- [#55 GC and repack as a DO alarm](./gc-and-repack-alarm.md). risky.
- [#56 Want/have negotiation with a commit-graph in SQLite](./want-have-negotiation.md). lands with caveats.

## wild

- [#30 Ephemeral repos with a self-destruct alarm](./ephemeral-repos.md). lands with caveats.
- [#31 Multi-writer CRDT branches](./crdt-branches.md). does not land.
- [#32 Agent-native protocol v2 commands (search, explain-diff, suggest-merge)](./agent-native-commands.md). lands with caveats.
- [#33 Semantic diffs via tree-sitter in Wasm](./semantic-diffs.md). risky.
- [#34 Commit-as-event-stream](./commit-event-stream.md). lands with caveats.
- [#35 Speculative packs](./speculative-packs.md). risky.
- [#36 Federated remotes via DO-to-DO gossip](./federated-gossip.md). risky.
- [#37 Encrypted-at-rest with client-held keys](./client-key-encryption.md). risky.
- [#38 Deduplication across all repos in one content-addressed bucket](./global-dedup.md). lands with caveats.
- [#39 Merkle inclusion proofs on fetch](./merkle-proofs.md). lands with caveats.
- [#40 Zero-clone execution: repo as a virtual filesystem inside an agent DO](./zero-clone-vfs.md). risky.
- [#41 Branch previews deployed as Workers on push](./branch-preview-workers.md). lands with caveats.
- [#42 Git as a database driver (ActiveRecord / JS ORM adapter)](./git-as-db-driver.md). risky.
- [#43 Bisect on the server with parallel test Workers](./server-side-bisect.md). lands with caveats.
- [#44 Ref leases](./ref-leases.md). lands with caveats.
- [#45 Commit graph in Vectorize for semantic git log](./vectorized-commit-graph.md). lands with caveats.
- [#46 Time-boxed history with cold-storage checkpoints](./time-boxed-history.md). lands with caveats.
- [#47 Pull-request review data as git objects under refs/reviews](./reviews-as-refs.md). lands with caveats.
- [#48 Cross-repo atomic pushes](./cross-repo-atomic-push.md). risky.
- [#49 GitHub-compatible webhook payloads](./github-webhook-compat.md). lands with caveats.
- [#50 Storage tiering by heat](./storage-tiering.md). lands with caveats.
- [#51 Blame that knows which agent wrote each line](./agent-blame.md). risky.
- [#52 Offline-first browser client with OPFS and the same Wasm core](./offline-browser-client.md). risky.
