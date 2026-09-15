//! RepoDo — one Durable Object per repo: refs, meta, pushes, jobs tables; internal HTTP routes.
//! CONTRACTS.md 1.3, 3, 8. Ported from repo-do-ref-authority + refs-sqlite-objects-r2 + auth proofs.

use std::cell::RefCell;

use bstr::{BString, ByteSlice};
use futures_util::stream;
use gix_hash::ObjectId;
use worker::{durable_object, DurableObject, Env, Method, Request, Response, SqlCursor, SqlStorage, SqlStorageValue as V, State};

use crate::error::Error;
use crate::jobs::{self, JobKind};
use crate::pack::generate;
use crate::platform;
use crate::store::{schema, Bucket, Index, ObjLoc, PackId, RepoId};
use crate::wire::{self, http::{do_error_response, json, parse, RepoHeaders}, FetchArgs, PktWriter, RefRow, V2Command};
use crate::ReqBudget;

/// A ref row is ~90 bytes in an ls-refs advertisement — a token holder could otherwise
/// mint refs until the advertisement alone exceeds the isolate. 65k refs is already huge.
const MAX_REFS: i64 = 65_536;

#[derive(serde::Deserialize)]
pub struct CmdDto {
    old: String,
    new: String,
    name: String,
    peeled: Option<String>,
}
#[derive(serde::Deserialize)]
pub struct CommitRequest {
    pub push_id: String,
    pub pack_id: Option<String>,
    pub principal: String,
    pub commands: Vec<CmdDto>,
}
#[derive(serde::Serialize)]
pub struct CommitResponse {
    pub results: Vec<(String, Option<&'static str>)>, // None = ok
}
#[derive(serde::Deserialize)]
struct BeginDto {
    push_id: String,
    principal: String,
}
#[derive(serde::Deserialize)]
struct AuthDto {
    hash: String,
}
#[derive(serde::Deserialize)]
struct NewToken {
    name: String,
    level: String,
}
#[derive(serde::Deserialize)]
struct RevokeDto {
    id: String,
}
#[derive(serde::Deserialize)]
struct N {
    n: i64,
}
#[derive(serde::Deserialize)]
struct PushRow {
    state: String,
    gc_epoch: i64,
}
struct Cmd {
    old: ObjectId,
    new: ObjectId,
    name: String,
    peeled: Option<String>,
}

pub struct Meta {
    pub repo_id: String,
    pub owner: String,
    pub repo: String,
    pub head: String,
    pub refs_version: i64,
    pub gc_epoch: i64,
}

#[durable_object]
pub struct RepoDo {
    pub(crate) state: State,
    pub(crate) env: Env,
    booted: RefCell<bool>,
}

impl DurableObject for RepoDo {
    fn new(state: State, env: Env) -> Self {
        Self { state, env, booted: RefCell::new(false) }
    }

    async fn fetch(&self, mut req: Request) -> worker::Result<Response> {
        let hdr = RepoHeaders::from_request(&req);
        let body = req.bytes().await?; // the only await on a "none" route
        let path = req.path();
        let method = req.method();
        // ---- sync span from here to the response for every "none" route ----
        let out: Result<Option<Response>, Error> = self.boot(&hdr).and_then(|meta| {
            match (&method, path.as_str()) {
                (Method::Get, "/_do/refs") => self.list_refs().and_then(refs_json),
                (Method::Get, "/_do/state") => self.debug_state(),
                (Method::Post, "/_do/push/begin") => self.push_begin(&meta, &parse::<BeginDto>(&body)?),
                (Method::Post, "/_do/push/lookup") => self.push_lookup(&parse(&body)?),
                (Method::Post, "/_do/push/index") => self.push_index(&parse(&body)?),
                (Method::Post, "/_do/push/abort") => self.push_abort(&parse(&body)?),
                (Method::Post, "/_do/push/commit") => {
                    self.commit_push(&parse::<CommitRequest>(&body)?).and_then(|r| json(serde_json::to_value(&r)?))
                }
                (Method::Post, "/_do/ls-refs") => self.ls_refs(&meta, &body),
                (Method::Post, "/_do/auth") => self.auth_lookup(&parse(&body)?),
                (Method::Post, "/_do/tokens") => self.token_create(&parse(&body)?),
                (Method::Get, "/_do/tokens") => self.token_list(),
                (Method::Post, "/_do/tokens/revoke") => self.token_revoke(&parse(&body)?),
                _ => return Ok(None),
            }
            .map(Some)
        });
        let resp = match out {
            Ok(Some(r)) => Ok(r),
            Ok(None) => match (&method, path.as_str()) {
                // the await route lives outside the sync span (1.3); boot may still have
                // enqueued jobs in its span, so a successful fetch must rearm the alarm
                (Method::Post, "/_do/fetch") => {
                    let r = self.fetch_v2(&body).await;
                    // boot may have enqueued jobs in this span — rearm even on error;
                    // a propagated Storage/Internal error rolls the span back anyway
                    let _ = jobs::rearm(self).await;
                    return r;
                }
                _ => Err(Error::NotFound),
            },
            Err(e) => Err(e),
        };
        match resp {
            Ok(r) => {
                // A3/A11: every span that can enqueue gets an unconditional rearm after it.
                let _ = jobs::rearm(self).await;
                Ok(r)
            }
            // Storage/Internal propagate so the platform rolls back the uncommitted span (A2).
            Err(e @ (Error::Storage(_) | Error::Internal(_))) => {
                Err(worker::Error::RustError(e.message()))
            }
            Err(e) => {
                // non-fatal errors commit the span — a boot-time enqueue survives, so rearm
                let r = do_error_response(&e)?;
                let _ = jobs::rearm(self).await;
                Ok(r)
            }
        }
    }

