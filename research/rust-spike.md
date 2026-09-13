# git-edge Rust Worker spike: results

Date: 2026-09-13. Source: `scratchpad/git-fdn/spikes/rust-ls-refs/` (README there explains how to run it). Nothing committed.

## What was built

One `cdylib` crate compiled to `wasm32-unknown-unknown` with workers-rs, exposing:

- `#[event(fetch)]` stateless Worker that routes `/:owner/:repo/*` to a Durable Object named `owner/repo` (`Stub::fetch_with_request`) and serves `/selftest` itself.
- `#[durable_object] RepoDO` using SQLite storage (`state.storage().sql().exec(...)`): tables `refs(name TEXT PRIMARY KEY, sha TEXT)` and `symrefs(name TEXT PRIMARY KEY, target TEXT)`, created in `fetch` via `CREATE TABLE IF NOT EXISTS`. Write outcomes are read with `SELECT changes() AS n` (never `rows_written()`).
- R2 binding `BUCKET` (bucket `git-edge`), exercised with a `put`+`get` round trip inside `/selftest`.
- git smart-HTTP v2 read handshake:
  - `GET /:owner/:repo/info/refs?service=git-upload-pack` with `Git-Protocol: version=2` -> `# service=git-upload-pack`, flush, `version 2`, `agent=git-edge-spike/0.1`, `ls-refs=unborn`, `fetch=shallow wait-for-done`, `server-option`, `object-format=sha1`, flush. Without the header it falls back to a v0 advertisement (`sha HEAD\0object-format=sha1 symref=HEAD:refs/heads/main agent=...`), so `-c protocol.version=0` also works.
  - `POST /:owner/:repo/git-upload-pack` parses the body with `gix_packetline::decode::streaming`, requires `command=ls-refs`, honours `symrefs` (emits `symref-target:`) and any number of `ref-prefix` args (`peel`/`unborn` accepted, no-ops: the seeded tag is lightweight and there are no unborn refs), answers with pkt-lines + flush. Unknown commands get an `ERR` pkt-line; malformed pkt-lines get HTTP 400.
  - `POST /:owner/:repo/_seed` upserts `HEAD -> refs/heads/main`, `refs/heads/main`, `refs/heads/dev`, `refs/tags/v1.0`.
- `/selftest`: an embedded 1031-byte pack (`git pack-objects --delta-base-offset` over a 2-commit repo, 6 objects, 2 OFS deltas) is (1) streamed with `gix_pack::data::input::BytesToEntriesIter` in `Mode::Verify` + `EntryDataMode::KeepAndCrc32` (trailer checksum verified), (2) opened in memory as `gix_pack::data::File::<&[u8]>::from_data` and every entry resolved with `decode_entry` (this is what runs `delta::apply` for the OFS deltas, using `gix_zlib::Inflate`), (3) hashed with `gix_object::compute_hash`, (4) the HEAD commit parsed with `gix_object::CommitRef::from_bytes`, and (5) ancestry walked with `gix_traverse::commit::Simple::new([head], &MemFind)` over a `HashMap` implementing `gix_object::Find`. All pkt-line output is built with `gix_packetline::blocking_io::encode::*` and the `blocking_io::Writer` is linked too.

## Crate versions and features that built for wasm32-unknown-unknown

All memo pins built unchanged; nothing had to be bumped.

| crate | version | features (default-features = false unless noted) |
|---|---|---|
| worker / worker-macros | 0.8.5 | default |
| wasm-bindgen | 0.2.128 (crate and CLI) | |
| gix-hash | 0.26.2 | `sha1` |
| gix-object | 0.64.1 | `sha1` |
| gix-packetline | 0.22.2 | `blocking-io` |
| gix-pack | 0.74.2 | `sha1`, `streaming-input`, `wasm` |
| gix-traverse | 0.61.0 | `sha1` |
| gix-zlib | 0.1.0 | none (needed directly for `gix_zlib::Inflate`, the argument type of `File::decode_entry`) |
| bstr 1.12 (`std`), serde 1, serde_json 1 | | |
| transitive of note | gix-diff 0.67.1, gix-features 0.49.1, gix-tempfile 24.0.0 (pulled by gix-diff; compiles, unused), memmap2 (compiles, unused) | |

Toolchain: rustc 1.94.1, cargo 1.94.1, worker-build 0.8.5 (downloads wasm-bindgen-cli 0.2.128, wasm-opt 130, esbuild 0.28.1 into `~/.cache/worker-build`), wrangler 4.129.1, node 22.22, git 2.43.0.

