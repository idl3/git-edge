//! edge (CONTRACTS.md 1.1, 6, 8, 10): stateless router. Auth first, then the DO.
//! The receive-pack flow honours A2: once the command header parses, every later failure
//! returns HTTP 200 carrying `unpack <err>` + `ng <ref>` for each command.

use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use gix_hash::ObjectId;
use worker::{Env, Method, Request, Response};

use crate::auth::{self, Level};
use crate::error::{respond, Error};
use crate::pack;
use crate::platform;
use crate::store::{Bucket, PushId, RepoId};
use crate::wire::{
    self,
    http::{stub_json, stub_raw, RepoRoute},
    PktReader, PktWriter, RefResult, Service,
};
use crate::ReqBudget;

const CMD_CAP: usize = 1 << 20; // section 6.3: command section cap
const FILL_STEP: usize = 64 << 10; // 6.3: fill in 64 KiB steps

/// The request body as a chunked byte stream (section 6). gzip is decoded through a
/// `DecompressionStream` when `Content-Encoding: gzip` is set; other encodings are rejected.
pub struct BodyReader {
    stream: Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>>>>,
    buf: Vec<u8>,
    eof: bool,
    pub total: u64,
}
impl BodyReader {
    pub fn new(req: &mut Request) -> Result<Self, Error> {
        let enc = req.headers().get("content-encoding").ok().flatten();
        let gzip = enc.as_deref().map(|e| e.eq_ignore_ascii_case("gzip")).unwrap_or(false);
        if !gzip {
            match enc.as_deref() {
                None | Some("identity") => {}
                Some(e) => return Err(Error::Protocol(format!("unsupported content-encoding {e}"))),
            }
        }
        if gzip {
            let raw = req
                .inner()
                .body()
                .ok_or_else(|| Error::Protocol("missing request body".into()))?;
            let ds = web_sys::DecompressionStream::new(web_sys::CompressionFormat::Gzip)
                .map_err(|e| Error::Internal(format!("DecompressionStream: {e:?}")))?;
            let body = raw.pipe_through(wasm_bindgen::JsCast::unchecked_ref::<web_sys::ReadableWritablePair>(&ds));
            let stream = wasm_streams::ReadableStream::from_raw(body).into_stream();
            return Ok(Self::wrap(stream.map(|item| {
                let v = item.map_err(|e| Error::Storage(format!("body stream: {e:?}")))?;
                let a = js_sys::Uint8Array::new(&v);
                let mut b = vec![0u8; a.length() as usize];
                a.copy_to(&mut b);
                Ok(b)
            })));
        }
        let stream = req.stream().map_err(|e| Error::Protocol(e.to_string()))?;
        Ok(Self::wrap(stream.map(|item| item.map_err(|e| Error::Storage(e.to_string())))))
    }
    fn wrap<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Vec<u8>, Error>> + 'static,
    {
        Self { stream: Box::pin(stream), buf: Vec::new(), eof: false, total: 0 }
    }
    /// Read until buf.len() >= min or EOF. Ok(false) means EOF before min.
    pub async fn fill(&mut self, min: usize) -> Result<bool, Error> {
        while self.buf.len() < min && !self.eof {
            match self.stream.next().await {
                Some(Ok(chunk)) => {
                    self.total = self.total.saturating_add(chunk.len() as u64);
                    self.buf.extend_from_slice(&chunk);
                }
                Some(Err(e)) => return Err(e),
                None => self.eof = true,
            }
        }
        Ok(self.buf.len() >= min)
    }
    pub fn buffered(&self) -> &[u8] {
        &self.buf
    }
    pub fn consume(&mut self, n: usize) {
        let n = n.min(self.buf.len());
        self.buf.drain(..n);
    }
    /// Put bytes back at the front (the PktReader's remainder after the receive header).
    pub fn unread(&mut self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let mut b = bytes;
            b.extend_from_slice(&self.buf);
            self.buf = b;
        }
    }
    /// Consume the rest of the request body, bounded. On error paths the response
    /// must not go out while the client is still uploading — Cloudflare resets the
    /// connection and the client sees a transport failure instead of our report-status.
    /// Bounded by bytes AND wall-clock: a stalled or slow-drip client could otherwise
    /// pin the isolate forever — idle awaits don't burn the cpu_ms budget.
    pub async fn drain(&mut self) {
        const MAX_DRAIN: u64 = 2 << 30; // the same ceiling a pushed pack gets
        const IDLE_MS: u64 = 15_000; // per-chunk stall bound, not total
        self.buf.clear();
        while self.total < MAX_DRAIN && !self.eof {
            let next = self.stream.next();
            let timeout = worker::Delay::from(std::time::Duration::from_millis(IDLE_MS));
            futures_util::pin_mut!(next, timeout);
            match futures_util::future::select(next, timeout).await {
                futures_util::future::Either::Left((Some(Ok(chunk)), _)) => {
                    self.total = self.total.saturating_add(chunk.len() as u64);
                }
                // stream end, read error, or stalled past IDLE_MS — answer anyway
                _ => break,
            }
        }
    }
}