    async fn alarm(&self) -> worker::Result<Response> {
        self.boot(&RepoHeaders::NONE)?; // alarm has no headers, section 8.2
        let _ = jobs::dispatch(self).await; // never lets an Err escape, section 4.4
        Response::empty()
    }
}

pub fn oid(hex: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(hex.as_bytes()).map_err(|e| Error::Protocol(e.to_string()))
}
fn refs_json((head, refs): (Option<BString>, Vec<RefRow>)) -> Result<Response, Error> {
    json(serde_json::json!({
        "head": head.map(|h| h.to_string()),
        "refs": refs.iter().map(|r| serde_json::json!({
            "name": r.name.to_string(), "target": r.target.to_string(),
            "peeled": r.peeled.map(|p| p.to_string()) })).collect::<Vec<_>>()
    }))
}

impl RepoDo {
    pub fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }
    /// `purge_repo` may drop the schema under a live `booted` flag; the next
    /// request must re-run `schema::migrate` or every query hits missing tables.
    pub(crate) fn unboot(&self) {
        *self.booted.borrow_mut() = false;
    }
    pub fn q(&self, s: &str, args: Vec<V>) -> Result<SqlCursor, Error> {
        self.sql().exec(s, Some(args)).map_err(|e| Error::Storage(e.to_string()))
    }
    /// The one CAS oracle (section 3): issued right after the write, same span. Never rows_written.
    pub fn changes(&self) -> Result<i64, Error> {
        Ok(self.q("SELECT changes() AS n", vec![])?.one::<N>()?.n)
    }
    pub fn meta(&self, key: &str) -> Result<String, Error> {
        // one() on an empty cursor throws at the JS boundary (not a catchable Rust
        // Err) — a missing key must not crash the isolate, so go through meta_opt
        self.meta_opt(key)?.ok_or_else(|| Error::Storage(format!("meta.{key} missing")))
    }
    pub fn meta_opt(&self, key: &str) -> Result<Option<String>, Error> {
        #[derive(serde::Deserialize)]
        struct S {
            value: String,
        }
        Ok(self
            .q("SELECT value FROM meta WHERE key=?", vec![V::from(key)])?
            .to_array::<S>()?
            .into_iter()
            .next()
            .map(|s| s.value))
    }
    pub fn meta_i64(&self, key: &str) -> Result<i64, Error> {
        self.meta(key)?.parse().map_err(|_| Error::Internal(format!("meta.{key}")))
    }
    fn ref_count(&self) -> Result<i64, Error> {
        Ok(self.q("SELECT COUNT(*) AS n FROM refs", vec![])?.one::<N>()?.n)
    }
    pub fn bucket(&self) -> Result<Bucket, Error> {
        Ok(Bucket::new(self.env.get_binding("BUCKET")?, self.repo_id()?))
    }
    pub fn repo_id(&self) -> Result<RepoId, Error> {
        Ok(RepoId(self.meta("repo_id")?))
    }
    /// GC quiet period in ms — GE_GC_QUIET_MS overrides the 10-minute contract default so
    /// the chain can be exercised in tests and dev without waiting.
    pub fn gc_quiet_ms(&self) -> i64 {
        self.env_ms("GE_GC_QUIET_MS", 600_000)
    }
    /// GC grace period in ms — GE_GC_GRACE_MS overrides the contract's 1-hour window.
    /// Below it, packs stay out of `marked` entirely so a push can never lose bytes mid-flight.
    pub fn gc_grace_ms(&self) -> i64 {
        self.env_ms("GE_GC_GRACE_MS", 3_600_000)
    }
    fn env_ms(&self, name: &str, default: i64) -> i64 {
        self.env
            .var(name)
            .ok()
            .and_then(|v| v.to_string().parse::<i64>().ok())
            .unwrap_or(default)
    }