`[profile.release]`: `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, **`strip = "debuginfo"`** (see error 2). `.cargo/config.toml`: `[build] target = "wasm32-unknown-unknown"`. No `RUSTFLAGS` needed (gix-diff's `wasm`/getrandom feature is not enabled).

## Build errors hit and fixes

1. **`gix_packetline::encode::{data_to_write, flush_to_write, ...}` and `gix_packetline::Writer` do not exist** (E0425/E0433). In 0.22.2 the `blocking-io` API lives under `gix_packetline::blocking_io::{encode, Writer, StreamingPeekableIter}` (`gix_packetline::encode` only holds the `Error` type). The memo's paths are stale. Fix: `use gix_packetline::blocking_io::encode;`.
2. **`worker-build --release` failed in wasm-bindgen: `failed to generate catch wrappers: externref table required for catch wrappers`**, while running `wasm-bindgen` by hand with default flags succeeded. worker-build adds `--experimental-reset-state-function --force-enable-abort-handler`; the abort handler needs an externref table, which wasm-bindgen only creates when it sees the `reference-types` target feature in the module's `target_features` custom section. The memo's `strip = true` makes rustc pass `--strip-all` to wasm-ld, which deletes that section. Fix: `strip = "debuginfo"` (or leave strip off; wasm-opt/wasm-bindgen strip names anyway). Alternative that also worked: an extra `--reference-types` flag to wasm-bindgen, but worker-build offers no way to pass it without `WASM_BINDGEN_BIN` pointing at a wrapper script.
3. **`gix_pack::data::delta::{apply, decode_header_size}` are `pub(crate)`** in 0.74.2, not public as the memo states. The public way to apply deltas is `gix_pack::data::File<T: Deref<Target=[u8]>>::from_data(bytes, PathBuf, Kind)` + `entry(offset)` + `decode_entry(entry, &mut out, &mut gix_zlib::Inflate, &resolve_ref_delta, &mut cache::Never)`; that resolves OFS/REF delta chains in memory. `BytesToEntriesIter` yields entries with *compressed* bytes only (`Entry.compressed`), so `gix-zlib` becomes a direct dependency for anything that inflates by hand.
4. Not a build error, but a runtime one: my first `upload_pack_v2` propagated pkt-line decode errors as `worker::Error`, which workerd reports as an uncaught exception / HTTP 500 (`Uncaught Error: pkt-line decode: ...` in the wrangler log). Client-controlled parse failures now return `Response::error(..., 400)`.

No other errors: cargo fetch/build behind the proxy worked with `SSL_CERT_FILE`/`CARGO_HTTP_CAINFO` set; `rustup target add wasm32-unknown-unknown` and `cargo install worker-build` (2m39s) were needed.

## git ls-remote transcript (real git 2.43 against `wrangler dev`)

```
$ curl -s -X POST http://127.0.0.1:8798/owner/repo/_seed
{"changes":4,"ok":true,"refs":3}

$ git ls-remote http://127.0.0.1:8798/owner/repo
b17a78de11335d8d2498c38009d1d14fb46a5b52	HEAD
3d33958e6837343041aa235df94b402655941e39	refs/heads/dev
b17a78de11335d8d2498c38009d1d14fb46a5b52	refs/heads/main
3d33958e6837343041aa235df94b402655941e39	refs/tags/v1.0
exit=0

$ GIT_TRACE_PACKET=1 git -c protocol.version=2 ls-remote http://127.0.0.1:8798/owner/repo
packet:          git< # service=git-upload-pack
packet:          git< 0000
packet:          git< version 2
packet:          git< agent=git-edge-spike/0.1
packet:          git< ls-refs=unborn
packet:          git< fetch=shallow wait-for-done
packet:          git< server-option
packet:          git< object-format=sha1
packet:          git< 0000
packet:    ls-remote> command=ls-refs
packet:    ls-remote> agent=git/2.43.0
packet:    ls-remote> object-format=sha1
packet:    ls-remote> 0001
packet:    ls-remote> peel
packet:    ls-remote> symrefs
packet:    ls-remote> unborn
packet:    ls-remote> 0000
packet:          git> 0002
packet:    ls-remote< b17a78de11335d8d2498c38009d1d14fb46a5b52 HEAD symref-target:refs/heads/main
packet:    ls-remote< 3d33958e6837343041aa235df94b402655941e39 refs/heads/dev
packet:    ls-remote< b17a78de11335d8d2498c38009d1d14fb46a5b52 refs/heads/main
packet:    ls-remote< 3d33958e6837343041aa235df94b402655941e39 refs/tags/v1.0
packet:    ls-remote< 0000
packet:    ls-remote< 0002
b17a78de11335d8d2498c38009d1d14fb46a5b52	HEAD
3d33958e6837343041aa235df94b402655941e39	refs/heads/dev
b17a78de11335d8d2498c38009d1d14fb46a5b52	refs/heads/main
3d33958e6837343041aa235df94b402655941e39	refs/tags/v1.0
exit=0

