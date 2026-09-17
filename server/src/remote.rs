//! Client half of protocol v2 — the mirror of wire/mod.rs (WILD: URL import).
//! `ls_refs` advertises a remote's tips; `fetch_open` streams the whole-history
//! pack. Public smart-HTTP only: no auth is sent, so a private remote answers
//! 401/404 and the import job rejects the push rather than burning retries.

use bstr::ByteSlice;
use gix_hash::ObjectId;
use js_sys::Uint8Array;
use worker::{Fetch, Headers, Method, Request, RequestInit, Response};

use crate::error::Error;
use crate::wire::{Pkt, PktReader, PktWriter};

const AGENT: &str = "agent=git-edge/0.1";

/// A remote ref advertisement: `<oid> SP <name>` plus the attrs we keep.
pub struct AdvRef {
    pub name: String,
    pub oid: ObjectId,
    /// `peeled:` attr on annotated tags — the object the tag points at.
    pub peeled: Option<ObjectId>,
}

pub struct Advertised {
    pub refs: Vec<AdvRef>,
    /// HEAD's symref target (e.g. "refs/heads/main"), when the remote says it.
    pub head: Option<String>,
    /// the remote's object-format, parsed off its capability advertisement —
    /// a sha256 source's refs name 64-hex ids and its pack carries a 32-byte
    /// trailer; sha1 when the line is absent (v2's default)
    pub format: gix_hash::Kind,
}

/// `https://host/owner/repo[.git][/]` → the base the git endpoints hang off.
/// http is loopback-only (conformance imports from the same workerd); anywhere
/// else it's plaintext+SSRF — the remote gets our bandwidth, not our secrets,
/// but a public source has no reason to be insecure.
pub fn normalize(url: &str) -> Result<String, Error> {
    let u = url.trim().trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u);
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err(Error::Protocol("import url needs http(s)".into()));
    }
    let rest = &u[u.find("://").map(|i| i + 3).unwrap_or(0)..];
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(Error::Protocol("import url must be a bare repo path".into()));
    }
    if u.starts_with("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or_default();
        if !matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
            return Err(Error::Protocol("http import urls must be loopback".into()));
        }
    }
    Ok(u.to_string())
}

fn headers() -> Result<Headers, Error> {
    let h = Headers::new();
    let set = |k: &str, v: &str| h.set(k, v).map_err(|e| Error::Internal(e.to_string()));
    set("Git-Protocol", "version=2")?;
    set("Accept", "application/x-git-upload-pack-result")?;
    set("Content-Type", "application/x-git-upload-pack-request")?;
    Ok(h)
}

/// A non-2xx answer is deterministic (404 repo, 401 private) — Protocol, so the
/// caller rejects the push instead of letting the job retry a dead remote.
fn ensure_ok(resp: &Response, what: &str) -> Result<(), Error> {
    let code = resp.status_code();
    if !(200..300).contains(&code) {
        return Err(Error::Protocol(format!("{what} answered HTTP {code}")));
    }
    Ok(())
}

async fn send(url: &str, method: Method, body: Option<Vec<u8>>) -> Result<Response, Error> {
    let mut init = RequestInit::new();
    init.with_method(method).with_headers(headers()?);
    if let Some(b) = body {
        init.with_body(Some(Uint8Array::from(b.as_slice()).into()));
    }
    let req = Request::new_with_init(url, &init).map_err(|e| Error::Internal(e.to_string()))?;
    let resp = Fetch::Request(req)
        .send()
        .await
        .map_err(|e| Error::Internal(format!("remote fetch: {e}")))?;
    ensure_ok(&resp, "remote")?;
    Ok(resp)
}