    /// Section 8.2. Sync span: migrate schema, write/verify meta, enqueue Janitor.
    pub fn boot(&self, hdr: &RepoHeaders) -> Result<Meta, Error> {
        let sql = self.sql();
        if !*self.booted.borrow() {
            schema::migrate(&sql)?;
            *self.booted.borrow_mut() = true;
        }
        // A4: re-queue a dead maintenance job at every boot, and requeue 'running' rows that a
        // killed isolate stranded (A12).
        jobs::repair(&sql)?;
        #[derive(serde::Deserialize)]
        struct KV {
            key: String,
            value: String,
        }
        let rows = self.q("SELECT key, value FROM meta", vec![])?.to_array::<KV>()?;
        let get = |k: &str| rows.iter().find(|r| r.key == k).map(|r| r.value.clone());
        let meta = match get("repo_id") {
            Some(repo_id) => {
                let need = |k: &str| get(k).ok_or_else(|| Error::Internal(format!("meta.{k} missing")));
                let num = |k: &str| need(k)?.parse::<i64>().map_err(|_| Error::Internal(format!("meta.{k}")));
                Meta {
                    repo_id,
                    owner: need("owner")?,
                    repo: need("repo")?,
                    head: need("head")?,
                    refs_version: num("refs_version")?,
                    gc_epoch: num("gc_epoch")?,
                }
            }
            None => {
                let (owner, repo) = (
                    hdr.owner.clone().ok_or_else(|| Error::Internal("boot without headers".into()))?,
                    hdr.repo.clone().ok_or_else(|| Error::Internal("boot without headers".into()))?,
                );
                let (repo_id, now) = (platform::hex16()?, platform::now_ms());
                for (k, v) in [
                    ("repo_id", repo_id.as_str()),
                    ("owner", owner.as_str()),
                    ("repo", repo.as_str()),
                    ("head", "refs/heads/main"),
                    ("refs_version", "0"),
                    ("gc_epoch", "0"),
                    ("created_at", &now.to_string()),
                    ("schema_version", "1"),
                ] {
                    self.q(
                        "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT DO NOTHING",
                        vec![V::from(k), V::from(v)],
                    )?;
                }
                jobs::enqueue(&sql, JobKind::Janitor, now, "{}")?;
                Meta {
                    repo_id,
                    owner,
                    repo,
                    head: "refs/heads/main".into(),
                    refs_version: 0,
                    gc_epoch: 0,
                }
            }
        };
        if let (Some(o), Some(r)) = (&hdr.owner, &hdr.repo) {
            if *o != meta.owner || *r != meta.repo {
                return Err(Error::Internal("identity mismatch".into()));
            }
        }
        // The one permitted ctx.id.name read (8.3): a cross-check that logs and decides nothing.
        let seen = platform::do_id_name(&self.state);
        if seen.as_deref() != Some(&format!("{}/{}", meta.owner, meta.repo)) {
            worker::console_log!("warn: ctx.id.name {seen:?} vs meta {}/{}", meta.owner, meta.repo);
        }
        Ok(meta)
    }

    /// GET /_do/state — internal observability: row counts per table and jobs by state.
    /// Not part of the git protocol surface; used by tests and ops to watch the job chain.
    fn debug_state(&self) -> Result<Response, Error> {
        #[derive(serde::Deserialize)]
        struct N {
            n: i64,
        }
        let count = |q: &str| -> Result<i64, Error> {
            Ok(self.q(q, vec![])?.one::<N>()?.n)
        };
        json(serde_json::json!({
            "refs": count("SELECT COUNT(*) AS n FROM refs")?,
            "objects": count("SELECT COUNT(*) AS n FROM objects")?,
            "packs_live": count("SELECT COUNT(*) AS n FROM packs WHERE state='live'")?,
            "packs_ingesting": count("SELECT COUNT(*) AS n FROM packs WHERE state='ingesting'")?,
            "packs_dead": count("SELECT COUNT(*) AS n FROM packs WHERE state='dead'")?,
            "jobs_queued": count("SELECT COUNT(*) AS n FROM jobs WHERE state='queued'")?,
            "jobs_running": count("SELECT COUNT(*) AS n FROM jobs WHERE state='running'")?,
            "jobs_dead": count("SELECT COUNT(*) AS n FROM jobs WHERE state='dead'")?,
            "pushes": count("SELECT COUNT(*) AS n FROM pushes")?,
            "marked": count("SELECT COUNT(*) AS n FROM marked")?,
            "tokens": count("SELECT COUNT(*) AS n FROM tokens")?,
        }))
    }