$ git -c protocol.version=0 ls-remote http://127.0.0.1:8798/owner/repo     # same four lines, exit=0
```

Direct `ls-refs` with `ref-prefix refs/tags/` returns only `refs/tags/v1.0` + flush; with `symrefs`, `ref-prefix HEAD`, `ref-prefix refs/heads/` returns HEAD (with `symref-target:`), dev, main + flush. The seeded refs survived a wrangler dev restart (local SQLite persisted in `.wrangler/state`).

## /selftest output

```json
{"pack_bytes":1031,"pack_version":"V2",
 "entries":[
  {"offset":12, "header":"Commit","decompressed_size":480,"crc32":3697827038,"kind":"commit","num_deltas":0,"id":"b17a78de11335d8d2498c38009d1d14fb46a5b52"},
  {"offset":372,"header":"OfsDelta { base_distance: 360 }","decompressed_size":154,"crc32":2889974922,"kind":"commit","num_deltas":1,"id":"3d33958e6837343041aa235df94b402655941e39"},
  {"offset":536,"header":"Tree","decompressed_size":33,"crc32":585512460,"kind":"tree","num_deltas":0,"id":"fc11c7d3ee79c21690ba73c513d6bb860eaa7158"},
  {"offset":580,"header":"Blob","decompressed_size":700,"crc32":3288885102,"kind":"blob","num_deltas":0,"id":"3fc014b66234ecf6f0bbc7776a962012b8be362c"},
  {"offset":939,"header":"Tree","decompressed_size":33,"crc32":2765654521,"kind":"tree","num_deltas":0,"id":"f80bb2f61175b36c7c616965a45f71e21ebc10d0"},
  {"offset":983,"header":"OfsDelta { base_distance: 403 }","decompressed_size":16,"crc32":4259675454,"kind":"blob","num_deltas":1,"id":"aa5e3f802c6a6d3eb7eac845d2293dec38ccfff1"}],
 "trailer":"80def6189ef92768fa99e2b2134e0a00d85a7d69","delta_entries":2,
 "commit_walk":["b17a78de11335d8d2498c38009d1d14fb46a5b52","3d33958e6837343041aa235df94b402655941e39"],
 "head_tree":"fc11c7d3ee79c21690ba73c513d6bb860eaa7158",
 "r2":"put+get ok: hello from git-edge","errors":[]}
```

Every id, offset, size and delta relation matches `git verify-pack -v` on the same pack (pack checksum `80def618...`; deltas at 372 -> base 12 and 983 -> base 580), so gix-pack's inflate + delta apply and gix-object's hashing run correctly on workerd, not just compile.

## Sizes and timings

| artifact | bytes |
|---|---|
| rustc output `target/wasm32-unknown-unknown/release/git_edge_spike.wasm` (opt-level z, lto, strip=debuginfo) | 1,347,238 |
| same with `strip = true` (does not work with worker-build, see error 2) | 1,100,524 |
| after wasm-bindgen, before wasm-opt (`index_bg.wasm`) | 674,613 |
| after wasm-opt 130 (`-Oz`, worker-build default) `build/index_bg.wasm` | 605,464 |
| `npx wrangler deploy --dry-run --outdir dist`: Total Upload | 633.96 KiB (649,178 B: wasm + 43,691 B `shim.js`); gzip 262.41 KiB |
| gzip -9 of the wasm alone | 255,232 |

The 1031-byte fixture pack is included in those numbers.

Cold start on `wrangler dev` (workerd, local, `curl -w %{time_total}`; wrangler's own per-request timing in parentheses). "Cold" = first request after starting wrangler dev, with no request sent before it.

| request | run A (first request = /selftest) | run B (first request = info/refs v2) |
|---|---|---|
| 1st request (isolate + wasm instantiate + first DO/SQLite open) | 39.9 ms (39 ms) | 29.7 ms (28 ms) |
| 2nd request, same route | 9.7 ms (9 ms) | 6.9 ms (6 ms) |
| 3rd request, same route | 9.0 ms (8 ms) | 6.2 ms (6 ms) |
| first `/selftest` on a warm isolate | - | 23.3 ms (23 ms) |
| warm `POST git-upload-pack` ls-refs | 8.3 ms | 6 ms |
| warm `POST _seed` (4 upserts + 4x `SELECT changes()`) | 7.3 ms | - |

So instantiating the 605 KB module costs roughly 20-30 ms extra on the first request on this machine; well inside the 1 s global-scope budget. Production (edge) numbers were not measured: no deploy was done.

## What did not work or was not done

- `strip = true` from the memo's manifest breaks `worker-build` (error 2). Use `strip = "debuginfo"`.
- The memo's `gix_packetline::encode::*_to_write` paths and public `gix_pack::data::delta::apply` do not exist in the pinned versions (errors 1 and 3). Delta application is only reachable through `data::File::decode_entry` (fine: it takes an in-memory slice) or by vendoring the ~100-line `apply` function.
- `worker-build` prints `Finished ... in 0.12s` and *reuses* the cargo artifact; it does not add `--reference-types` itself, and there is no flag to change the wasm-bindgen argument list short of `WASM_BINDGEN_BIN`.
- Not implemented: v2 `fetch` command, sideband, pack serving from R2, receive-pack, peeling of annotated tags (`peel` is accepted and ignored; the fixture tag is lightweight), `unborn` HEAD, `server-option`, `Content-Encoding: gzip` request bodies.
- Not measured: deployed (edge) cold start, memory usage, and CPU time under the 128 MB DO limit. `wrangler deploy` was only run with `--dry-run`.
- `State::name()` is still absent in workers-rs 0.8.5 (the DO learns its repo only from the request path), as the memo noted.
- Timing noise: the "cold" numbers include wrangler dev's local proxy; treat them as an upper bound for module instantiation cost, not as edge latency.