/// The one public entry point (lib.rs #[event(fetch)]).
pub async fn fetch(req: Request, env: Env) -> worker::Result<Response> {
    let started = platform::now_ms();
    let path = req.path();
    if path == "/healthz" {
        return Response::ok("ok");
    }
    let (route, rest) = match RepoRoute::parse(&path) {
        Ok(x) => x,
        Err(e) => return respond(Err(e), false),
    };
    // section 10: git-protocol POSTs get a pkt-line ERR; info/refs and _state get plain text
    let git_pkt = matches!((&req.method(), rest.as_str()), (Method::Post, "git-upload-pack" | "git-receive-pack"));
    let (op, repo) = (op_name(&req.method(), &rest), route.name());
    // resolved before the handlers move `env` — absent in local dev, never fatal
    let ds = env.analytics_engine("GE_METRICS").ok();
    let r = match (req.method(), rest.as_str()) {
        (Method::Get, "info/refs") => info_refs(&req, &env, &route).await,
        (Method::Get, "_state") => state_probe(&req, &env, &route).await,
        (Method::Post, "git-upload-pack") => upload_pack(req, &env, &route).await,
        (Method::Post, "git-receive-pack") => receive_pack(req, env, route).await,
        (Method::Post, "_admin/tokens") => token_create(req, &env, &route).await,
        (Method::Get, "_admin/tokens") => token_list(&req, &env, &route).await,
        (Method::Delete, p) if p.starts_with("_admin/tokens/") => {
            token_revoke(&req, &env, &route, &p["_admin/tokens/".len()..]).await
        }
        _ => Err(Error::NotFound),
    };
    let resp = respond(r, git_pkt);
    metric(ds.as_ref(), &repo, op, started, resp.as_ref().ok());
    resp
}

fn op_name(method: &Method, rest: &str) -> &'static str {
    match (method, rest) {
        (Method::Get, "info/refs") => "info-refs",
        (Method::Get, "_state") => "state",
        (Method::Post, "git-upload-pack") => "fetch",
        (Method::Post, "git-receive-pack") => "push",
        (_, p) if p.starts_with("_admin/") => "admin",
        _ => "other",
    }
}

/// One Analytics Engine datapoint per request when GE_METRICS is bound; absent in
/// local dev — never fatal. For streamed fetch responses the duration is
/// time-to-first-byte: the stream outlives the handler.
fn metric(
    ds: Option<&worker::AnalyticsEngineDataset>,
    repo: &str,
    op: &'static str,
    started: i64,
    resp: Option<&Response>,
) {
    let Some(ds) = ds else {
        return;
    };
    let status = resp.map(|r| f64::from(r.status_code())).unwrap_or(0.0);
    let subreqs = resp
        .and_then(|r| r.headers().get("x-ge-subrequests").ok().flatten())
        .and_then(|v| v.split('/').next().and_then(|n| n.parse::<f64>().ok()))
        .unwrap_or(0.0);
    let _ = worker::AnalyticsEngineDataPointBuilder::new()
        .indexes([repo])
        .blobs([op])
        .doubles([status, (platform::now_ms().saturating_sub(started)) as f64, subreqs])
        .write_to(ds);
}

