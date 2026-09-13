//! git-edge spike: workers-rs Worker + `RepoDO` (SQLite) + R2 binding, with gitoxide plumbing
//! (gix-hash, gix-object, gix-packetline, gix-pack, gix-traverse) linked and exercised at runtime.
//!
//! Routes:
//!   GET  /:owner/:repo/info/refs?service=git-upload-pack   v2 capability advertisement (v0 fallback)
//!   POST /:owner/:repo/git-upload-pack                      v2 `command=ls-refs`
//!   POST /:owner/:repo/_seed                                seed HEAD/main/tag into the DO's SQLite
//!   GET  /selftest                                          parse an embedded pack, resolve deltas, hash, traverse

use std::collections::HashMap;
use std::io::Write;

use bstr::ByteSlice;
use gix_hash::ObjectId;
use gix_packetline::{decode, encode, PacketLineRef};
use serde::Serialize;
use worker::*;

const AGENT: &str = "git-edge-spike/0.1";
const TEST_PACK: &[u8] = include_bytes!("../fixtures/test.pack");

// ---------------------------------------------------------------------------------------------
// Stateless Worker
// ---------------------------------------------------------------------------------------------

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.path();
    if path == "/selftest" {
        return selftest(&env).await;
    }
    // /:owner/:repo/<rest>
    let segs: Vec<&str> = path.trim_start_matches('/').splitn(3, '/').collect();
    if segs.len() == 3 {
        let ns = env.durable_object("REPO")?;
        let stub = ns.id_from_name(&format!("{}/{}", segs[0], segs[1]))?.get_stub()?;
        return stub.fetch_with_request(req).await;
    }
    Response::error("not found", 404)
}

// ---------------------------------------------------------------------------------------------
// Durable Object: one per repo, refs in SQLite
// ---------------------------------------------------------------------------------------------

#[durable_object]
pub struct RepoDO {
    state: State,
    #[allow(dead_code)]
    env: Env,
}

#[derive(serde::Deserialize)]
struct RefRow {
    name: String,
    sha: String,
}
#[derive(serde::Deserialize)]
struct SymRow {
    name: String,
    target: String,
}
#[derive(serde::Deserialize)]
struct Changes {
    n: i64,
}

impl RepoDO {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn ensure_schema(&self) -> Result<()> {
        let sql = self.sql();
        sql.exec("CREATE TABLE IF NOT EXISTS refs(name TEXT PRIMARY KEY, sha TEXT NOT NULL)", None)?;
        sql.exec("CREATE TABLE IF NOT EXISTS symrefs(name TEXT PRIMARY KEY, target TEXT NOT NULL)", None)?;
        Ok(())
    }

    /// Write outcome via `SELECT changes()` (never `rows_written()`), per the memo.
    fn changes(&self) -> Result<i64> {
        Ok(self.sql().exec("SELECT changes() AS n", None)?.one::<Changes>()?.n)
    }

    fn all_refs(&self) -> Result<Vec<RefRow>> {
        self.sql().exec("SELECT name, sha FROM refs ORDER BY name", None)?.to_array::<RefRow>()
    }

    fn symrefs(&self) -> Result<HashMap<String, String>> {
        let rows = self.sql().exec("SELECT name, target FROM symrefs", None)?.to_array::<SymRow>()?;
        Ok(rows.into_iter().map(|r| (r.name, r.target)).collect())
    }

    fn resolve(&self, name: &str, refs: &[RefRow], syms: &HashMap<String, String>) -> Option<String> {
        let mut cur = name.to_string();
        for _ in 0..8 {
            if let Some(t) = syms.get(&cur) {
                cur = t.clone();
                continue;
            }
            return refs.iter().find(|r| r.name == cur).map(|r| r.sha.clone());
        }
        None
    }

