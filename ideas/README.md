# Idea catalog

The 56 ideas as they went into the proof and review workflow. Tiers: **foundation** is the trunk everything roots into, **edge** is what only exists because it is serverless and at the edge, **wild** is exotic.


## foundation

- **#1 One Durable Object per repo as the ref authority** (`repo-do-ref-authority`). Git consistency reduces to compare-and-swap on refs. A single DO per repo serializes every push without distributed locks.
- **#2 Refs in DO SQLite, objects in R2** (`refs-sqlite-objects-r2`). Refs are tiny and hot (DO SQLite); objects are large and immutable (R2).
- **#3 Speak git protocol v2 only, translate v0 at the edge** (`protocol-v2-only`). v2 is command-based (ls-refs, fetch, object-info) and maps onto stateless request/response Workers. Legacy v0 dumb/smart clients get a thin shim.
- **#4 Packfile parsing in a Worker with a streaming inflater** (`streaming-pack-parser`). A push arrives as one packfile in the git-receive-pack body. Index it on the fly (header, per-object zlib inflate via DecompressionStream, ref-delta/ofs-delta resolution), write objects to R2, then flip refs in the DO.
- **#5 Content-addressed R2 keys** (`content-addressed-r2-keys`). Key objects by SHA so writes are idempotent and retried pushes are harmless.
- **#6 Two-phase push** (`two-phase-push`). Phase one writes objects to R2 under a pending prefix. Phase two asks the DO to validate connectivity and atomically advance refs. Crash between phases leaves orphans a janitor alarm sweeps.
- **#7 Precomputed pack slices for clone** (`precomputed-clone-pack`). Keep a periodically rebuilt full pack in R2 and serve fresh clones as a single R2 range-read stream.
- **#8 Delta bases pinned per repo** (`pinned-delta-bases`). Store a small set of hot base objects in DO storage so incremental fetches can delta against them without R2 round trips.
- **#9 Tiny in-DO object cache with alarm-driven eviction** (`in-do-object-cache`). Hot blobs (package.json, lockfiles) served from DO memory/storage; the alarm evicts on a schedule.
- **#10 Shallow and partial clone as first-class filters** (`partial-clone-filters`). blob:none and tree:0 filters map to R2 lazy fetches; each later blob request is exactly one R2 GET.
- **#11 Git LFS natively via presigned R2 URLs** (`native-lfs`). Implement the LFS batch API; objects are already in R2, so upload/download actions are presigned R2 URLs.
- **#12 Bundle-URI support** (`bundle-uri`). Advertise bundle-uri so clients pull the bulk pack from an R2 public bucket via CDN, then do a small incremental fetch through the DO.
- **#53 The /info/refs?service= entrypoint and pkt-line codec** (`info-refs-endpoint`). Implement pkt-line framing, capability advertisement, and the smart HTTP handshake in a Worker.
- **#54 Auth and multi-tenancy: owner/repo routing to DO ids** (`auth-and-multitenancy`). idFromName(owner/repo), token verification at the edge, per-repo ACL in DO SQLite.

## edge

- **#13 Refs replicated to every region via KV and DO location hints** (`replicated-refs-edge`). Reads from nearest edge (KV replica of refs), writes to the single authority DO.
- **#14 Push-triggered CI as a DO alarm chain** (`alarm-chain-ci`). A push schedules alarms; each alarm is a stage. No queue service.
- **#15 Live fetch over hibernating WebSockets** (`live-fetch-websocket`). Client holds a hibernated WebSocket to the repo DO and is nudged the instant a ref moves.
- **#16 Branch-level Durable Objects for monorepos** (`branch-level-dos`). Shard refs by namespace so a busy branch does not block a quiet one.
- **#17 Server-side three-way merge in the Worker** (`server-side-merge`). Push with a push-option asks the server to merge into main and reject only on real conflicts.
- **#18 Server-side rebase and squash as protocol v2 extensions** (`server-side-rebase`). Advertised capability; unknown to old clients.
- **#19 Signed refs by default with append-only DO reflog** (`signed-reflog`). Every ref move emits a signed record into an append-only log in DO SQLite.
- **#20 Time-travel refs** (`time-travel-refs`). refs/at/<timestamp>/main resolves from the reflog at fetch time.
- **#21 Snapshots via R2 object versioning of ref state** (`r2-versioned-snapshots`). Every ref flip writes a refs snapshot to R2 so the DO state is recoverable if wiped.
- **#22 Copy-on-write forks** (`cow-forks`). A fork is a new DO pointing at the parent's R2 prefix, writing only new objects to its own prefix.
- **#23 Pre/post-receive hooks as Workers via service bindings** (`hooks-as-workers`). Users deploy a Worker and register it; the repo DO calls it with the ref update payload.
- **#24 Per-blob presigned direct upload for giant pushes** (`presigned-direct-upload`). Client PUTs blobs straight to R2 with presigned URLs, then sends the DO a manifest.
- **#25 Wasm git core (gitoxide/libgit2) for delta resolution and merge** (`wasm-git-core`). Compile git internals to Wasm for heavy lifting; TypeScript for protocol.
- **#26 Search index built on push (D1 FTS / Vectorize)** (`search-index-on-push`). Post-receive tokenizes changed blobs into a search index.
- **#27 Diff API served with R2 range reads** (`diff-api-range-reads`). Serve diffs without reconstructing full files when packs already store deltas.
- **#28 Rate-limited, token-scoped remote URLs** (`scoped-token-remotes`). A URL with a scoped token in the path that can only push one branch for one hour.
- **#29 Push from a sibling workspace DO over RPC, no HTTP** (`tui-rpc-push`). The grok-pi workspace DO pushes to the repo DO via DO RPC.
- **#55 GC and repack as a DO alarm** (`gc-and-repack-alarm`). Periodic alarm walks reachable objects, writes a new pack to R2, deletes unreachable loose objects.
- **#56 Want/have negotiation with a commit-graph in SQLite** (`want-have-negotiation`). Compute the set of objects to send using a commit graph table instead of walking R2.