fn git_resp(body: Vec<u8>, content_type: &str) -> Result<Response, Error> {
    let h = worker::Headers::new();
    h.set("Content-Type", content_type).map_err(|e| Error::Internal(e.to_string()))?;
    h.set("Cache-Control", "no-cache").map_err(|e| Error::Internal(e.to_string()))?;
    Ok(Response::from_bytes(body).map_err(|e| Error::Internal(e.to_string()))?.with_headers(h))
}

/// Same as git_resp but streams the body — the DO's fetch response is a stream and must
/// stay one: buffering it would hold an entire clone's pack in edge memory.
fn git_resp_stream<S>(stream: S, content_type: &str) -> Result<Response, Error>
where
    S: Stream<Item = Result<Vec<u8>, worker::Error>> + 'static,
{
    let h = worker::Headers::new();
    h.set("Content-Type", content_type).map_err(|e| Error::Internal(e.to_string()))?;
    h.set("Cache-Control", "no-cache").map_err(|e| Error::Internal(e.to_string()))?;
    Response::from_stream(stream)
        .map(|r| r.with_headers(h))
        .map_err(|e| Error::Internal(e.to_string()))
}

fn protocol_version(req: &Request) -> Result<Option<u8>, Error> {
    Ok(req
        .headers()
        .get("git-protocol")
        .map_err(|e| Error::Internal(e.to_string()))?
        .and_then(|v| {
            v.split(':')
                .filter_map(|p| p.trim().strip_prefix("version=").and_then(|n| n.parse::<u8>().ok()))
                .next()
        }))
}