    /// POST /_do/auth {hash} — edge authenticates a presented token by its sha1 hash.
    /// The raw token never crosses the stub boundary. No row: Auth, which the edge
    /// maps back to a 401 challenge.
    fn auth_lookup(&self, b: &AuthDto) -> Result<Response, Error> {
        #[derive(serde::Deserialize)]
        struct R {
            level: String,
            name: String,
        }
        let row = self
            .q("SELECT level, name FROM tokens WHERE hash=?", vec![V::from(b.hash.as_str())])?
            .to_array::<R>()?
            .into_iter()
            .next()
            .ok_or(Error::Auth)?;
        json(serde_json::json!({ "level": row.level, "name": row.name }))
    }

    /// POST /_do/tokens {name, level} — the edge has already gated this on the global
    /// write token. Returns the token value once; only its sha1 hash is stored.
    fn token_create(&self, b: &NewToken) -> Result<Response, Error> {
        if !matches!(b.level.as_str(), "read" | "write") {
            return Err(Error::Protocol("level must be read or write".into()));
        }
        if b.name.is_empty() || b.name.len() > 128 {
            return Err(Error::Protocol("name must be 1-128 bytes".into()));
        }
        let n = self.q("SELECT COUNT(*) AS n FROM tokens", vec![])?.one::<N>()?.n;
        if n >= 256 {
            return Err(Error::Limit("too many tokens (256 max)".into()));
        }
        let token = format!("ge_{}{}", platform::hex16()?, platform::hex16()?);
        let id = platform::hex16()?;
        self.q(
            "INSERT INTO tokens(id,hash,level,name,created_at) VALUES(?,?,?,?,?)",
            vec![
                V::from(id.as_str()),
                V::from(crate::auth::token_hash(&token)?),
                V::from(b.level.as_str()),
                V::from(b.name.as_str()),
                V::from(platform::now_ms()),
            ],
        )?;
        json(serde_json::json!({ "id": id, "token": token, "level": b.level, "name": b.name }))
    }

    /// GET /_do/tokens — id/name/level only; hashes and token values never leave.
    fn token_list(&self) -> Result<Response, Error> {
        #[derive(serde::Deserialize)]
        struct T {
            id: String,
            name: String,
            level: String,
            created_at: i64,
        }
        let rows = self
            .q("SELECT id, name, level, created_at FROM tokens ORDER BY created_at", vec![])?
            .to_array::<T>()?;
        json(serde_json::json!({
            "tokens": rows
                .iter()
                .map(|t| serde_json::json!({
                    "id": t.id, "name": t.name, "level": t.level, "created_at": t.created_at,
                }))
                .collect::<Vec<_>>()
        }))
    }

    /// POST /_do/tokens/revoke {id}.
    fn token_revoke(&self, b: &RevokeDto) -> Result<Response, Error> {
        self.q("DELETE FROM tokens WHERE id=?", vec![V::from(b.id.as_str())])?;
        json(serde_json::json!({ "revoked": self.changes()? > 0 }))
    }

    /// GET /_do/refs (1.3, awaits: none).
    pub fn list_refs(&self) -> Result<(Option<BString>, Vec<RefRow>), Error> {
        #[derive(serde::Deserialize)]
        struct R {
            name: String,
            target: String,
            peeled: Option<String>,
        }
        let head = self.meta("head")?;
        let mut refs = Vec::new();
        for r in self
            .q("SELECT name, target, peeled FROM refs ORDER BY name", vec![])?
            .to_array::<R>()?
        {
            let target =
                ObjectId::from_hex(r.target.as_bytes()).map_err(|_| Error::Internal("bad oid in refs".into()))?;
            let peeled = r
                .peeled
                .as_deref()
                .map(|p| ObjectId::from_hex(p.as_bytes()).map_err(|_| Error::Internal("bad peeled".into())))
                .transpose()?;
            refs.push(RefRow { name: r.name.into(), target, peeled });
        }
        Ok((Some(head.into()), refs))
    }

    /// POST /_do/ls-refs (raw v2 body, awaits: none).
    fn ls_refs(&self, _meta: &Meta, body: &[u8]) -> Result<Response, Error> {
        let args = match wire::parse_v2_command(body)? {
            V2Command::LsRefs(a) => a,
            _ => return Err(Error::Protocol("not ls-refs".into())),
        };
        let (head, refs) = self.list_refs()?;
        let mut w = PktWriter::default();
        wire::write_ls_refs(&mut w, &args, head.as_deref().map(|v| bstr::ByteSlice::as_bstr(v.as_slice())), &refs);
        Response::from_bytes(w.out).map_err(|e| Error::Internal(e.to_string()))
    }