    fn seed(&self) -> Result<Response> {
        // Values are the commit ids from fixtures/test.pack so ls-remote and selftest agree.
        let main = "b17a78de11335d8d2498c38009d1d14fb46a5b52";
        let v1 = "3d33958e6837343041aa235df94b402655941e39";
        let sql = self.sql();
        let mut written = 0i64;
        for (name, sha) in [("refs/heads/main", main), ("refs/heads/dev", v1), ("refs/tags/v1.0", v1)] {
            sql.exec(
                "INSERT INTO refs(name, sha) VALUES (?, ?) ON CONFLICT(name) DO UPDATE SET sha = excluded.sha",
                vec![SqlStorageValue::from(name), SqlStorageValue::from(sha)],
            )?;
            written += self.changes()?;
        }
        sql.exec(
            "INSERT INTO symrefs(name, target) VALUES ('HEAD', 'refs/heads/main') ON CONFLICT(name) DO UPDATE SET target = excluded.target",
            None,
        )?;
        written += self.changes()?;
        let refs = self.all_refs()?.len();
        Response::from_json(&serde_json::json!({ "ok": true, "changes": written, "refs": refs }))
    }

    fn advertise(&self, v2: bool) -> Result<Response> {
        let mut out = Vec::<u8>::new();
        encode::data_to_write(b"# service=git-upload-pack\n", &mut out)?;
        encode::flush_to_write(&mut out)?;
        if v2 {
            encode::data_to_write(b"version 2\n", &mut out)?;
            encode::data_to_write(format!("agent={AGENT}\n").as_bytes(), &mut out)?;
            encode::data_to_write(b"ls-refs=unborn\n", &mut out)?;
            encode::data_to_write(b"fetch=shallow wait-for-done\n", &mut out)?;
            encode::data_to_write(b"server-option\n", &mut out)?;
            encode::data_to_write(b"object-format=sha1\n", &mut out)?;
            encode::flush_to_write(&mut out)?;
        } else {
            // v0 fallback so `git -c protocol.version=0` also works.
            let refs = self.all_refs()?;
            let syms = self.symrefs()?;
            let mut first = true;
            let mut lines: Vec<(String, String)> = Vec::new();
            if let Some(sha) = self.resolve("HEAD", &refs, &syms) {
                lines.push(("HEAD".into(), sha));
            }
            for r in &refs {
                lines.push((r.name.clone(), r.sha.clone()));
            }
            if lines.is_empty() {
                let caps = format!("{} capabilities^{{}}\0symref=HEAD:refs/heads/main agent={AGENT}\n", "0".repeat(40));
                encode::data_to_write(caps.as_bytes(), &mut out)?;
            }
            for (name, sha) in lines {
                let line = if first {
                    first = false;
                    let sym = syms.get("HEAD").map(|t| format!(" symref=HEAD:{t}")).unwrap_or_default();
                    format!("{sha} {name}\0object-format=sha1{sym} agent={AGENT}\n")
                } else {
                    format!("{sha} {name}\n")
                };
                encode::data_to_write(line.as_bytes(), &mut out)?;
            }
            encode::flush_to_write(&mut out)?;
        }
        git_response(out, "application/x-git-upload-pack-advertisement")
    }

    fn upload_pack_v2(&self, body: &[u8]) -> Result<Response> {
        // Decode the request: command=..., capability lines, delim, args, flush.
        let mut lines: Vec<PacketLineRef<'_>> = Vec::new();
        let mut rest = body;
        while !rest.is_empty() {
            match decode::streaming(rest).map_err(|e| Error::RustError(format!("pkt-line decode: {e}")))? {
                decode::Stream::Complete { line, bytes_consumed } => {
                    lines.push(line);
                    rest = &rest[bytes_consumed..];
                }
                decode::Stream::Incomplete { .. } => {
                    return Response::error("truncated pkt-line stream", 400);
                }
            }
        }
        let mut command = None;
        let mut args: Vec<&[u8]> = Vec::new();
        let mut in_args = false;
        for l in &lines {
            match l {
                PacketLineRef::Data(d) => {
                    let d = d.strip_suffix(b"\n").unwrap_or(d);
                    if in_args {
                        args.push(d);
                    } else if let Some(c) = d.strip_prefix(b"command=") {
                        command = Some(c.to_vec());
                    } // else: capability line (agent=..., object-format=sha1) - ignored
                }
                PacketLineRef::Delimiter => in_args = true,
                PacketLineRef::Flush | PacketLineRef::ResponseEnd => break,
            }
        }
        let mut out = Vec::<u8>::new();
        match command.as_deref() {
            Some(b"ls-refs") => {
                let want_symrefs = args.iter().any(|a| *a == b"symrefs");
                let prefixes: Vec<&[u8]> = args.iter().filter_map(|a| a.strip_prefix(b"ref-prefix ")).collect();
                let refs = self.all_refs()?;
                let syms = self.symrefs()?;
                let matches = |name: &str| prefixes.is_empty() || prefixes.iter().any(|p| name.as_bytes().starts_with(p));
                if matches("HEAD") {
                    if let Some(sha) = self.resolve("HEAD", &refs, &syms) {
                        let mut line = format!("{sha} HEAD");
                        if want_symrefs {
                            if let Some(t) = syms.get("HEAD") {
                                line.push_str(&format!(" symref-target:{t}"));
                            }
                        }
                        line.push('\n');
                        encode::data_to_write(line.as_bytes(), &mut out)?;
                    }
                }
                for r in refs.iter().filter(|r| matches(&r.name)) {
                    encode::data_to_write(format!("{} {}\n", r.sha, r.name).as_bytes(), &mut out)?;
                }
                encode::flush_to_write(&mut out)?;
            }
            Some(other) => {
                encode::error_to_write(format!("unsupported command {}", other.as_bstr()).as_bytes(), &mut out)?;
                encode::flush_to_write(&mut out)?;
            }
            None => return Response::error("missing command", 400),
        }
        git_response(out, "application/x-git-upload-pack-result")
    }
}