## wild

- **#30 Ephemeral repos with a self-destruct alarm** (`ephemeral-repos`). A repo that lives for an hour, then deletes its R2 prefix and DO state.
- **#31 Multi-writer CRDT branches** (`crdt-branches`). A branch tip is a CRDT document; the DO merges concurrent edits and materializes a real commit on demand.
- **#32 Agent-native protocol v2 commands (search, explain-diff, suggest-merge)** (`agent-native-commands`). New v2 commands so an agent harness talks git like it talks tools.
- **#33 Semantic diffs via tree-sitter in Wasm** (`semantic-diffs`). Function-level diffs for Ruby and JS computed server-side.
- **#34 Commit-as-event-stream** (`commit-event-stream`). Every commit fans out to Queues, Workers AI, webhooks.
- **#35 Speculative packs** (`speculative-packs`). Predict a client's next fetch from previous negotiation and prewarm R2 ranges into the DO cache.
- **#36 Federated remotes via DO-to-DO gossip** (`federated-gossip`). Repos subscribe to each other's ref changes; a mesh of mirrors with no hub.
- **#37 Encrypted-at-rest with client-held keys** (`client-key-encryption`). Objects encrypted before reaching R2; the DO only sees hashes.
- **#38 Deduplication across all repos in one content-addressed bucket** (`global-dedup`). Every copy of lodash stored exactly once, ever.
- **#39 Merkle inclusion proofs on fetch** (`merkle-proofs`). Return a proof so a client can verify a partial clone without trusting the server.
- **#40 Zero-clone execution: repo as a virtual filesystem inside an agent DO** (`zero-clone-vfs`). Read files by hash straight from R2; no checkout ever.
- **#41 Branch previews deployed as Workers on push** (`branch-preview-workers`). Push to preview/foo and a hook deploys the tree via Workers for Platforms.
- **#42 Git as a database driver (ActiveRecord / JS ORM adapter)** (`git-as-db-driver`). Every save is a commit, every query a tree walk, migrations are rebases.
- **#43 Bisect on the server with parallel test Workers** (`server-side-bisect`). The DO runs a test Worker against midpoints in parallel and returns the culprit.
- **#44 Ref leases** (`ref-leases`). Lock a branch for one client for N minutes, enforced by the DO.
- **#45 Commit graph in Vectorize for semantic git log** (`vectorized-commit-graph`). Embed messages and diffs so log --semantic works.
- **#46 Time-boxed history with cold-storage checkpoints** (`time-boxed-history`). Squash commits older than a year into checkpoints, keep full objects in cold R2.
- **#47 Pull-request review data as git objects under refs/reviews** (`reviews-as-refs`). A clone carries its own code review history.
- **#48 Cross-repo atomic pushes** (`cross-repo-atomic-push`). Push to three repos in one request; DOs coordinate a small two-phase commit.
- **#49 GitHub-compatible webhook payloads** (`github-webhook-compat`). Emit the same payloads so existing tooling works.
- **#50 Storage tiering by heat** (`storage-tiering`). Untouched objects migrate to R2 infrequent access; hot objects stay in DO.
- **#51 Blame that knows which agent wrote each line** (`agent-blame`). Commit trailers carry agent session IDs; exposed via a protocol command.
- **#52 Offline-first browser client with OPFS and the same Wasm core** (`offline-browser-client`). Same protocol, same core, browser host.
