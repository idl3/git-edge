# Rust for the edge git server: workers-rs + gitoxide (git2-rs considered and set aside)

Date: 2026-09-13. Scope: can the git smart-HTTP server (Workers + Durable Objects + R2, fully serverless) be written in Rust, and with what. The user has already chosen gitoxide over git2-rs; item 2 is therefore short. Everything below was checked against primary sources on the date above; anything I could not verify is marked as such.

Versions used throughout (crates.io, fetched 2026-09-13): `worker` 0.8.5, `gix-pack` 0.74.2, `gix-object` 0.64.1, `gix-hash` 0.26.2, `gix-packetline` 0.22.2, `gix-features` 0.49.1, `gix-traverse` 0.61.0, `gix-diff` 0.67.1, `gix-merge` 0.20.1, `gix-revision` 0.49.1, `gix-commitgraph` 0.39.0, `gix-refspec` 0.45.1, `gix-protocol` 0.65.1, `gix-transport` 0.59.2, `gix-odb` 0.84.0, `gix-ref` 0.67.1, `gix` 0.87.1, `imara-diff` 0.2.0, `git2` 0.21.0, `libgit2-sys` 0.18.8+1.9.7.

## 1. Cloudflare Workers in Rust: state of workers-rs

Source of truth: the `worker` crate docs (https://docs.rs/worker/latest/worker/, v0.8.5), the repo README (https://github.com/cloudflare/workers-rs), the repo source (`worker/src/durable.rs`, `worker/src/sql.rs`), and Cloudflare's Rust page (https://developers.cloudflare.com/workers/languages/rust/).

Model: the Worker is compiled to `wasm32-unknown-unknown`, `worker-build` generates a JS shim (`build/worker/shim.mjs`) and runs `wasm-pack` + `wasm-opt`; every host API is reached through `wasm-bindgen`/`js-sys`/`web-sys` bindings and async Rust is bridged to JS promises with `wasm-bindgen-futures`. There is no Tokio; the README FAQ: "All crates in your Worker project must compile to wasm32-unknown-unknown target." Panics abort the isolate unless built with `--panic-unwind`, which converts them to JS `PanicError` exceptions.

| Capability | Status in `worker` 0.8.5 | Evidence |
|---|---|---|
| Durable Objects (class, `fetch`) | Supported. `#[durable_object]` macro; trait `new(state, env)`, `async fn fetch(&self, req)`. | https://github.com/cloudflare/workers-rs/blob/main/worker/src/durable.rs |
| DO SQLite storage | Supported and synchronous. `state.storage().sql()` returns `SqlStorage` with `exec(query, bindings) -> Result<SqlCursor>` (non-async), `exec_raw`, `database_size`; `SqlCursor::{to_array, one, next, raw, column_names, rows_read, rows_written}`. Migration must use `new_sqlite_classes`. | https://github.com/cloudflare/workers-rs/blob/main/worker/src/sql.rs |
| DO alarms | Supported. `Storage::{get_alarm, set_alarm, delete_alarm}` (async) and `async fn alarm(&self)` on the trait. | https://docs.rs/worker/latest/worker/durable/struct.Storage.html |
| DO WebSocket hibernation | Supported. `State::{accept_web_socket, accept_websocket_with_tags, get_websockets, get_websockets_with_tag, get_tags, set_websocket_auto_response}` plus trait hooks `websocket_message`, `websocket_close`, `websocket_error`. | https://docs.rs/worker/latest/worker/durable/struct.State.html |
| DO `blockConcurrencyWhile` | Supported: `State::block_concurrency_while(future)` ("blocking delivery of any other events ... until it completes"). | `worker/src/durable.rs` |
| DO name (`id.name`) | No `State::name()`. Issue #760 was closed with the workaround (`js_sys::Reflect::get(id, "name")` on the inner JS id). Same trap as the TS proofs (defect 9). | https://github.com/cloudflare/workers-rs/issues/760 |
| DO RPC (typed methods on stubs) | JS-shim-only / experimental. README: "workers-rs has experimental support for Workers RPC ... relies on JavaScript bindings and may require some manual usage of wasm-bindgen"; "Not all features of RPC are supported yet ... Function arguments and return values, Class instances, Stub forwarding". Rust->DO calls in practice go through `Stub::fetch_with_request` (HTTP-shaped). Open issue #720 "No access to Env in #[wasm_bindgen] RPC functions". | https://github.com/cloudflare/workers-rs#rpc, https://github.com/cloudflare/workers-rs/issues/720 |
| DO synchronous KV API (`ctx.storage.kv`) | Missing (open feature request #967, Apr 2026). SQLite `exec` is sync, so this does not matter for us. | https://github.com/cloudflare/workers-rs/issues/967 |
| DO + `http` feature (axum inside a DO) | Gap: DO `fetch` does not use `http` types (open #582). Router works; axum-in-DO needs manual mapping. | https://github.com/cloudflare/workers-rs/issues/582 |
| R2 get / range | Supported. `Bucket::get(key) -> GetOptionsBuilder` with `.range(Range)` ("only a specific length (from an optional offset) or suffix") and `.only_if(Conditional)`; `execute() -> Result<Option<Object>>`; `ObjectBody::{bytes, stream, text}`. | https://docs.rs/worker/latest/worker/struct.GetOptionsBuilder.html |
| R2 put | Supported. `Bucket::put(key, impl Into<Data>)`; `Data::{ReadableStream, Stream(FixedLengthStream), Text, Bytes, Empty}`. A `FixedLengthStream` variant exists because R2 needs a known length for single-shot streaming puts. | https://docs.rs/worker/latest/worker/enum.Data.html |
| R2 multipart | Supported. `create_multipart_upload(key)`, `resume_multipart_upload(key, upload_id)`, `MultipartUpload::{upload_part(u16, Data) -> UploadedPart, complete(parts) -> Object, abort}`. Platform rules unchanged: parts >= 5 MiB except last, equal sizes, <= 10,000 parts. | https://docs.rs/worker/latest/worker/struct.MultipartUpload.html, https://developers.cloudflare.com/r2/objects/multipart-objects/ |
| Streaming request body | Supported. `Request::stream() -> ByteStream` (a `futures::Stream` of byte chunks); `bytes()/text()/json()` for buffered; `inner()` exposes `web_sys::Request`. | https://docs.rs/worker/latest/worker/struct.Request.html |
| Streaming response body | Supported. `Response::from_stream<S: TryStream>(s)` where `S::Ok: Into<Vec<u8>>`; with the `http` feature, `Body` wraps `web_sys::ReadableStream` and implements `http_body::Body`. | https://docs.rs/worker/latest/worker/struct.Response.html |
| Fetch, KV, D1, Queues, service bindings | Supported (`Fetcher`, `d1` and `queue` features). | README |

Production evidence that this stack holds up: Cloudflare's own Static-CT log, azul (https://github.com/cloudflare/azul), is a `cdylib` Worker on `worker = { features = ["http", "axum"] }` with a `wrangler.jsonc` (`compatibility_date` 2025-09-25) — `crates/ct_worker/Cargo.toml`. I did not independently confirm from its README which bindings it uses; the crate name `sequencer_do.rs` cited in workers-rs issue #760 indicates a Durable Object.

Size and startup (https://developers.cloudflare.com/workers/platform/limits/ and the 2026-09-04 changelog https://developers.cloudflare.com/changelog/post/2026-09-04-increased-worker-size-limit/): the compressed limits of 3 MB (Free) / 10 MB (Paid) that the 56 proofs assumed **no longer exist**. The only limit is 64 MiB uncompressed on both plans. Startup: "A Worker must parse and execute its global scope ... within 1 second" (raised from the old 400 ms). Memory: 128 MB per isolate "including the JavaScript heap and WebAssembly allocations". CPU: Free 10 ms; Paid 30 s default, 300 s via `limits.cpu_ms`; network waits do not count. Subrequests: Free 50, Paid 10,000 per invocation (the "1,000" figure in the defects file is out of date). Wasm on Workers: `WebAssembly.instantiate()` only on pre-compiled bundled modules, no threads/`SharedArrayBuffer`, SIMD on (https://developers.cloudflare.com/workers/runtime-apis/webassembly/). Whether Wasm compilation is counted inside the 1 s startup budget is **not documented**; Cloudflare's Rust page only says unoptimized Rust binaries "can be large and may exceed" limits and that `worker-build` runs `wasm-opt`. Plan for a 1-3 MB `.wasm` (gitoxide subset + wasm-bindgen glue with `opt-level="z"`, `lto`, `codegen-units=1`, `panic="abort"`) and measure startup with `wrangler deploy --dry-run --outdir` plus a cold-start probe; I could not build here, so treat the size figure as an estimate.

## 2. git2-rs / libgit2 on wasm32 — considered and set aside

Three hard reasons, each verified:

1. **libgit2-sys does not build for wasm32.** `libgit2-sys/build.rs` (https://raw.githubusercontent.com/rust-lang/git2-rs/master/libgit2-sys/build.rs) has no wasm32 branch: it unconditionally adds `src/util/unix` on non-Windows, defines `GIT_THREADS 1`, compiles bundled pcre2, llhttp, xdiff and SHA-1 collision detection, and expects `DEP_Z_INCLUDE`/OpenSSL/libssh2 for the `https`/`ssh` features. The `wasm32-unknown-unknown` target has no libc, so none of the POSIX I/O layer links. git2-rs issue #511 (2020, closed) says libgit2 "would probably need some POSIX JS emulation, like the one provided from emscripten", and #871 (2022, still open, https://github.com/rust-lang/git2-rs/issues/871) asks again with no solution; wasm-git issue #77 (https://github.com/petersalomonsen/wasm-git/issues/77) requesting git2-rs support is unresolved. The only working libgit2-on-Wasm is wasm-git (https://github.com/petersalomonsen/wasm-git): an Emscripten build with `-DTHREADSAFE=OFF -DUSE_HTTPS=OFF -DUSE_SSH=OFF -DREGEX_BACKEND=regcomp`, `-sFORCE_FILESYSTEM` with MEMFS/IDBFS/NODEFS, and `-s ASYNCIFY` or `-sJSPI` variants (https://github.com/petersalomonsen/wasm-git/blob/master/emscriptenbuild/build.sh). libgit2 upstream closed the Emscripten PR (#4400, https://github.com/libgit2/libgit2/pull/4400) unmerged. That is a C/Emscripten toolchain, not a Rust/`wasm-bindgen` one; you cannot link it into a workers-rs Worker.
2. **The backend extension points are synchronous C.** `git_odb_backend` (https://raw.githubusercontent.com/libgit2/libgit2/main/include/git2/sys/odb_backend.h, `GIT_ODB_BACKEND_VERSION 1`) and `git_refdb_backend` (https://raw.githubusercontent.com/libgit2/libgit2/main/include/git2/sys/refdb_backend.h) are public, supported, and would in principle allow an R2 ODB and a DO refdb — but every callback (`read`, `read_header`, `exists`, `write`, `lookup`, `iterator`, `write`/`lock`/`unlock`...) must return its result synchronously. On Workers, R2/DO/fetch are promise-based. Bridging requires suspending Wasm mid-call: Asyncify (an Emscripten/Binaryen whole-program transform, ~2x code size, one suspension at a time — https://emscripten.org/docs/porting/asyncify.html; it does run on Workers, e.g. quickjs-emscripten and DuckDB's @ducklings/workers use it) or JSPI. JSPI on workerd: the isolate setup header declares `static bool jspiEnabledCallback(v8::Local<v8::Context>)` (https://github.com/cloudflare/workerd/blob/main/src/workerd/jsg/setup.h) and Python Workers exercise JSPI via Pyodide `run_sync` (`src/workerd/server/tests/python/jspi/`). I could **not** find the body of that callback, any compatibility flag in `compatibility-date.capnp` that enables JSPI for JS/Wasm Workers, or any Cloudflare documentation of `WebAssembly.Suspending`. Treat JSPI as **not available to Rust Workers today**; Asyncify is available only to Emscripten C builds and would still mean a libgit2 that thinks it is talking to a filesystem.
3. **Even if it linked, it is the wrong shape.** libgit2's ODB/refdb still layer on `git_repository` which wants a working directory, config files, `mmap`, temp files and a packfile writer; the proofs' storage model (loose objects in R2, refs in a DO, one pack index in SQLite) would be re-implemented underneath a library that then re-does delta resolution and negotiation in its own memory. The wasm-git binary is several MB and needs a POSIX shim; the sandboxing and 128 MB isolate cap make that a poor fit.

Where git2-rs still earns its keep (all off-Worker): a `git2`-based conformance harness that clones/pushes against the edge server from CI (real libgit2 client behaviour differs from git's, so it is a second oracle); a local admin CLI for repository import/export; and, only if "fully serverless" is ever relaxed, a Cloudflare Containers sidecar for repack/gc where libgit2 or the git binary itself can run natively.

## 3. gitoxide: which plumbing crates compile to wasm32-unknown-unknown, and what each gives us

Source of truth: the `wasm` job in `.github/workflows/ci.yml` (https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/.github/workflows/ci.yml), which builds against `wasm32-unknown-unknown`, `wasm32-wasip1` and `wasm32-wasip2`, plus each crate's `Cargo.toml`.

CI-verified for `wasm32-unknown-unknown` (exact commands from the workflow):

| Crate | CI command | What it gives the server |
|---|---|---|
| gix-hash | `--features sha1` | `ObjectId`/`oid`, hex parsing, hasher (`sha1-checked`, collision-detecting SHA-1 like git). |
| gix-object | `--features sha1` | Parse/encode commit, tree, tag, blob (`ObjectRef`, `TreeRef`, `CommitRef`), `compute_hash(kind, data)`, `encode::loose_header`, `WriteTo`, and the synchronous `Find`/`FindExt`/`Exists` traits (`try_find(&self, id, &mut Vec<u8>) -> Result<Option<Data>>`, https://docs.rs/gix-object/latest/gix_object/trait.Find.html). |
| gix-pack | `--features sha1,wasm` and `--all-features` | The big win: `data::input::BytesToEntriesIter::new_from_header(read: BufRead, Mode, EntryDataMode, object_hash)` streams pack entries from any `BufRead` (so from a request body buffered chunk-wise), `data::delta::{apply, decode_header_size}`, `data::entry::Header` (ofs/ref delta decoding), `data::output` (encode entries and write a pack to `Write`), `index` (v2 `.idx` reading and writing), `multi_index`, `verify`, `cache`. Feature `wasm` (`wasm = ["gix-diff?/wasm"]`) exists precisely to make wasm32 build: `gix-tempfile` is only a dependency on `cfg(not(target_arch = "wasm32"))`, and `bundle::write` — the on-disk pack+index writer — is compiled out with `#[cfg(all(not(feature = "wasm"), feature = "streaming-input"))]` (`gix-pack/src/bundle/mod.rs`). Inflate/deflate come from `gix-zlib` 0.1.0, a pure-Rust `zlib-rs` wrapper, so no C zlib. `parallel` is a feature; leave it off (no threads on Workers). |
| gix-packetline | no features | pkt-line codec: `PacketLineRef`, `decode`, `encode::{data_to_write, band_to_write, flush_to_write, delim_to_write, response_end_to_write}`, `Channel`/`BandRef` sideband types, `MAX_LINE_LEN`/`MAX_DATA_LEN`. `Writer` and `StreamingPeekableIter` need `blocking-io` (std `Read`/`Write`, fine over `Vec<u8>`/`&[u8]` in Wasm) or `async-io` (`futures-io`). CI builds the crate for wasm without those features; enabling `blocking-io` on wasm is not CI-covered but has no OS dependency. |
| gix-traverse | `--features sha1` | Commit ancestry walks (`commit::Simple`, `Topo`) and tree walks (breadth/depth first) over any `gix_object::Find` implementation — the building block for want/have negotiation, connectivity checks after push, and pack generation. Synchronous `Find` only. |
| gix-revision, gix-commitgraph, gix-refspec, gix-actor, gix-date, gix-validate, gix-quote, gix-url, gix-glob, gix-pathspec, gix-attributes, gix-config-value, gix-hashtable, gix-index, gix-mailmap, gix-path, gix-prompt, gix-command, gix-bitmap, gix-chunk | as listed in CI | Ref-name validation (`gix-validate`), refspec matching (`gix-refspec`), signature/date parsing, commit-graph reading (if we ever store one in R2), hash tables. `gix-index`/`gix-command`/`gix-prompt` build but are not useful server-side. |
| gix-features | `--features progress`, `parallel`, `io-pipe`, `crc32` each | CRC32 for `.idx`, progress traits. `default = []`. |

Compiles transitively but with caveats:
- gix-diff 0.67.1: has a `wasm` feature (`wasm = ["dep:getrandom"]` with `getrandom 0.4` feature `wasm_js`, which additionally needs `RUSTFLAGS='--cfg getrandom_backend="wasm_js"'` per https://docs.rs/getrandom/latest/getrandom/). Its default `blob` feature pulls `gix-filter`, `gix-worktree`, `gix-command`, `gix-tempfile`, `gix-fs` (filesystem); tree diff (`gix_diff::tree`) with `default-features = false, features = ["sha1", "wasm"]` is what `gix-pack --all-features` exercises on wasm CI. Blob diffing through gix-diff is filesystem-shaped; use `imara-diff` 0.2.0 directly for blob diffs.
- gix-merge 0.20.1: **not** in the wasm CI list, no `wasm` feature, and `blob::Platform` hard-depends on `gix_filter::Pipeline` and `gix_worktree::Stack` (attributes from a worktree). But the text driver itself — `gix-merge/src/blob/builtin_driver/text/{mod.rs, function.rs, utils.rs}` = 142 + 299 + 485 = 926 lines — imports only `imara_diff` and `bstr`. Vendor those three files (MIT/Apache-2.0) to get git-compatible `<<<<<<< ======= >>>>>>>` output with `ConflictStyle::{Merge, Diff3, ZealousDiff3}` and a conflict count. Tree-level merge (`gix_merge::tree`) also assumes the Platform; write our own tree merge over `gix-object` trees (the TS `server-side-merge` proof already has that logic).

Does not build / does not exist for our purpose:
- gix-odb, gix-ref, gix, gix-transport, gix-protocol are not in the wasm CI list, and tracking issue #463 (https://github.com/GitoxideLabs/gitoxide/issues/463, closed "not planned") names `gix-tempfile`, `gix-sec`, `gix-lock`, `gix-config`, `gix-ref`, `gix-protocol`, `gix-transport` and `gix` as blockers; discussion #1150 (https://github.com/GitoxideLabs/gitoxide/discussions/1150) has the maintainer saying use without a filesystem is "not possible yet", maybe "after `gix` gets stabilized in a year or two".
- gix-protocol is **client-side** ("An abstraction over fetching a pack from the server" — https://docs.rs/gix-protocol/latest/gix_protocol/; modules `handshake`, `fetch`, `ls_refs`, `command`). gitoxide has no `upload-pack`/`receive-pack` server implementation. The v2 command parsing (`ls-refs`, `fetch` with `want`/`have`/`done`/`deepen`/`filter`), ACK/NAK negotiation, `report-status-v2`, and the sideband demux/mux must be written by us on top of gix-packetline. This is the same amount of protocol code the TS design had to write; the codec underneath is the part that becomes free.

The async problem, restated for gitoxide: `gix_object::Find` and everything in `gix-traverse`/`gix-pack::find` are synchronous. workers-rs I/O (R2 get, DO storage, fetch) is async. There is no way to block on a promise inside Wasm on workerd for a Rust Worker today (no JSPI for JS/Wasm Workers; Asyncify is an Emscripten-side transform not offered by `wasm-bindgen`/`worker-build`). So the design is forced into the same shape the `wasm-git-core` proof chose: **async Rust host code prefetches bytes from R2/DO into memory (an in-memory `Find` impl over a `HashMap<ObjectId, (Kind, Vec<u8>)>` or over a pack slice), then calls synchronous gitoxide code**. This is not a limitation of Rust-vs-TS; it is the identical constraint, just with the boundary now between two Rust modules rather than between TS and Wasm. It does mean traversals that discover what to load next (ancestry walks for negotiation, connectivity checks) must be written as async loops that call `gix-object` parsing per step, or as prefetch-then-walk over a pack-index range read.

## 4. Recommended architecture

**Recommendation: one Rust Worker (workers-rs) + gitoxide plumbing, no TypeScript in the request path.** The Worker and the repo Durable Object are both Rust; all R2/DO/`fetch` I/O is async Rust through `worker`; all git logic (pkt-line, pack parsing, delta apply, hashing, tree/commit parsing, traversal, pack writing, blob merge) is synchronous gitoxide code fed from memory. The "host prefetches, core computes" boundary from the wasm-git-core proof survives, but it collapses to a function-call boundary inside one binary, which removes the linear-memory copy in/out, the `alloc`/`reset` leak the review flagged, the detached-`memory.buffer` bug class, and the second language.

Sketch of the request path:
- `GET /info/refs?service=git-upload-pack` and v2 `ls-refs`: Worker (stateless) forwards to the repo DO via `Stub::fetch_with_request`; DO reads refs from SQLite (`state.storage().sql().exec(...)`, synchronous, so no gate opening between read and write), builds the advertisement with `gix_packetline::encode::*` into a `Vec<u8>`, returns it.
- v2 `fetch`: DO parses the command with a small hand-written v2 parser over `gix_packetline::decode`; negotiation walks commits with `gix_traverse::commit::Simple` over an in-memory `Find` that is populated by async R2 range reads of the stored pack (using the SQLite `objects(sha -> pack_key, offset, len)` index from defect 3's fix); pack output is assembled with `gix_pack::data::output` (or, for the "precomputed pack" first milestone, streamed straight from R2 with `Response::from_stream` wrapped in sideband-64k chunks of <= 65515 bytes).
- `receive-pack`: Worker decompresses gzip bodies (use the platform `DecompressionStream` via `web_sys` rather than bundling a gzip decoder; `gix-zlib` is raw-zlib only), feeds the pack body through `gix_pack::data::input::BytesToEntriesIter` (over a buffered chunk reader) to get entries with their headers, applies deltas with `gix_pack::data::delta::apply` once bases are resident, hashes with `gix_object::compute_hash`, writes the raw pack to R2 with `create_multipart_upload` (>= 5 MiB parts) and loose/indexed objects as the storage document says, then asks the DO for the CAS ref update inside one synchronous SQLite transaction and returns `report-status-v2` on sideband 1.
- Background work (janitor with grace period, delta drain, repack) goes through the DO's single alarm with a `jobs` table and `set_alarm(min(next_run))`.

Trade-offs, concretely:

| | Rust everywhere (workers-rs + gitoxide) | Hybrid: TS Worker + Rust Wasm core (the wasm-git-core proof) | TS everywhere |
|---|---|---|---|
| pkt-line / pack / delta / hashing correctness | From gitoxide, battle-tested against git. | Same core, but every call copies bytes across the Wasm boundary and must be sized by hand. | Hand-written; 37/56 proofs got pkt-line wrong. |
| Async I/O ergonomics | Native `async fn` over `worker` bindings; `Result`-typed errors. | Best of both for I/O (TS), worst for glue: manual `alloc`/`free`, `memory.buffer` re-reads, two build systems. | Native. |
| DO semantics (gate, single alarm, name) | Identical to TS; workers-rs adds nothing and lacks `State::name()`. | Identical. | Identical. |
| Binary size / cold start | `.wasm` likely 1-3 MB + shim; well under 64 MiB; cold start impact unmeasured (the 1 s global-scope budget is the risk; instantiate lazily inside the handler). | Small TS + 300-600 KB Wasm. | Smallest. |
| Compile / iterate | Minutes per change (gitoxide is a big workspace; `wasm-opt` pass). `wrangler dev` works with `worker-build`. | Two toolchains. | Seconds. |
| Hiring / reviewability | Rust; the 56 proofs' authors and reviewers wrote TS. | Both. | TS. |
| Streaming | `Request::stream()` and `Response::from_stream` exist; `BytesToEntriesIter` wants `BufRead`, so an adapter that buffers the async stream into a `Cursor`/ring buffer is needed (pack ingestion is sequential, so a bounded "fill then parse" loop works). | TS streams, Wasm gets slices. | TS streams. |
| Panics | A Rust panic aborts the isolate unless `--panic-unwind`; treat every gitoxide `Result` as recoverable and never `unwrap` on client data. | Same for the core. | Exceptions. |
| Merge | Vendor gix-merge's 926-line text driver; own tree merge. | Same vendoring. | diff3 in TS (not byte-identical to git). |

Why not the hybrid: it keeps every problem of the Rust build (toolchain, size, vendoring) and adds the FFI hazards the review already caught (memory leak, detached views), while giving up typed async I/O. The hybrid is right only if the team wants the TS foundation to keep going and to graft Wasm in for delta apply and merge later; if that is the plan, the wasm-git-core proof (with its three blockers fixed) is the path.

First Worker milestone — "ls-refs and a clone from a precomputed pack" — concrete manifest (versions as of 2026-09-13):

```toml
[package]
name = "git-edge"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
worker = { version = "0.8.5" }                     # add features = ["http"] only for the stateless Worker; DOs ignore it (#582)
worker-macros = "0.8.5"
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["DecompressionStream"] }
futures-util = { version = "0.3", default-features = false }
bstr = { version = "1.12", default-features = false, features = ["std"] }

gix-hash        = { version = "0.26.2", default-features = false, features = ["sha1"] }
gix-object      = { version = "0.64.1", default-features = false, features = ["sha1"] }
gix-packetline  = { version = "0.22.2", default-features = false, features = ["blocking-io"] }
gix-pack        = { version = "0.74.2", default-features = false, features = ["sha1", "streaming-input", "wasm"] }
gix-traverse    = { version = "0.61.0", default-features = false, features = ["sha1"] }
gix-validate    = { version = "0.11.4", default-features = false }
gix-features    = { version = "0.49.1", default-features = false, features = ["crc32"] }
# later milestones:
# gix-diff  = { version = "0.67.1", default-features = false, features = ["sha1", "wasm"] }   # tree diff only; needs RUSTFLAGS --cfg getrandom_backend="wasm_js"
# imara-diff = "0.2.0"                                                                       # blob diff; vendored gix-merge text driver sits on top
# gix-refspec = { version = "0.45.1", default-features = false, features = ["sha1"] }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"      # or keep unwind and build with `worker-build --release -- --panic-unwind`
strip = true
```

`.cargo/config.toml`: `[build] target = "wasm32-unknown-unknown"`; add `rustflags = ['--cfg', 'getrandom_backend="wasm_js"']` when gix-diff's `wasm` feature enters. `wrangler.jsonc`: `main = "build/worker/shim.mjs"`, `build.command = "cargo install -q worker-build && worker-build --release"`, `durable_objects.bindings = [{ name: "REPO", class_name: "RepoDO" }]`, `migrations = [{ tag: "v1", new_sqlite_classes: ["RepoDO"] }]`, `r2_buckets = [{ binding: "BUCKET", bucket_name: "git-edge" }]`, `limits.cpu_ms = 300000`.

What that milestone exercises: `gix_packetline::encode` for the v2 capability advertisement and `ls-refs` reply (flush/delim/response-end, 65515-byte data cap), DO SQLite for refs, R2 range reads for the pack with `Response::from_stream` and sideband-64k framing, `gix_pack::index::File` (from bytes) or the SQLite object index to answer `want` against a precomputed pack, and `gix_hash` for id parsing. It deliberately does not yet ingest a push.

Where gitoxide's `generate` feature fits later: `gix_pack::data::output::{count, entry}` can build a pack from an object set given a `Find` — after the objects are prefetched — and that is how `fetch` with `have`s gets a thin or full pack without a filesystem.

## 5. Risks and effort

Which of the nine cross-cutting defects Rust helps with:

| # | Defect | Effect of Rust + gitoxide |
|---|---|---|
| 1 | pkt-line byte errors (37/56) | **Helps a lot.** `gix-packetline` owns the length prefix, `MAX_DATA_LEN`, flush/delim/response-end and sideband band bytes; the "one shared tested module" fix is largely bought, not built. Still must be tested against real `git`. |
| 2 | Janitor deletes live objects | **No effect.** Grace period + ref-version check at delete time is a design rule, same in any language. |
| 3 | Deltas in pushed packs (27/56) | **Helps a lot.** `BytesToEntriesIter` + `delta::apply` + `data::entry::Header` handle ofs/ref deltas, varints, and chain bases; `gix-pack::index` writes the `.idx`. The "which pack/offset holds each object" index in SQLite is still ours. |
| 4 | Inconsistent object storage | **Helps modestly.** A single `ObjectStore` type implementing `gix_object::Find`/`Write` makes "one shared reader" a compiler-enforced fact, and `Kind`/`ObjectId` types stop string-vs-bytes drift. The storage document is still needed. |
| 5 | DO gate opens on network waits | **No effect.** `await` on R2 from Rust yields exactly as in JS. The fix (all R2 first, then a synchronous `SqlStorage::exec` CAS) is available and slightly easier because `sql().exec` is non-async in workers-rs. |
| 6 | Single alarm | **No effect.** Same `set_alarm` semantics; needs the jobs table. |
| 7 | gzip/chunked bodies | **No effect**, slightly more work: gzip inflate is not in gitoxide (`gix-zlib` is raw zlib); use `DecompressionStream` through `web_sys` or add `flate2` with the `zlib-rs` backend. Multipart upload is supported. |
| 8 | Subrequest ceiling | **No effect** (and the number is now 10,000 on Paid / 50 on Free; range reads over packs still needed). |
| 9 | DO does not know its name | **Slightly worse ergonomics.** No `State::name()`; use the `js_sys::Reflect` workaround from #760 or the same "store name on first request" rule. |

New risks Rust adds:

1. **No server-side protocol in gitoxide.** `upload-pack`/`receive-pack` command parsing, negotiation (ACK/NAK, `done`, `deepen`, `filter`), `report-status-v2`, `push-options`, shallow handling — all ours. Same as TS, but do not budget as if gitoxide gave it.
2. **Sync `Find` vs async I/O.** Every traversal must be prefetch-driven; a naive port of a recursive walk will not compile (good) or will prefetch the world (bad). Design the SQLite object index and pack-range reads first; this is the same shape as the TS "1,000 calls" fix, so it is not extra work, but it is a hard constraint that shapes every module.
3. **Cold start and size.** Unmeasured. `wasm-bindgen` glue + gitoxide subset likely 1-3 MB; 64 MiB limit is irrelevant, the 1 s global-scope budget and per-isolate instantiate cost are what matter. Mitigation: nothing in global scope, lazy init, `wasm-opt -Oz`, measure with a cold-start probe before committing.
4. **Compile times and iteration speed.** Minutes, not seconds; `wrangler dev` cycles are slower; debugging is `console_log!` plus `--panic-unwind` for readable panics. `coredump` example exists in workers-rs for post-mortems.
5. **workers-rs API gaps** (all workaroundable via `js_sys`): no typed DO RPC (HTTP-shaped stub calls), no `State::name()`, no sync KV (not needed), DO `fetch` ignores the `http` feature, `fetch(&self)` forces interior mutability for per-DO caches.
6. **Panic = isolate abort** unless `--panic-unwind`. Every byte from a client must go through `Result`; forbid `unwrap`/`indexing` on client data by lint.
7. **Vendored merge driver** (926 lines) drifts from upstream; pin the gix-merge version you copied from and diff on upgrade.
8. **gix-pack `wasm` feature is a build-only promise.** It compiles out `bundle::write` and `gix-tempfile`; nothing in gitoxide is *tested* on wasm in CI (it is `cargo build`, not `cargo test`). Run our own `wasm32` unit tests under `wasm-bindgen-test`/workerd (`wrangler dev` or `workerd` binary) for delta application and pack streaming.
9. **Dependency churn.** gitoxide releases monthly with lockstep version bumps across ~40 crates; pin exact versions and upgrade deliberately.

Effort (one senior Rust engineer who knows git internals; foundation = the nine fixes + clone/fetch/push over smart-HTTP v2 against real `git`):

| Phase | Rust (workers-rs + gitoxide) | TS (as the 56 proofs assumed) |
|---|---|---|
| Toolchain, skeleton Worker + DO + R2, CI, cold-start measurement | 1 week | 2 days |
| Milestone 1: ls-refs + clone from precomputed pack (crate list above) | 1-2 weeks | 1-2 weeks (pkt-line written by hand) |
| Storage document + object store + SQLite index + range reads (defects 3, 4, 8) | 2 weeks | 3 weeks (delta resolver written by hand) |
| Push: gzip/multipart body, streaming pack ingest, deltas, connectivity, CAS in DO, report-status (defects 1, 3, 5, 7) | 3 weeks | 4 weeks |
| Alarm dispatcher, janitor with grace, DO name (defects 2, 6, 9) | 1 week | 1 week |
| Fetch with negotiation, pack generation with `gix-pack` output | 2 weeks | 3 weeks |
| Conformance against `git` 2.4x (and `git2` harness), soak | 2 weeks | 2 weeks |
| **Total to a foundation that survives the reviewers** | **~12-13 weeks (3 months)** | **~14-15 weeks (3.5 months)** |

The totals are close because the platform constraints dominate. Rust buys back the two most-hit defect classes (pkt-line, deltas) and a stronger type boundary around storage, and pays it out in toolchain, cold-start work, and the missing server-side protocol layer. If the team is stronger in TS than Rust, the TS path is faster; if a Rust engineer is available, the Rust path produces the more robust artifact for roughly the same calendar time. git2-rs is not on this table because it cannot be built.

Not verified, stated plainly: actual `.wasm` size and cold-start time for the crate set above (no build here); whether `gix-packetline` with `blocking-io` and `gix-pack` with `generate` produce any wasm32 compile errors in a real build (CI covers `--all-features` for gix-pack, which implies both, but I did not run it); the body of workerd's `jspiEnabledCallback` and whether any compat flag exposes JSPI to non-Python Workers; azul's exact binding usage.

## Verdict

**(a) git2-rs inside the Worker: does not land.** `libgit2-sys` has no wasm32 build path (unconditional POSIX/`GIT_THREADS`/libc assumptions; issues #511/#871 open since 2020/2022), the only libgit2-on-Wasm is an Emscripten C build with a virtual filesystem and Asyncify/JSPI that cannot be linked into a `wasm-bindgen` Worker, and even a hypothetical build would face synchronous `git_odb_backend`/`git_refdb_backend` callbacks with no supported way to block on a promise from Wasm on workerd today (JSPI is not exposed to JS/Wasm Workers; Asyncify is Emscripten-only). Keep git2 for an off-Worker conformance harness or a Containers sidecar. **(b) workers-rs + gitoxide: lands with caveats.** Every Workers primitive the design needs — DO SQLite (sync `exec`), alarms, WebSocket hibernation, R2 get/range/put/multipart, streaming request and response bodies — is present in `worker` 0.8.5 and used in production by Cloudflare's own azul; `gix-hash`, `gix-object`, `gix-pack` (with `wasm`), `gix-packetline`, `gix-traverse` and a dozen helpers are CI-built for `wasm32-unknown-unknown`, and the size limits that worried the proofs are gone (64 MiB uncompressed). The caveats are structural and firm: gitoxide has no server-side protocol (write `upload-pack`/`receive-pack` ourselves on top of `gix-packetline`), its object access is synchronous so all R2/DO I/O must be prefetch-then-compute, `gix-odb`/`gix-ref`/`gix-merge` do not build for wasm (vendor the 926-line text merge driver), cold-start cost is unmeasured, and Rust does nothing for defects 2, 5, 6, 7, 8 and 9 — those remain design rules to enforce in the foundation regardless of language.

## Post-script: corrections after building it (2026-09-13)

The spike in `spikes/rust-ls-refs/` built and ran. Three statements above were wrong and are corrected in `CONTRACTS.md` under "Corrections from the Rust spike": the `gix_packetline` encode path is `blocking_io::encode`; `gix_pack::data::delta::apply` is not public and deltas are resolved through `data::File::decode_entry` with `gix_zlib::Inflate`; `strip = true` must be `strip = "debuginfo"`. The size estimate of 1 to 3 MB was pessimistic: 605 KB after wasm-opt. Full results in `research/rust-spike.md`.