impl DurableObject for RepoDO {
    fn new(state: State, env: Env) -> Self {
        Self { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        self.ensure_schema()?;
        let path = req.path();
        let url = req.url()?;
        let method = req.method();
        let v2 = req
            .headers()
            .get("Git-Protocol")?
            .map(|v| v.split(':').any(|p| p.trim() == "version=2"))
            .unwrap_or(false);

        if path.ends_with("/_seed") && method == Method::Post {
            return self.seed();
        }
        if path.ends_with("/info/refs") && method == Method::Get {
            let service = url.query_pairs().find(|(k, _)| k == "service").map(|(_, v)| v.into_owned());
            if service.as_deref() != Some("git-upload-pack") {
                return Response::error("only git-upload-pack is supported", 403);
            }
            return self.advertise(v2);
        }
        if path.ends_with("/git-upload-pack") && method == Method::Post {
            if !v2 {
                return Response::error("protocol v2 required (send Git-Protocol: version=2)", 400);
            }
            let body = req.bytes().await?;
            return self.upload_pack_v2(&body);
        }
        Response::error("not found", 404)
    }
}

fn git_response(body: Vec<u8>, content_type: &str) -> Result<Response> {
    let h = Headers::new();
    h.set("Content-Type", content_type)?;
    h.set("Cache-Control", "no-cache")?;
    Ok(Response::from_bytes(body)?.with_headers(h))
}

// ---------------------------------------------------------------------------------------------
// /selftest: gix-pack + gix-object + gix-traverse at runtime on workerd
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct EntryReport {
    offset: u64,
    header: String,
    decompressed_size: u64,
    crc32: Option<u32>,
    kind: String,
    num_deltas: u32,
    id: String,
}

#[derive(Serialize)]
struct SelfTest {
    pack_bytes: usize,
    pack_version: String,
    entries: Vec<EntryReport>,
    trailer: Option<String>,
    delta_entries: usize,
    commit_walk: Vec<String>,
    head_tree: Option<String>,
    r2: String,
    errors: Vec<String>,
}

struct MemFind(HashMap<ObjectId, (gix_object::Kind, Vec<u8>)>);
impl gix_object::Find for MemFind {
    fn try_find<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> std::result::Result<Option<gix_object::Data<'a>>, gix_object::find::Error> {
        Ok(self.0.get(id).map(|(kind, data)| {
            buffer.clear();
            buffer.extend_from_slice(data);
            gix_object::Data { kind: *kind, object_hash: gix_hash::Kind::Sha1, data: buffer.as_slice() }
        }))
    }
}