/// GET /:owner/:repo/info/refs?service=... — v2 advertisement, else v0/v1 (1.1 rules 5, 7).
async fn info_refs(req: &Request, env: &Env, route: &RepoRoute) -> Result<Response, Error> {
    let url = req.url().map_err(|e| Error::Internal(e.to_string()))?;
    let service = url.query_pairs().find(|(k, _)| k == "service").map(|(_, v)| v.into_owned());
    let (service, level, ct) = match service.as_deref() {
        Some("git-upload-pack") => (Service::UploadPack { v1: protocol_version(req)? == Some(1) }, Level::Read, "application/x-git-upload-pack-advertisement"),
        Some("git-receive-pack") => (Service::ReceivePack, Level::Write, "application/x-git-receive-pack-advertisement"),
        _ => return Err(Error::Protocol("service must be git-upload-pack or git-receive-pack".into())),
    };
    auth::authenticate(req, env, level, route).await?; // before the DO wakes (8.1)
    let mut refs_version = None;
    let mut w = PktWriter::default();
    if protocol_version(req)? == Some(2) && matches!(service, Service::UploadPack { .. }) {
        wire::write_capability_advertisement_v2(&mut w);
    } else {
        let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
        #[derive(serde::Deserialize)]
        struct RefDto {
            name: String,
            target: String,
            peeled: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct RefsDto {
            head: Option<String>,
            refs: Vec<RefDto>,
        }
        let mut init = worker::RequestInit::new();
        init.with_method(Method::Get);
        let mut r = worker::Request::new_with_init("https://do/_do/refs", &init)
            .map_err(|e| Error::Internal(e.to_string()))?;
        route.apply_headers(&mut r)?;
        budget.charge(1)?;
        let mut resp = stub.fetch_with_request(r).await?;
        if resp.status_code() != 200 {
            return Err(Error::from_do_response(resp).await);
        }
        refs_version = resp.headers().get("x-ge-refs-version").ok().flatten();
        let dto: RefsDto = resp.json().await.map_err(|e| Error::Internal(e.to_string()))?;
        let refs: Vec<wire::RefRow> = dto
            .refs
            .into_iter()
            .map(|r| {
                Ok(wire::RefRow {
                    name: r.name.into(),
                    target: ObjectId::from_hex(r.target.as_bytes())
                        .map_err(|e| Error::Internal(e.to_string()))?,
                    peeled: r
                        .peeled
                        .map(|p| ObjectId::from_hex(p.as_bytes()).map_err(|e| Error::Internal(e.to_string())))
                        .transpose()?,
                })
            })
            .collect::<Result<_, Error>>()?;
        wire::write_advertisement_v0(&mut w, service, dto.head.as_deref().map(|s| bstr::ByteSlice::as_bstr(s.as_bytes())), &refs);
    }
    let mut out = git_resp(w.out, ct)?;
    if let Some(v) = refs_version {
        out.headers_mut()
            .set("x-ge-refs-version", &v)
            .map_err(|e| Error::Internal(e.to_string()))?;
    }
    Ok(out)
}

/// GET /:owner/:repo/_state — internal observability probe (write-token gated).
async fn state_probe(req: &Request, env: &Env, route: &RepoRoute) -> Result<Response, Error> {
    auth::authenticate(req, env, Level::Write, route).await?;
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let mut init = worker::RequestInit::new();
    init.with_method(Method::Get);
    let mut r = worker::Request::new_with_init("https://do/_do/state", &init)
        .map_err(|e| Error::Internal(e.to_string()))?;
    route.apply_headers(&mut r)?;
    budget.charge(1)?;
    let mut resp = stub.fetch_with_request(r).await?;
    if resp.status_code() != 200 {
        return Err(Error::from_do_response(resp).await);
    }
    let bytes = resp.bytes().await.map_err(|e| Error::Internal(e.to_string()))?;
    git_resp(bytes, "application/json")
}

/// POST /:owner/:repo/git-upload-pack — v2 only; route on the command name, forward raw.
async fn upload_pack(mut req: Request, env: &Env, route: &RepoRoute) -> Result<Response, Error> {
    auth::authenticate(&req, env, Level::Read, route).await?;
    if protocol_version(&req)? != Some(2) {
        // contract 1.1 rule 7: v0 upload-pack POST -> HTTP 400 with the ERR pkt-line
        let mut w = PktWriter::default();
        let _ = w.data(b"ERR protocol v2 required (git >= 2.26)\n");
        w.flush();
        return git_resp(w.out, "application/x-git-upload-pack-result")
            .map(|r| r.with_status(400));
    }
    // bound the read: a declared Content-Length is rejected before any byte is buffered,
    // and a chunked body is still capped at CMD_CAP while streaming in
    if let Some(n) = req
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
    {
        if n > CMD_CAP as u64 {
            return Err(Error::Limit("upload-pack body > 1 MiB".into()));
        }
    }
    let mut reader = BodyReader::new(&mut req)?;
    let mut body = Vec::new();
    while reader.fill(FILL_STEP).await? {
        if reader.buffered().len() > CMD_CAP {
            return Err(Error::Limit("upload-pack body > 1 MiB".into()));
        }
        body.extend_from_slice(reader.buffered());
        reader.consume(usize::MAX);
    }
    body.extend_from_slice(reader.buffered()); // EOF tail shorter than FILL_STEP
    if body.len() > CMD_CAP {
        return Err(Error::Limit("upload-pack body > 1 MiB".into()));
    }
    let path = match wire::parse_v2_command(&body)? {
        wire::V2Command::LsRefs(_) => "/_do/ls-refs",
        wire::V2Command::Fetch(_) => "/_do/fetch",
    };
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let mut resp = stub_raw(&stub, route, path, body, &mut budget).await?;
    if resp.status_code() != 200 {
        return Err(Error::from_do_response(resp).await);
    }
    // fetch responses carry immutable headers; rebuild around the same stream so the
    // pack flows through the edge instead of being buffered whole in memory — and
    // propagate the DO's subrequest accounting so clients see the real budget cost
    let subreqs = resp.headers().get("x-ge-subrequests").ok().flatten();
    let stream = resp.stream().map_err(|e| Error::Internal(e.to_string()))?;
    let mut out = git_resp_stream(stream, "application/x-git-upload-pack-result")?;
    if let Some(v) = subreqs {
        out.headers_mut()
            .set("x-ge-subrequests", &v)
            .map_err(|e| Error::Internal(e.to_string()))?;
    }
    Ok(out)
}

/// POST /:owner/:repo/_admin/tokens {name, level} — mint a per-repo credential.
/// Global-write-token only (authenticate_admin): a repo-level token must not mint
/// more credentials. The token value is returned once and only its hash is stored.
async fn token_create(mut req: Request, env: &Env, route: &RepoRoute) -> Result<Response, Error> {
    auth::authenticate_admin(&req, env)?;
    // {name, level} needs a few hundred bytes; cap before buffering the body whole
    let too_big = req
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|n| n > 65_536)
        .unwrap_or(false);
    if too_big {
        return Err(Error::Protocol("token create body too large".into()));
    }
    let body = req.bytes().await.map_err(|e| Error::Protocol(e.to_string()))?;
    if body.len() > 65_536 {
        // chunked upload had no content-length to check up front
        return Err(Error::Protocol("token create body too large".into()));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| Error::Protocol(format!("token create body: {e}")))?;
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let out: serde_json::Value = stub_json(&stub, route, "/_do/tokens", &v, &mut budget).await?;
    git_resp(serde_json::to_vec(&out).map_err(|e| Error::Internal(e.to_string()))?, "application/json")
}

