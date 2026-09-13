# Dependency graph

Edges point from an idea to the ideas it depends on, as declared by each proof. Rendered by GitHub as Mermaid.

```mermaid
graph LR
  subgraph foundation
    repo_do_ref_authority["#1 One Durable Object per repo as the ref authority"]
    refs_sqlite_objects_r2["#2 Refs in DO SQLite, objects in R2"]
    protocol_v2_only["#3 Speak git protocol v2 only, translate v0 at the edge"]
    streaming_pack_parser["#4 Packfile parsing in a Worker with a streaming inflater"]
    content_addressed_r2_keys["#5 Content-addressed R2 keys"]
    two_phase_push["#6 Two-phase push"]
    precomputed_clone_pack["#7 Precomputed pack slices for clone"]
    pinned_delta_bases["#8 Delta bases pinned per repo"]
    in_do_object_cache["#9 Tiny in-DO object cache with alarm-driven eviction"]
    partial_clone_filters["#10 Shallow and partial clone as first-class filters"]
    native_lfs["#11 Git LFS natively via presigned R2 URLs"]
    bundle_uri["#12 Bundle-URI support"]
    info_refs_endpoint["#53 The /info/refs?service= entrypoint and pkt-line codec"]
    auth_and_multitenancy["#54 Auth and multi-tenancy: owner/repo routing to DO ids"]
  end
  subgraph edge
    replicated_refs_edge["#13 Refs replicated to every region via KV and DO location hints"]
    alarm_chain_ci["#14 Push-triggered CI as a DO alarm chain"]
    live_fetch_websocket["#15 Live fetch over hibernating WebSockets"]
    branch_level_dos["#16 Branch-level Durable Objects for monorepos"]
    server_side_merge["#17 Server-side three-way merge in the Worker"]
    server_side_rebase["#18 Server-side rebase and squash as protocol v2 extensions"]
    signed_reflog["#19 Signed refs by default with append-only DO reflog"]
    time_travel_refs["#20 Time-travel refs"]
    r2_versioned_snapshots["#21 Snapshots via R2 object versioning of ref state"]
    cow_forks["#22 Copy-on-write forks"]
    hooks_as_workers["#23 Pre/post-receive hooks as Workers via service bindings"]
    presigned_direct_upload["#24 Per-blob presigned direct upload for giant pushes"]
    wasm_git_core["#25 Wasm git core (gitoxide/libgit2) for delta resolution and merge"]
    search_index_on_push["#26 Search index built on push (D1 FTS / Vectorize)"]
    diff_api_range_reads["#27 Diff API served with R2 range reads"]
    scoped_token_remotes["#28 Rate-limited, token-scoped remote URLs"]
    tui_rpc_push["#29 Push from a sibling workspace DO over RPC, no HTTP"]
    gc_and_repack_alarm["#55 GC and repack as a DO alarm"]
    want_have_negotiation["#56 Want/have negotiation with a commit-graph in SQLite"]
  end
  subgraph wild
    ephemeral_repos["#30 Ephemeral repos with a self-destruct alarm"]
    crdt_branches["#31 Multi-writer CRDT branches"]
    agent_native_commands["#32 Agent-native protocol v2 commands (search, explain-diff, suggest-merge)"]
    semantic_diffs["#33 Semantic diffs via tree-sitter in Wasm"]
    commit_event_stream["#34 Commit-as-event-stream"]
    speculative_packs["#35 Speculative packs"]
    federated_gossip["#36 Federated remotes via DO-to-DO gossip"]
    client_key_encryption["#37 Encrypted-at-rest with client-held keys"]
    global_dedup["#38 Deduplication across all repos in one content-addressed bucket"]
    merkle_proofs["#39 Merkle inclusion proofs on fetch"]
    zero_clone_vfs["#40 Zero-clone execution: repo as a virtual filesystem inside an agent DO"]
    branch_preview_workers["#41 Branch previews deployed as Workers on push"]
    git_as_db_driver["#42 Git as a database driver (ActiveRecord / JS ORM adapter)"]
    server_side_bisect["#43 Bisect on the server with parallel test Workers"]
    ref_leases["#44 Ref leases"]
    vectorized_commit_graph["#45 Commit graph in Vectorize for semantic git log"]
    time_boxed_history["#46 Time-boxed history with cold-storage checkpoints"]
    reviews_as_refs["#47 Pull-request review data as git objects under refs/reviews"]
    cross_repo_atomic_push["#48 Cross-repo atomic pushes"]
    github_webhook_compat["#49 GitHub-compatible webhook payloads"]
    storage_tiering["#50 Storage tiering by heat"]
    agent_blame["#51 Blame that knows which agent wrote each line"]
    offline_browser_client["#52 Offline-first browser client with OPFS and the same Wasm core"]
  end
  repo_do_ref_authority --> refs_sqlite_objects_r2
  repo_do_ref_authority --> content_addressed_r2_keys
  repo_do_ref_authority --> info_refs_endpoint
  repo_do_ref_authority --> auth_and_multitenancy
  repo_do_ref_authority --> two_phase_push
  refs_sqlite_objects_r2 --> repo_do_ref_authority
  refs_sqlite_objects_r2 --> content_addressed_r2_keys
  refs_sqlite_objects_r2 --> streaming_pack_parser
  refs_sqlite_objects_r2 --> info_refs_endpoint
  protocol_v2_only --> repo_do_ref_authority
  protocol_v2_only --> refs_sqlite_objects_r2
  protocol_v2_only --> info_refs_endpoint
  protocol_v2_only --> want_have_negotiation
  protocol_v2_only --> precomputed_clone_pack
  streaming_pack_parser --> info_refs_endpoint
  streaming_pack_parser --> content_addressed_r2_keys
  streaming_pack_parser --> two_phase_push
  streaming_pack_parser --> repo_do_ref_authority
  streaming_pack_parser --> refs_sqlite_objects_r2
  content_addressed_r2_keys --> streaming_pack_parser
  content_addressed_r2_keys --> refs_sqlite_objects_r2
  content_addressed_r2_keys --> repo_do_ref_authority
  two_phase_push --> repo_do_ref_authority
  two_phase_push --> refs_sqlite_objects_r2
  two_phase_push --> streaming_pack_parser
  two_phase_push --> content_addressed_r2_keys
  two_phase_push --> info_refs_endpoint
  precomputed_clone_pack --> repo_do_ref_authority
  precomputed_clone_pack --> refs_sqlite_objects_r2
  precomputed_clone_pack --> protocol_v2_only
  precomputed_clone_pack --> info_refs_endpoint
  precomputed_clone_pack --> gc_and_repack_alarm
  precomputed_clone_pack --> want_have_negotiation
  pinned_delta_bases --> streaming_pack_parser
  pinned_delta_bases --> content_addressed_r2_keys
  pinned_delta_bases --> refs_sqlite_objects_r2
  pinned_delta_bases --> want_have_negotiation
  pinned_delta_bases --> protocol_v2_only
  in_do_object_cache --> repo_do_ref_authority
  in_do_object_cache --> refs_sqlite_objects_r2
  in_do_object_cache --> content_addressed_r2_keys
  in_do_object_cache --> streaming_pack_parser
  partial_clone_filters --> content_addressed_r2_keys
  partial_clone_filters --> refs_sqlite_objects_r2
  partial_clone_filters --> protocol_v2_only
  partial_clone_filters --> streaming_pack_parser
  partial_clone_filters --> want_have_negotiation
  partial_clone_filters --> precomputed_clone_pack
  native_lfs --> repo_do_ref_authority
  native_lfs --> auth_and_multitenancy
  native_lfs --> content_addressed_r2_keys
  native_lfs --> refs_sqlite_objects_r2
  bundle_uri --> gc_and_repack_alarm
  bundle_uri --> precomputed_clone_pack
  bundle_uri --> refs_sqlite_objects_r2
  bundle_uri --> protocol_v2_only
  bundle_uri --> info_refs_endpoint
  bundle_uri --> want_have_negotiation
  replicated_refs_edge --> repo_do_ref_authority
  replicated_refs_edge --> refs_sqlite_objects_r2
  replicated_refs_edge --> protocol_v2_only
  replicated_refs_edge --> info_refs_endpoint
  replicated_refs_edge --> two_phase_push
  replicated_refs_edge --> auth_and_multitenancy
  alarm_chain_ci --> repo_do_ref_authority
  alarm_chain_ci --> refs_sqlite_objects_r2
  alarm_chain_ci --> content_addressed_r2_keys
  alarm_chain_ci --> two_phase_push
  alarm_chain_ci --> hooks_as_workers
  live_fetch_websocket --> repo_do_ref_authority
  live_fetch_websocket --> refs_sqlite_objects_r2
  live_fetch_websocket --> protocol_v2_only
  live_fetch_websocket --> info_refs_endpoint
  live_fetch_websocket --> want_have_negotiation
  live_fetch_websocket --> two_phase_push
  branch_level_dos --> repo_do_ref_authority
  branch_level_dos --> refs_sqlite_objects_r2
  branch_level_dos --> content_addressed_r2_keys
  branch_level_dos --> two_phase_push
  branch_level_dos --> streaming_pack_parser
  branch_level_dos --> protocol_v2_only
  branch_level_dos --> auth_and_multitenancy
  branch_level_dos --> cross_repo_atomic_push
  server_side_merge --> two_phase_push
  server_side_merge --> streaming_pack_parser
  server_side_merge --> repo_do_ref_authority
  server_side_merge --> refs_sqlite_objects_r2
  server_side_merge --> content_addressed_r2_keys
  server_side_merge --> want_have_negotiation
  server_side_merge --> info_refs_endpoint
  server_side_merge --> auth_and_multitenancy
  server_side_rebase --> repo_do_ref_authority
  server_side_rebase --> refs_sqlite_objects_r2
  server_side_rebase --> protocol_v2_only
  server_side_rebase --> content_addressed_r2_keys
  server_side_rebase --> want_have_negotiation
  server_side_rebase --> server_side_merge
  server_side_rebase --> wasm_git_core
  server_side_rebase --> in_do_object_cache
  signed_reflog --> repo_do_ref_authority
  signed_reflog --> refs_sqlite_objects_r2
  signed_reflog --> two_phase_push
  signed_reflog --> auth_and_multitenancy
  signed_reflog --> r2_versioned_snapshots
  time_travel_refs --> repo_do_ref_authority
  time_travel_refs --> refs_sqlite_objects_r2
  time_travel_refs --> protocol_v2_only
  time_travel_refs --> gc_and_repack_alarm
  r2_versioned_snapshots --> repo_do_ref_authority
  r2_versioned_snapshots --> refs_sqlite_objects_r2
  r2_versioned_snapshots --> content_addressed_r2_keys
  r2_versioned_snapshots --> two_phase_push
  cow_forks --> refs_sqlite_objects_r2
  cow_forks --> content_addressed_r2_keys
  cow_forks --> repo_do_ref_authority
  cow_forks --> gc_and_repack_alarm
  cow_forks --> streaming_pack_parser
  cow_forks --> want_have_negotiation
  hooks_as_workers --> two_phase_push
  hooks_as_workers --> repo_do_ref_authority
  hooks_as_workers --> auth_and_multitenancy
  hooks_as_workers --> scoped_token_remotes
  hooks_as_workers --> github_webhook_compat
  presigned_direct_upload --> two_phase_push
  presigned_direct_upload --> content_addressed_r2_keys
  presigned_direct_upload --> streaming_pack_parser
  presigned_direct_upload --> repo_do_ref_authority
  presigned_direct_upload --> native_lfs
  wasm_git_core --> streaming_pack_parser
  wasm_git_core --> two_phase_push
  wasm_git_core --> content_addressed_r2_keys
  wasm_git_core --> server_side_merge
  wasm_git_core --> repo_do_ref_authority
  search_index_on_push --> repo_do_ref_authority
  search_index_on_push --> refs_sqlite_objects_r2
  search_index_on_push --> content_addressed_r2_keys
  search_index_on_push --> streaming_pack_parser
  diff_api_range_reads --> streaming_pack_parser
  diff_api_range_reads --> refs_sqlite_objects_r2
  diff_api_range_reads --> gc_and_repack_alarm
  diff_api_range_reads --> in_do_object_cache
  scoped_token_remotes --> info_refs_endpoint
  scoped_token_remotes --> auth_and_multitenancy
  scoped_token_remotes --> repo_do_ref_authority
  scoped_token_remotes --> streaming_pack_parser
  scoped_token_remotes --> two_phase_push
  tui_rpc_push --> repo_do_ref_authority
  tui_rpc_push --> refs_sqlite_objects_r2
  tui_rpc_push --> content_addressed_r2_keys
  tui_rpc_push --> two_phase_push
  ephemeral_repos --> repo_do_ref_authority
  ephemeral_repos --> refs_sqlite_objects_r2
  ephemeral_repos --> auth_and_multitenancy
  ephemeral_repos --> info_refs_endpoint
  ephemeral_repos --> streaming_pack_parser
  ephemeral_repos --> scoped_token_remotes
  crdt_branches --> repo_do_ref_authority
  crdt_branches --> refs_sqlite_objects_r2
  crdt_branches --> content_addressed_r2_keys
  crdt_branches --> streaming_pack_parser
  crdt_branches --> info_refs_endpoint
  crdt_branches --> server_side_merge
  agent_native_commands --> protocol_v2_only
  agent_native_commands --> info_refs_endpoint
  agent_native_commands --> refs_sqlite_objects_r2
  agent_native_commands --> two_phase_push
  agent_native_commands --> server_side_merge
  agent_native_commands --> diff_api_range_reads
  agent_native_commands --> want_have_negotiation
  agent_native_commands --> search_index_on_push
  semantic_diffs --> refs_sqlite_objects_r2
  semantic_diffs --> content_addressed_r2_keys
  semantic_diffs --> diff_api_range_reads
  semantic_diffs --> wasm_git_core
  commit_event_stream --> repo_do_ref_authority
  commit_event_stream --> refs_sqlite_objects_r2
  commit_event_stream --> streaming_pack_parser
  commit_event_stream --> content_addressed_r2_keys
  speculative_packs --> want_have_negotiation
  speculative_packs --> streaming_pack_parser
  speculative_packs --> refs_sqlite_objects_r2
  speculative_packs --> repo_do_ref_authority
  speculative_packs --> protocol_v2_only
  speculative_packs --> two_phase_push
  federated_gossip --> repo_do_ref_authority
  federated_gossip --> refs_sqlite_objects_r2
  federated_gossip --> content_addressed_r2_keys
  federated_gossip --> two_phase_push
  federated_gossip --> streaming_pack_parser
  federated_gossip --> protocol_v2_only
  federated_gossip --> auth_and_multitenancy
  client_key_encryption --> repo_do_ref_authority
  client_key_encryption --> refs_sqlite_objects_r2
  client_key_encryption --> content_addressed_r2_keys
  client_key_encryption --> two_phase_push
  client_key_encryption --> want_have_negotiation
  client_key_encryption --> offline_browser_client
  global_dedup --> content_addressed_r2_keys
  global_dedup --> streaming_pack_parser
  global_dedup --> refs_sqlite_objects_r2
  global_dedup --> repo_do_ref_authority
  global_dedup --> want_have_negotiation
  global_dedup --> gc_and_repack_alarm
  merkle_proofs --> refs_sqlite_objects_r2
  merkle_proofs --> content_addressed_r2_keys
  merkle_proofs --> protocol_v2_only
  merkle_proofs --> partial_clone_filters
  merkle_proofs --> signed_reflog
  zero_clone_vfs --> content_addressed_r2_keys
  zero_clone_vfs --> refs_sqlite_objects_r2
  zero_clone_vfs --> repo_do_ref_authority
  zero_clone_vfs --> tui_rpc_push
  zero_clone_vfs --> streaming_pack_parser
  branch_preview_workers --> repo_do_ref_authority
  branch_preview_workers --> refs_sqlite_objects_r2
  branch_preview_workers --> content_addressed_r2_keys
  branch_preview_workers --> streaming_pack_parser
  branch_preview_workers --> hooks_as_workers
  branch_preview_workers --> alarm_chain_ci
  git_as_db_driver --> repo_do_ref_authority
  git_as_db_driver --> refs_sqlite_objects_r2
  git_as_db_driver --> content_addressed_r2_keys
  git_as_db_driver --> r2_versioned_snapshots
  git_as_db_driver --> gc_and_repack_alarm
  server_side_bisect --> repo_do_ref_authority
  server_side_bisect --> content_addressed_r2_keys
  server_side_bisect --> want_have_negotiation
  server_side_bisect --> hooks_as_workers
  server_side_bisect --> zero_clone_vfs
  server_side_bisect --> scoped_token_remotes
  ref_leases --> repo_do_ref_authority
  ref_leases --> two_phase_push
  ref_leases --> auth_and_multitenancy
  ref_leases --> info_refs_endpoint
  vectorized_commit_graph --> repo_do_ref_authority
  vectorized_commit_graph --> refs_sqlite_objects_r2
  vectorized_commit_graph --> streaming_pack_parser
  vectorized_commit_graph --> content_addressed_r2_keys
  vectorized_commit_graph --> two_phase_push
  vectorized_commit_graph --> want_have_negotiation
  vectorized_commit_graph --> agent_native_commands
  time_boxed_history --> want_have_negotiation
  time_boxed_history --> precomputed_clone_pack
  time_boxed_history --> gc_and_repack_alarm
  time_boxed_history --> refs_sqlite_objects_r2
  time_boxed_history --> content_addressed_r2_keys
  time_boxed_history --> storage_tiering
  reviews_as_refs --> repo_do_ref_authority
  reviews_as_refs --> refs_sqlite_objects_r2
  reviews_as_refs --> content_addressed_r2_keys
  reviews_as_refs --> streaming_pack_parser
  reviews_as_refs --> two_phase_push
  reviews_as_refs --> want_have_negotiation
  reviews_as_refs --> gc_and_repack_alarm
  cross_repo_atomic_push --> repo_do_ref_authority
  cross_repo_atomic_push --> refs_sqlite_objects_r2
  cross_repo_atomic_push --> content_addressed_r2_keys
  cross_repo_atomic_push --> two_phase_push
  cross_repo_atomic_push --> streaming_pack_parser
  cross_repo_atomic_push --> gc_and_repack_alarm
  cross_repo_atomic_push --> auth_and_multitenancy
  github_webhook_compat --> two_phase_push
  github_webhook_compat --> repo_do_ref_authority
  github_webhook_compat --> refs_sqlite_objects_r2
  github_webhook_compat --> streaming_pack_parser
  github_webhook_compat --> alarm_chain_ci
  github_webhook_compat --> commit_event_stream
  storage_tiering --> content_addressed_r2_keys
  storage_tiering --> refs_sqlite_objects_r2
  storage_tiering --> repo_do_ref_authority
  storage_tiering --> in_do_object_cache
  storage_tiering --> precomputed_clone_pack
  storage_tiering --> gc_and_repack_alarm
  agent_blame --> protocol_v2_only
  agent_blame --> refs_sqlite_objects_r2
  agent_blame --> two_phase_push
  agent_blame --> want_have_negotiation
  agent_blame --> agent_native_commands
  agent_blame --> scoped_token_remotes
  agent_blame --> diff_api_range_reads
  offline_browser_client --> wasm_git_core
  offline_browser_client --> streaming_pack_parser
  offline_browser_client --> protocol_v2_only
  offline_browser_client --> info_refs_endpoint
  offline_browser_client --> repo_do_ref_authority
  offline_browser_client --> two_phase_push
  offline_browser_client --> auth_and_multitenancy
  info_refs_endpoint --> repo_do_ref_authority
  info_refs_endpoint --> refs_sqlite_objects_r2
  info_refs_endpoint --> protocol_v2_only
  auth_and_multitenancy --> repo_do_ref_authority
  auth_and_multitenancy --> info_refs_endpoint
  gc_and_repack_alarm --> repo_do_ref_authority
  gc_and_repack_alarm --> refs_sqlite_objects_r2
  gc_and_repack_alarm --> content_addressed_r2_keys
  gc_and_repack_alarm --> two_phase_push
  gc_and_repack_alarm --> precomputed_clone_pack
  want_have_negotiation --> repo_do_ref_authority
  want_have_negotiation --> refs_sqlite_objects_r2
  want_have_negotiation --> streaming_pack_parser
  want_have_negotiation --> content_addressed_r2_keys
  want_have_negotiation --> protocol_v2_only
  want_have_negotiation --> info_refs_endpoint
```