    /// Section 3 step 0: the push row carrying the gc_epoch the whole push validates against.
    fn push_begin(&self, meta: &Meta, b: &BeginDto) -> Result<Response, Error> {
        // an 'open' push owns a pending/ key and eventually a packs row; bound how many
        // a client may hold at once (expired ones are reaped by the janitor)
        let open = self.q("SELECT COUNT(*) AS n FROM pushes WHERE state='open'", vec![])?.one::<N>()?.n;
        if open >= 64 {
            return Err(Error::Limit("too many open pushes".into()));
        }
        self.q(
            "INSERT INTO pushes(id,state,principal,began_at,gc_epoch) VALUES(?,'open',?,?,?)",
            vec![
                V::from(b.push_id.as_str()),
                V::from(b.principal.as_str()),
                V::from(platform::now_ms()),
                V::from(meta.gc_epoch),
            ],
        )?;
        json(serde_json::json!({
            "repo_id": meta.repo_id, "refs_version": meta.refs_version, "gc_epoch": meta.gc_epoch
        }))
    }

    /// POST /_do/push/abort — a post-begin failure closes the row immediately instead
    /// of leaving it `open` for the janitor's 1h timeout (64 leaked rows = push DoS).
    fn push_abort(&self, b: &serde_json::Value) -> Result<Response, Error> {
        let id = b.get("push_id").and_then(|v| v.as_str()).ok_or_else(|| Error::Protocol("push_id".into()))?;
        self.q(
            "UPDATE pushes SET state='dead' WHERE id=? AND state='open'",
            vec![V::from(id)],
        )?;
        json(serde_json::json!({ "ok": true }))
    }

    /// POST /_do/push/lookup (sync). `pack` = the caller's own ingesting pack counts (2.5).
    fn push_lookup(&self, b: &LookupDto) -> Result<Response, Error> {
        if b.ids.len() > 1_000 {
            return Err(Error::Internal("lookup > 1000 ids".into()));
        }
        let ids = b.ids.iter().map(|h| oid(h)).collect::<Result<Vec<_>, Error>>()?;
        let sql = self.sql();
        let idx = Index(&sql);
        let mut locs = idx.lookup(&ids)?;
        if let Some(p) = &b.pack {
            let pack = PackId(p.clone());
            for (id, loc) in ids.iter().zip(locs.iter_mut()) {
                if loc.is_none() {
                    *loc = idx.lookup_in_pack(id, &pack)?;
                }
            }
        }
        json(serde_json::json!({ "locs": locs }))
    }

    /// Upsert the packs row ('ingesting') and insert <= 10,000 rows. Refused once the push
    /// is not 'open', so an expired push can never add rows (5.1/5.2, 3 step 1).
    fn push_index(&self, b: &IndexDto) -> Result<Response, Error> {
        if b.rows.len() > 10_000 {
            return Err(Error::Internal("index > 10000 rows".into()));
        }
        let m = &b.pack;
        #[derive(serde::Deserialize)]
        struct StateRow {
            state: String,
        }
        let st = self
            .q("SELECT state FROM pushes WHERE id=?", vec![V::from(m.push_id.as_str())])?
            .to_array::<StateRow>()?
            .into_iter()
            .next();
        if st.map(|s| s.state).as_deref() != Some("open") {
            return Err(Error::Conflict("push not open".into()));
        }
        self.q(
            "INSERT INTO packs(id,state,count,bytes,commit_lo,commit_hi,push_id,created_at) \
             VALUES(?,'ingesting',?,?,?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET count=excluded.count, bytes=excluded.bytes, \
             commit_lo=excluded.commit_lo, commit_hi=excluded.commit_hi \
             WHERE packs.state='ingesting' AND packs.push_id=excluded.push_id",
            vec![
                V::from(m.id.as_str()),
                V::from(m.count),
                V::from(m.bytes),
                V::from(m.commit_lo),
                V::from(m.commit_hi),
                V::from(m.push_id.as_str()),
                V::from(platform::now_ms()),
            ],
        )?;
        if self.changes()? != 1 {
            return Err(Error::Conflict("pack not ingesting for this push".into()));
        }
        Index(&self.sql()).insert_objects(&PackId(m.id.clone()), &b.rows)?;
        json(serde_json::json!({}))
    }