fn run_pack_selftest(report: &mut SelfTest) -> std::result::Result<(), String> {
    use gix_pack::data::input::{BytesToEntriesIter, EntryDataMode, Mode};

    // 1. Stream entries from bytes (the `streaming-input` feature), verifying the trailer checksum.
    let iter = BytesToEntriesIter::new_from_header(
        std::io::Cursor::new(TEST_PACK),
        Mode::Verify,
        EntryDataMode::KeepAndCrc32,
        gix_hash::Kind::Sha1,
    )
    .map_err(|e| format!("new_from_header: {e}"))?;
    report.pack_version = format!("{:?}", iter.version());
    let mut streamed = Vec::new();
    for e in iter {
        let e = e.map_err(|e| format!("entry: {e}"))?;
        if let Some(t) = e.trailer {
            report.trailer = Some(t.to_string());
        }
        streamed.push((e.pack_offset, format!("{:?}", e.header), e.decompressed_size, e.crc32, e.header.is_delta()));
    }
    report.delta_entries = streamed.iter().filter(|s| s.4).count();

    // 2. Resolve every entry (including OFS deltas via the in-crate delta::apply) from an in-memory pack.
    let file = gix_pack::data::File::from_data(TEST_PACK, "mem.pack".into(), gix_hash::Kind::Sha1)
        .map_err(|e| format!("File::from_data: {e}"))?
        .with_alloc_limit_bytes(Some(64 << 20));
    let mut inflate = gix_zlib::Inflate::default();
    let mut cache = gix_pack::cache::Never;
    let mut objects = HashMap::new();
    for (offset, header, size, crc32, _) in streamed {
        let entry = file.entry(offset).map_err(|e| format!("entry@{offset}: {e}"))?;
        let mut buf = Vec::new();
        let outcome = file
            .decode_entry(entry, &mut buf, &mut inflate, &|_, _| None, &mut cache)
            .map_err(|e| format!("decode_entry@{offset}: {e}"))?;
        // 3. Hash with gix-object.
        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, outcome.kind, &buf).map_err(|e| e.to_string())?;
        report.entries.push(EntryReport {
            offset,
            header,
            decompressed_size: size,
            crc32,
            kind: outcome.kind.to_string(),
            num_deltas: outcome.num_deltas,
            id: id.to_string(),
        });
        objects.insert(id, (outcome.kind, buf));
    }

    // 4. Parse a commit and walk ancestry with gix-traverse over an in-memory Find.
    let head = ObjectId::from_hex(b"b17a78de11335d8d2498c38009d1d14fb46a5b52").map_err(|e| e.to_string())?;
    if let Some((gix_object::Kind::Commit, data)) = objects.get(&head) {
        let commit = gix_object::CommitRef::from_bytes(data, gix_hash::Kind::Sha1).map_err(|e| e.to_string())?;
        report.head_tree = Some(commit.tree().to_string());
    }
    let find = MemFind(objects);
    for info in gix_traverse::commit::Simple::new([head], &find) {
        let info = info.map_err(|e| format!("traverse: {e}"))?;
        report.commit_walk.push(info.id.to_string());
    }
    Ok(())
}

async fn selftest(env: &Env) -> Result<Response> {
    let mut report = SelfTest {
        pack_bytes: TEST_PACK.len(),
        pack_version: String::new(),
        entries: Vec::new(),
        trailer: None,
        delta_entries: 0,
        commit_walk: Vec::new(),
        head_tree: None,
        r2: String::new(),
        errors: Vec::new(),
    };
    if let Err(e) = run_pack_selftest(&mut report) {
        report.errors.push(e);
    }
    // Prove the R2 binding is wired: put + get a small key.
    match r2_roundtrip(env).await {
        Ok(s) => report.r2 = s,
        Err(e) => report.errors.push(format!("r2: {e}")),
    }
    // Exercise the packetline Writer (blocking-io) too, just to link it.
    let mut w = gix_packetline::Writer::new(Vec::<u8>::new());
    w.write_all(b"ping").map_err(|e| Error::RustError(e.to_string()))?;
    Response::from_json(&report)
}

async fn r2_roundtrip(env: &Env) -> Result<String> {
    let bucket = env.bucket("BUCKET")?;
    bucket.put("selftest/hello.txt", "hello from git-edge".to_string()).execute().await?;
    let obj = bucket.get("selftest/hello.txt").execute().await?;
    match obj {
        Some(o) => {
            let body = o.body().ok_or("no body")?.text().await?;
            Ok(format!("put+get ok: {body}"))
        }
        None => Ok("get returned None after put".into()),
    }
}