/// GET /:owner/:repo/_admin/tokens — list per-repo credentials (never the secrets).
async fn token_list(req: &Request, env: &Env, route: &RepoRoute) -> Result<Response, Error> {
    auth::authenticate_admin(req, env)?;
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let mut init = worker::RequestInit::new();
    init.with_method(Method::Get);
    let mut r = worker::Request::new_with_init("https://do/_do/tokens", &init)
        .map_err(|e| Error::Internal(e.to_string()))?;
    route.apply_headers(&mut r)?;
    budget.charge(1)?;
    let mut resp = stub.fetch_with_request(r).await?;
    if resp.status_code() != 200 {
        return Err(Error::from_do_response(resp).await);
    }
    let bytes = resp.bytes().await.map_err(|e| Error::Internal(e.to_string()))?;
    git_resp(bytes, "application/json")
}

/// DELETE /:owner/:repo/_admin/tokens/<id> — revoke a per-repo credential.
async fn token_revoke(req: &Request, env: &Env, route: &RepoRoute, id: &str) -> Result<Response, Error> {
    auth::authenticate_admin(req, env)?;
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(Error::Protocol("bad token id".into()));
    }
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let out: serde_json::Value = stub_json(
        &stub,
        route,
        "/_do/tokens/revoke",
        &serde_json::json!({ "id": id }),
        &mut budget,
    )
    .await?;
    git_resp(serde_json::to_vec(&out).map_err(|e| Error::Internal(e.to_string()))?, "application/json")
}

/// POST /:owner/:repo/git-receive-pack — two-phase push (2.4, 3) with the A2 error arm.
async fn receive_pack(mut req: Request, env: Env, route: RepoRoute) -> Result<Response, Error> {
    let principal = auth::authenticate(&req, &env, Level::Write, &route).await?;
    // A18: sha1 of the presented token is the rate-limit bucket key — the raw
    // credential never crosses the stub boundary (8.1)
    let rate_key = auth::presented_hash(&req)?;
    let mut body = BodyReader::new(&mut req)?;

    // ---- pre-header: parse errors are still normal HTTP errors (A2 arm not yet active) ----
    let (hdr, leftover) = match receive_header(&mut body).await {
        Ok(x) => x,
        Err(e) => {
            // let the client finish uploading first — answering mid-upload gets the
            // connection reset and the client sees a transport error, not our message
            body.drain().await;
            return Err(e);
        }
    };
    body.unread(leftover);

    // flush-only request: "everything up-to-date" — no pack follows, no report needed
    if hdr.commands.is_empty() {
        let mut w = PktWriter::default();
        w.flush();
        return git_resp(w.out, "application/x-git-receive-pack-result");
    }

    // ---- post-header (A2): everything from here reports HTTP 200 + report-status ----
    match receive_inner(&mut body, &env, &route, &hdr, &principal, &rate_key).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            // A18: a throttled push answers a real HTTP 429 + Retry-After, not an
            // in-band unpack error — and skips the drain, since shedding load is the
            // point (a client still mid-upload may see a reset instead of the 429).
            if matches!(e, Error::RateLimit(_)) {
                return Err(e);
            }
            body.drain().await; // finish the client's upload before answering (see drain)
            report_status_200(&hdr, Err(e.client_message()), &[])
        }
    }
}