/// GET info/refs (v2 gate) + POST command=ls-refs → advertised refs + HEAD symref.
pub async fn ls_refs(raw_url: &str) -> Result<Advertised, Error> {
    let base = normalize(raw_url)?;
    // capability advertisement: `version 2`, `agent=`, `ls-refs`, `fetch=…` —
    // a dumb-HTTP or v0-only endpoint lacks both commands we need.
    let mut resp = send(&format!("{base}/info/refs?service=git-upload-pack"), Method::Get, None).await?;
    let caps = resp.bytes().await.map_err(|e| Error::Internal(format!("info/refs body: {e}")))?;
    let mut r = PktReader::default();
    r.push(&caps);
    let (mut ls, mut fetch) = (false, false);
    let mut format = None;
    while let Some(pkt) = r.next()? {
        if let Pkt::Data(d) = pkt {
            ls |= d.starts_with(b"ls-refs");
            fetch |= d.starts_with(b"fetch");
            if format.is_none() {
                format = d
                    .strip_suffix(b"\n")
                    .unwrap_or(d)
                    .strip_prefix(b"object-format=")
                    .map(|f| match f {
                        b"sha256" => Ok(gix_hash::Kind::Sha256),
                        b"sha1" => Ok(gix_hash::Kind::Sha1),
                        other => Err(Error::Protocol(format!(
                            "remote object-format {} unsupported",
                            other.as_bstr()
                        ))),
                    })
                    .transpose()?;
            }
        }
    }
    if !(ls && fetch) {
        return Err(Error::Protocol("remote lacks protocol v2 ls-refs/fetch".into()));
    }
    let format = format.unwrap_or(gix_hash::Kind::Sha1);
    let fmt_name = if format == gix_hash::Kind::Sha256 { "sha256" } else { "sha1" };
    let mut w = PktWriter::default();
    w.text("command=ls-refs")?;
    w.text(AGENT)?;
    w.text(&format!("object-format={fmt_name}"))?;
    w.delim();
    w.text("peel")?;
    w.text("symrefs")?;
    w.flush();
    let mut resp = send(&format!("{base}/git-upload-pack"), Method::Post, Some(w.out)).await?;
    let body = resp.bytes().await.map_err(|e| Error::Internal(format!("ls-refs body: {e}")))?;
    let mut r = PktReader::default();
    r.push(&body);
    let mut adv = Advertised { refs: Vec::new(), head: None, format };
    while let Some(pkt) = r.next()? {
        let Pkt::Data(d) = pkt else { continue };
        let line = d.strip_suffix(b"\n").unwrap_or(d).as_bstr();
        // `<oid> SP <name>[ SP attr]*` — HEAD carries symref-target and no peeled
        let mut it = line.split_str(b" ");
        let Some(oid_hex) = it.next() else { continue };
        let Some(name) = it.next() else { continue };
        let name = name.to_str_lossy().into_owned();
        let mut head = None;
        let mut peeled = None;
        for a in it {
            if let Some(t) = a.strip_prefix(b"symref-target:") {
                head = Some(t.to_str_lossy().into_owned());
            } else if let Some(p) = a.strip_prefix(b"peeled:") {
                peeled = ObjectId::from_hex(p).ok();
            }
        }
        if name == "HEAD" {
            adv.head = head;
            continue;
        }
        if !name.starts_with("refs/") {
            continue;
        }
        let Ok(oid) = ObjectId::from_hex(oid_hex.as_bytes()) else { continue };
        adv.refs.push(AdvRef { name, oid, peeled });
    }
    Ok(adv)
}

/// POST command=fetch for `wants` (no haves — a URL import is a full clone).
/// `kind` is the remote's advertised object-format — the pack it returns carries
/// that digest's trailer width. The returned Response's body is the live stream
/// — it must be consumed inside this invocation; there is no resume across slices.
pub async fn fetch_open(raw_url: &str, wants: &[ObjectId], kind: gix_hash::Kind) -> Result<Response, Error> {
    let base = normalize(raw_url)?;
    let fmt_name = if kind == gix_hash::Kind::Sha256 { "sha256" } else { "sha1" };
    let mut w = PktWriter::default();
    w.text("command=fetch")?;
    w.text(AGENT)?;
    w.text(&format!("object-format={fmt_name}"))?;
    w.delim();
    // git's minimal clone request: wants + done. Servers answer the pack as
    // band-1 sideband either way (github defaults so; we always emit it), so
    // no sideband-all/ofs-delta args — our own parser would 400 on them anyway
    for o in wants {
        w.text(&format!("want {o}"))?;
    }
    w.text("done")?;
    w.flush();
    send(&format!("{base}/git-upload-pack"), Method::Post, Some(w.out)).await
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalize_rules() {
        // canonical https forms
        assert_eq!(normalize("https://github.com/o/r").unwrap(), "https://github.com/o/r");
        assert_eq!(normalize("https://github.com/o/r.git").unwrap(), "https://github.com/o/r");
        assert_eq!(normalize("https://github.com/o/r.git/").unwrap(), "https://github.com/o/r");
        assert_eq!(normalize("https://github.com/o/r/").unwrap(), "https://github.com/o/r");
        // loopback http is allowed (conformance), remote http is not
        assert_eq!(normalize("http://localhost:8794/o/r").unwrap(), "http://localhost:8794/o/r");
        assert!(normalize("http://127.0.0.1:8794/o/r").is_ok());
        assert!(normalize("http://example.com/o/r").is_err());
        assert!(normalize("http://169.254.169.254/o/r").is_err());
        // not http(s), userinfo, query, fragment all refuse
        assert!(normalize("ftp://github.com/o/r").is_err());
        assert!(normalize("git@github.com:o/r").is_err());
        assert!(normalize("https://user:pw@github.com/o/r").is_err());
        assert!(normalize("https://github.com/o/r?x=1").is_err());
        assert!(normalize("https://github.com/o/r#main").is_err());
    }
}