    /// Section 3, steps 1-7. One sync span: no await, no R2, no stub between first SELECT and return.
    pub fn commit_push(&self, req: &CommitRequest) -> Result<CommitResponse, Error> {
        let cmds = req
            .commands
            .iter()
            .map(|c| {
                Ok(Cmd {
                    old: oid(&c.old)?,
                    new: oid(&c.new)?,
                    name: c.name.clone(),
                    peeled: c.peeled.clone(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let now = platform::now_ms();
        let push = self
            .q("SELECT state, gc_epoch FROM pushes WHERE id=?", vec![V::from(req.push_id.as_str())])?
            .to_array::<PushRow>()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Conflict("unknown push".into()))?;
        if push.state != "open" {
            return Err(Error::Conflict(format!("push is {}", push.state))); // step 1
        }
        if self.meta_i64("gc_epoch")? != push.gc_epoch {
            // step 2
            let results = cmds
                .iter()
                .map(|c| (c.name.clone(), Some("gc ran during push, retry")))
                .collect::<Vec<_>>();
            return self.finish_push(req, "rejected", now, results);
        }
        if let Some(pack) = &req.pack_id {
            // step 3
            self.q(
                // bound to this push: an ingesting pack that belongs to a different push
                // must not be promotable by someone else's commit
                "UPDATE packs SET state='live' WHERE id=? AND state='ingesting' AND push_id=?",
                vec![V::from(pack.as_str()), V::from(req.push_id.as_str())],
            )?;
            if self.changes()? != 1 {
                return Err(Error::Conflict("pack not in state ingesting".into()));
            }
        }
        let head = self.meta("head")?;
        let sql = self.sql();
        let idx = Index(&sql);
        let mut results = Vec::with_capacity(cmds.len());
        let mut any_ok = false;
        for c in &cmds {
            // step 4
            let r = self.apply_one(&idx, &head, c, &req.push_id, &req.principal, now)?;
            any_ok |= r.is_none();
            results.push((c.name.clone(), r));
        }
        if any_ok {
            self.q("UPDATE meta SET value=value+1 WHERE key='refs_version'", vec![])?; // step 5
            // a repo created by pushing a non-main history (e.g. master-first) leaves
            // meta.head dangling — adopt an existing branch or clones can't check out
            self.q(
                "UPDATE meta SET value=(SELECT name FROM refs WHERE name LIKE 'refs/heads/%' \
                 ORDER BY name LIMIT 1) WHERE key='head' \
                 AND NOT EXISTS(SELECT 1 FROM refs WHERE name=meta.value) \
                 AND EXISTS(SELECT 1 FROM refs WHERE name LIKE 'refs/heads/%')",
                vec![],
            )?;
            jobs::enqueue(&sql, JobKind::GcMark, now + self.gc_quiet_ms(), "{}")?; // step 7 (dedups)
        }
        self.finish_push(req, "committed", now, results) // step 6
    }

    /// One RefCommand, independent of its siblings (git default; `atomic` is not advertised).
    fn apply_one(
        &self,
        idx: &Index<'_>,
        head: &str,
        c: &Cmd,
        push: &str,
        who: &str,
        now: i64,
    ) -> Result<Option<&'static str>, Error> {
        // refs live under refs/ only — a full valid refname, never HEAD or a bare word
        if !c.name.as_bytes().starts_with(b"refs/")
            || gix_validate::reference::name(c.name.as_bytes().as_bstr()).is_err()
        {
            return Ok(Some("funny refname"));
        }
        if c.old.is_null() && c.new.is_null() {
            return Ok(Some("funny refname"));
        }
        if c.new.is_null() && c.name == head {
            return Ok(Some("deletion of the current branch prohibited"));
        }
        if !c.new.is_null() {
            // 2.5 re-guard: the tip must be live *now*, in this span. The push's pack is live since step 3.
            let loc: Option<ObjLoc> = idx.lookup(&[c.new])?.into_iter().next().flatten();
            if loc.is_none() {
                return Ok(Some("missing necessary objects"));
            }
            // a ref create costs the writer ~90 bytes of header but a reader the whole
            // table — bound it or ls-refs becomes an unbounded advertisement
            if c.old.is_null() && self.ref_count()? >= MAX_REFS {
                return Ok(Some("too many refs"));
            }
        }
        let (o, n, name) = (c.old.to_string(), c.new.to_string(), c.name.as_str());
        let peeled = c.peeled.clone().map_or(V::Null, |p| V::from(p));
        if c.old.is_null() {
            self.q(
                "INSERT INTO refs(name,target,peeled,updated_at) VALUES(?,?,?,?) ON CONFLICT DO NOTHING",
                vec![V::from(name), V::from(n.as_str()), peeled, V::from(now)],
            )?;
        } else if c.new.is_null() {
            self.q(
                "DELETE FROM refs WHERE name=? AND target=?",
                vec![V::from(name), V::from(o.as_str())],
            )?;
        } else {
            self.q(
                "UPDATE refs SET target=?, peeled=?, updated_at=? WHERE name=? AND target=?",
                vec![V::from(n.as_str()), peeled, V::from(now), V::from(name), V::from(o.as_str())],
            )?;
        }
        if self.changes()? != 1 {
            return Ok(Some("failed to update ref")); // git's own ng string
        }
        self.q(
            "INSERT INTO reflog(name,old,new,push_id,principal,at) VALUES(?,?,?,?,?,?)",
            vec![
                V::from(name),
                V::from(o.as_str()),
                V::from(n.as_str()),
                V::from(push),
                V::from(who),
                V::from(now),
            ],
        )?;
        Ok(None)
    }

    fn finish_push(
        &self,
        req: &CommitRequest,
        state: &str,
        now: i64,
        results: Vec<(String, Option<&'static str>)>,
    ) -> Result<CommitResponse, Error> {
        let result = serde_json::to_string(&results).map_err(|e| Error::Internal(e.to_string()))?;
        self.q(
            "UPDATE pushes SET state=?, ended_at=?, pack_id=?, result=? WHERE id=?",
            vec![
                V::from(state),
                V::from(now),
                req.pack_id.as_deref().map_or(V::Null, V::from),
                V::from(result.as_str()),
                V::from(req.push_id.as_str()),
            ],
        )?;
        Ok(CommitResponse { results })
    }

    /// POST /_do/fetch (section 9): the only DO route that awaits (R2 reads).
    async fn fetch_v2(&self, body: &[u8]) -> worker::Result<Response> {
        match self.fetch_v2_inner(body).await {
            Ok(r) => Ok(r),
            Err(e @ (Error::Storage(_) | Error::Internal(_))) => Err(worker::Error::RustError(e.message())),
            Err(e) => {
                worker::console_log!("fetch_v2: {e}");
                do_error_response(&e).map_err(worker::Error::from)
            }
        }
    }

    async fn fetch_v2_inner(&self, body: &[u8]) -> Result<Response, Error> {
        let args = match wire::parse_v2_command(body)? {
            V2Command::Fetch(a) => a,
            _ => return Err(Error::Protocol("not fetch".into())),
        };
        let mut budget = ReqBudget::paid();
        let bucket = self.bucket()?;
        let sql = self.sql();
        let idx = Index(&sql);
        // step 0: want-ref names resolve against the refs table (HEAD follows meta.head);
        // the resolved pairs ride back in the wanted-refs section
        let mut wants = args.wants.clone();
        let mut wanted_refs: Vec<(ObjectId, BString)> = Vec::new();
        if !args.want_refs.is_empty() {
            #[derive(serde::Deserialize)]
            struct T {
                target: String,
            }
            let head = self.meta("head")?;
            for name in &args.want_refs {
                let qname = if name.as_slice() == b"HEAD" {
                    head.clone()
                } else {
                    String::from_utf8(name.to_vec()).map_err(|_| Error::Protocol("bad want-ref".into()))?
                };
                let target = self
                    .q("SELECT target FROM refs WHERE name=?", vec![V::from(qname.as_str())])?
                    .to_array::<T>()?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::Protocol(format!("couldn't find remote ref {qname}")))?;
                let id = oid(&target.target)?;
                wanted_refs.push((id, name.clone()));
                wants.push(id);
            }
        }
        // deepen-not carries ref names: resolve each to its tip commit — peel annotated
        // tags (`peeled` holds the target commit; NULL on pre-column rows → peel one
        // level via the index). Clients send unqualified names ("mid"), so resolve by
        // git's standard search order; an unresolvable name is an error, like want-ref.
        let mut deepen_not: Vec<ObjectId> = Vec::new();
        if !args.deepen_not.is_empty() {
            #[derive(serde::Deserialize)]
            struct T {
                target: String,
                peeled: Option<String>,
            }
            for name in &args.deepen_not {
                let qname = String::from_utf8(name.to_vec())
                    .map_err(|_| Error::Protocol(format!("couldn't find remote ref {}", name.as_bstr())))?;
                let mut found = None;
                for cand in [
                    qname.clone(),
                    format!("refs/{qname}"),
                    format!("refs/tags/{qname}"),
                    format!("refs/heads/{qname}"),
                    format!("refs/remotes/{qname}"),
                    format!("refs/remotes/{qname}/HEAD"),
                ] {
                    if let Some(t) = self
                        .q("SELECT target, peeled FROM refs WHERE name=? LIMIT 1", vec![V::from(cand.as_str())])?
                        .to_array::<T>()?
                        .into_iter()
                        .next()
                    {
                        found = Some(t);
                        break;
                    }
                }
                let t = found.ok_or_else(|| {
                    Error::Protocol(format!("couldn't find remote ref {qname}"))
                })?;
                let mut id = match t.peeled {
                    Some(p) => oid(&p)?,
                    None => oid(&t.target)?,
                };
                // pre-column rows can leave peeled NULL on annotated tags — if the
                // target is itself a tag object, read it and peel one level so the
                // exclusion walk starts at the commit
                if let Some(l) = idx.lookup(&[id])?.into_iter().next().flatten() {
                    if l.kind == gix_object::Kind::Tag {
                        if let Some((_, entry)) =
                            bucket.read_entries(&[(id, l)], &mut budget).await?.into_iter().next()
                        {
                            let (_, data) = crate::store::codec::decode_entry(&entry)?;
                            if let Some(hex) =
                                data.as_slice().lines().find_map(|l| l.strip_prefix(b"object "))
                            {
                                id = ObjectId::from_hex(hex)
                                    .map_err(|_| Error::Internal("bad tag object".into()))?;
                            }
                        }
                    }
                }
                deepen_not.push(id);
            }
        }
        // step 1: acks = known haves (unknown dropped). Wants validated inside send_set.
        let acks: Vec<ObjectId> = args
            .haves
            .iter()
            .zip(idx.lookup(&args.haves)?)
            .filter_map(|(h, l)| l.map(|_| *h))
            .collect();
        // step 2: readiness (stateless-RPC) — partial acks without `done` keep the
        // negotiation open; answering early would ship a pack built on an incomplete
        // have-set
        let ready = args.done || args.haves.is_empty() || acks.len() == args.haves.len();
        let mut w = PktWriter::default();
        if !ready {
            wire::write_fetch_prelude(&mut w, &args, &acks, &[], &[], &[])?;
            return Response::from_bytes(w.out).map_err(|e| Error::Internal(e.to_string()));
        }
        // steps 3-5: the send set
        let set = generate::send_set(
            self,
            &bucket,
            &wants,
            &args.haves,
            args.filter.as_ref(),
            args.deepen,
            args.deepen_since,
            &deepen_not,
            args.deepen_relative,
            args.include_tag,
            &args.shallow,
            &mut budget,
        )
        .await?;
        // step 6: stream header + chunks + trailer + flush as band-1 sideband frames
        wire::write_fetch_prelude(&mut w, &args, &acks, &wanted_refs, &set.shallow, &set.unshallow)?;
        let prelude = w.out;
        // contract 406: the harness asserts on the ReqBudget counters — report projected
        // spend (used so far + every planned pack read) before the stream starts
        let projected = budget
            .used
            .saturating_add(u32::try_from(set.reads.len()).unwrap_or(u32::MAX));
        let max_sub = budget.max_subrequests;
        let st = FetchStream { budget, bucket, set, next: 0, hasher: gix_hash::hasher(gix_hash::Kind::Sha1), prelude: Some(prelude), _args: args };
        let s = stream::unfold(st, |mut st| async move {
            match st.step().await {
                Ok(Some(chunk)) => Some((Ok::<Vec<u8>, Error>(chunk), st)),
                Ok(None) => None,
                Err(e) => {
                    // mid-stream: one band-3 ERR frame, then the stream ends (section 10).
                    // `next` must advance past the end or the same step would retry forever
                    st.next = usize::MAX;
                    let mut w = PktWriter::default();
                    wire::Sideband::new(&mut w).error(&format!("ERR {}", e.client_message()));
                    Some((Ok(w.out), st))
                }
            }
        });
        let resp = Response::from_stream(s).map_err(Error::from)?;
        resp.headers()
            .set("x-ge-subrequests", &format!("{projected}/{max_sub}"))
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(resp)
    }
}

struct FetchStream {
    budget: ReqBudget,
    bucket: Bucket,
    set: generate::SendSet,
    next: usize,
    hasher: gix_hash::Hasher,
    prelude: Option<Vec<u8>>,
    _args: FetchArgs,
}
impl FetchStream {
    async fn step(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if let Some(p) = self.prelude.take() {
            return Ok(Some(p));
        }
        let n = self.set.reads.len();
        match self.next {
            0 => {
                // PACK header first (count is exact before the first read: popcount)
                let mut hdr = b"PACK".to_vec();
                hdr.extend_from_slice(&2u32.to_be_bytes());
                hdr.extend_from_slice(&self.set.count().to_be_bytes());
                self.hasher.update(&hdr);
                self.next = 1;
                let mut w = PktWriter::default();
                wire::Sideband::new(&mut w).data(&hdr);
                Ok(Some(w.out))
            }
            i if i <= n => {
                let chunk = generate::pack_chunk(
                    &self.bucket,
                    &self.set,
                    i - 1,
                    &mut self.hasher,
                    &mut self.budget,
                )
                .await?;
                self.next += 1;
                let mut w = PktWriter::default();
                wire::Sideband::new(&mut w).data(&chunk);
                Ok(Some(w.out))
            }
            i if i == n + 1 => {
                // trailer + final flush, once
                let h = std::mem::replace(&mut self.hasher, gix_hash::hasher(gix_hash::Kind::Sha1));
                let trailer = h.try_finalize().map_err(|e| Error::Internal(e.to_string()))?;
                let mut w = PktWriter::default();
                wire::Sideband::new(&mut w).data(trailer.as_slice());
                w.flush();
                self.next += 1;
                Ok(Some(w.out))
            }
            _ => Ok(None),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct LookupDto {
    ids: Vec<String>,
    pack: Option<String>,
}
#[derive(serde::Deserialize)]
pub struct IndexDto {
    pack: PackMetaDto,
    rows: Vec<crate::store::ObjRow>,
}
#[derive(serde::Deserialize)]
struct PackMetaDto {
    id: String,
    push_id: String,
    count: i64,
    bytes: i64,
    commit_lo: i64,
    commit_hi: i64,
}