/// Parse the receive-pack command header: pkt-lines up to the first flush, then the
/// pack follows. Re-parses the whole accumulated buffer each round (bounded by CMD_CAP)
/// because parse_receive_header's accumulators don't survive an Incomplete result.
async fn receive_header(body: &mut BodyReader) -> Result<(wire::ReceiveHeader, Vec<u8>), Error> {
    let mut pr = PktReader::default();
    loop {
        let mut fresh = PktReader::default();
        fresh.push(&pr.buf);
        match wire::parse_receive_header(&mut fresh)? {
            Some(h) => return Ok((h, fresh.remainder())),
            None => {
                body.fill(FILL_STEP).await?;
                if body.buffered().is_empty() {
                    return Err(Error::Protocol("truncated receive header".into()));
                }
                pr.push(&body.buffered().to_vec());
                body.consume(usize::MAX);
                if pr.buf.len() > CMD_CAP {
                    return Err(Error::Protocol("receive header > 1 MiB".into()));
                }
            }
        }
    }
}

async fn receive_inner(
    body: &mut BodyReader,
    env: &Env,
    route: &RepoRoute,
    hdr: &wire::ReceiveHeader,
    principal: &str,
    rate_key: &str,
) -> Result<Response, Error> {
    let (stub, mut budget) = (route.stub(env)?, ReqBudget::paid());
    let push = PushId::random()?;
    #[derive(serde::Deserialize)]
    struct Begin {
        repo_id: String,
        #[serde(default)]
        claimed: bool,
    }
    let begin: Begin = stub_json(
        &stub,
        route,
        "/_do/push/begin",
        &serde_json::json!({ "push_id": push.0, "principal": principal, "key": rate_key }),
        &mut budget,
    )
    .await?;
    // A17: a repo that has never committed a pack must claim one of the owner's
    // GE_QUOTA_MAX_REPOS_PER_OWNER slots in the `owner!<owner>` registry DO before
    // ingest burns bandwidth. Failure here aborts the open push like any post-begin
    // error; the Limit message reaches the client in `unpack`.
    if !begin.claimed {
        if let Err(e) = owner_claim(env, route, &mut budget).await {
            let _: serde_json::Value = stub_json(
                &stub,
                route,
                "/_do/push/abort",
                &serde_json::json!({ "push_id": push.0 }),
                &mut budget,
            )
            .await
            .unwrap_or_default();
            return Err(e);
        }
    }
    let bucket = Bucket::new(env.bucket("BUCKET")?, RepoId(begin.repo_id));

    // a post-begin failure must close the open push row now — leaving it for the
    // janitor's 1h expiry lets 64 failures DoS all pushes
    let run = pack::run::run(body, &bucket, &stub, route, &push, &mut budget).await;
    if run.is_err() {
        let _: serde_json::Value = stub_json(
            &stub,
            route,
            "/_do/push/abort",
            &serde_json::json!({ "push_id": push.0 }),
            &mut budget,
        )
        .await
        .unwrap_or_default();
    }
    let (pack, tags) = run?;
    let commands: Vec<serde_json::Value> = hdr
        .commands
        .iter()
        .map(|c| {
            serde_json::json!({
                "old": c.old.to_string(),
                "new": c.new.to_string(),
                "name": c.name.to_string(),
                "peeled": tags.get(&c.new).map(|t| t.to_string()),
            })
        })
        .collect();
    #[derive(serde::Deserialize)]
    struct Commit {
        results: Vec<(String, Option<String>)>,
    }
    let commit = stub_json(
        &stub,
        route,
        "/_do/push/commit",
        &serde_json::json!({
            "push_id": push.0, "pack_id": pack.map(|p| p.0), "principal": principal, "commands": commands,
        }),
        &mut budget,
    )
    .await;
    // a failed commit (network error, DO 500) must also close the row — the abort is
    // idempotent so a DO-side rejection that already ended the push is harmless
    let res: Commit = match commit {
        Ok(r) => r,
        Err(e) => {
            let _: serde_json::Value = stub_json(
                &stub,
                route,
                "/_do/push/abort",
                &serde_json::json!({ "push_id": push.0 }),
                &mut budget,
            )
            .await
            .unwrap_or_default();
            return Err(e);
        }
    };
    let results: Vec<RefResult> = res
        .results
        .into_iter()
        .map(|(n, e)| match e {
            None => RefResult::Ok(n.into()),
            Some(m) => RefResult::Ng(n.into(), leak_reason(&m)),
        })
        .collect();
    report_status_200(hdr, Ok(()), &results)
}

