# git-edge Rust Worker spike: `ls-refs` over smart-HTTP v2

A minimal workers-rs (`worker` 0.8.5) Cloudflare Worker with one SQLite-backed Durable Object
(`RepoDO`) and an R2 binding, linking gitoxide plumbing (gix-hash, gix-object, gix-packetline,
gix-pack, gix-traverse, gix-zlib) for `wasm32-unknown-unknown`. Findings are written up in
`../../research/rust-spike.md`.

## Layout

- `src/lib.rs`        Worker entry, `RepoDO`, v2 advertisement + `ls-refs`, `/selftest`
- `fixtures/test.pack` 6-object pack (2 OFS deltas) produced by `git pack-objects`, embedded via `include_bytes!`
- `Cargo.toml`        pinned crate versions/features that build for wasm32 (see the `strip` comment)
- `wrangler.jsonc`    DO + migration (`new_sqlite_classes`) + R2 binding; `build.command` runs `worker-build --release`
- `dev.sh`            starts `wrangler dev --port 8798`

## Prerequisites

    export PATH=/root/.cargo/bin:$PATH
    rustup target add wasm32-unknown-unknown
    cargo install worker-build        # downloads wasm-bindgen-cli 0.2.128 + wasm-opt 130 on first run
    npm i                             # wrangler 4.129

If TLS fails behind the proxy: `export SSL_CERT_FILE=/root/.ccr/ca-bundle.crt CARGO_HTTP_CAINFO=/root/.ccr/ca-bundle.crt NODE_EXTRA_CA_CERTS=/root/.ccr/ca-bundle.crt`.

## Build and run

    cargo build --release                  # wasm32 is the default target via .cargo/config.toml
    worker-build --release                 # -> build/index_bg.wasm + build/index.js + build/worker/shim.mjs
    ./dev.sh                               # or: npx wrangler dev --port 8798

## Exercise

    curl -s -X POST http://127.0.0.1:8798/owner/repo/_seed            # HEAD -> refs/heads/main, main, dev, tags/v1.0
    git ls-remote http://127.0.0.1:8798/owner/repo                     # v2 by default in git >= 2.26
    GIT_TRACE_PACKET=1 git -c protocol.version=2 ls-remote http://127.0.0.1:8798/owner/repo
    git -c protocol.version=0 ls-remote http://127.0.0.1:8798/owner/repo   # v0 fallback advertisement
    curl -s http://127.0.0.1:8798/selftest | python3 -m json.tool       # gix-pack/gix-object/gix-traverse + R2 at runtime

## Measure

    npx wrangler deploy --dry-run --outdir dist     # prints "Total Upload: ... / gzip: ..."
    ls -l target/wasm32-unknown-unknown/release/*.wasm build/index_bg.wasm

Nothing here is committed to git; the spike lives only in the scratchpad.