/// report-status `ng` reasons are `&'static str`; the DO's known reasons get their static form,
/// anything else falls back to git's generic string (the detail is in the unpack line anyway).
fn leak_reason(m: &str) -> &'static str {
    match m {
        "funny refname" => "funny refname",
        "deletion of the current branch prohibited" => "deletion of the current branch prohibited",
        "missing necessary objects" => "missing necessary objects",
        "gc ran during push, retry" => "gc ran during push, retry",
        _ => "failed to update ref",
    }
}

/// A2 post-header response: HTTP 200, `unpack <msg>` + `ng <ref> <why>` for every command.
fn report_status_200(
    hdr: &wire::ReceiveHeader,
    unpack: Result<(), String>,
    results: &[RefResult],
) -> Result<Response, Error> {
    // report-status is a negotiated capability — a client that didn't ask for it gets
    // a bare 200, not a report body it isn't demuxing
    if !hdr.caps.report_status && !hdr.caps.report_status_v2 {
        return git_resp(Vec::new(), "application/x-git-receive-pack-result");
    }
    let mut w = PktWriter::default();
    let results_buf;
    let results = if results.is_empty() && unpack.is_err() {
        results_buf = hdr
            .commands
            .iter()
            .map(|c| RefResult::Ng(c.name.clone(), "unpack failed"))
            .collect::<Vec<_>>();
        &results_buf
    } else {
        results
    };
    wire::write_report_status(&mut w, unpack.as_ref().map(|_|()).map_err(String::as_str), results, &hdr.caps)?;
    git_resp(w.out, "application/x-git-receive-pack-result")
}

/// Integer env knob, edge side — mirrors RepoDo::env_i64.
fn env_i64(env: &Env, name: &str, default: i64) -> i64 {
    env.var(name)
        .ok()
        .and_then(|v| v.to_string().parse::<i64>().ok())
        .unwrap_or(default)
}

/// A17: claim one of the owner's GE_QUOTA_MAX_REPOS_PER_OWNER slots in the
/// `owner!<owner>` registry DO — the same RepoDo class under a name no repo route
/// can produce (`!` fails seg_ok), so no extra migration or binding is needed.
/// `<= 0` disables the cap entirely (no registry hop). Over-cap -> Error::Limit,
/// which lands in the client's `unpack` line via the A2 arm.
async fn owner_claim(env: &Env, route: &RepoRoute, budget: &mut ReqBudget) -> Result<(), Error> {
    if env_i64(env, "GE_QUOTA_MAX_REPOS_PER_OWNER", 50) <= 0 {
        return Ok(());
    }
    let stub = env
        .durable_object("REPO")
        .map_err(|e| Error::Internal(e.to_string()))?
        .id_from_name(&format!("owner!{}", route.owner))
        .map_err(|e| Error::Internal(e.to_string()))?
        .get_stub()
        .map_err(|e| Error::Internal(e.to_string()))?;
    let mut init = worker::RequestInit::new();
    init.with_method(Method::Post);
    let mut r = worker::Request::new_with_init("https://do/_owner/claim", &init)
        .map_err(|e| Error::Internal(e.to_string()))?;
    route.apply_headers(&mut r)?;
    budget.charge(1)?;
    let resp = stub.fetch_with_request(r).await?;
    if resp.status_code() != 200 {
        return Err(Error::from_do_response(resp).await);
    }
    Ok(())
}
