//! Edge authentication (CONTRACTS.md 1.1/8): GE_READ_TOKEN, GE_WRITE_TOKEN.
//! Runs before the DO is woken. Anonymous gets a 401 challenge; a read token on a write
//! route gets 403, not a second challenge.

use worker::{Env, Request};

use crate::error::Error;

#[derive(Clone, Copy)]
pub enum Level {
    Read,
    Write,
}

fn secret(env: &Env, name: &str) -> Result<String, Error> {
    env.secret(name).map(|s| s.to_string()).map_err(|_| Error::Internal(format!("{name} unset")))
}
/// Missing optional secret is not an internal error — the compare just fails.
fn secret_opt(env: &Env, name: &str) -> Option<String> {
    env.secret(name).ok().map(|s| s.to_string())
}

/// RFC 7235: the auth scheme is case-insensitive.
fn scheme<'a>(hdr: &'a str, name: &str) -> Option<&'a str> {
    let (s, rest) = hdr.split_at_checked(name.len())?;
    (s.eq_ignore_ascii_case(name) && rest.starts_with(' ')).then(|| &rest[1..])
}

/// Returns the principal string recorded in the reflog.
pub fn authenticate(req: &Request, env: &Env, need: Level) -> Result<String, Error> {
    let hdr = req
        .headers()
        .get("authorization")
        .map_err(|e| Error::Internal(e.to_string()))?
        .ok_or(Error::Auth)?;
    let (token, principal) = if let Some(t) = scheme(&hdr, "Bearer") {
        (t.trim().to_string(), "bearer".to_string())
    } else if let Some(b) = scheme(&hdr, "Basic") {
        // git sends Basic base64(user:token); the token is the password, the user names the principal
        let decoded = b64_decode(b.trim()).ok_or(Error::Auth)?;
        let s = String::from_utf8_lossy(&decoded);
        match s.split_once(':') {
            Some((u, p)) => (p.to_string(), if u.is_empty() { "basic".to_string() } else { u.to_string() }),
            None => (s.to_string(), "basic".to_string()),
        }
    } else {
        return Err(Error::Auth);
    };
    let write = secret(env, "GE_WRITE_TOKEN")?;
    if ct_eq(token.as_bytes(), write.as_bytes()) {
        return Ok(principal);
    }
    let read = secret_opt(env, "GE_READ_TOKEN");
    match need {
        Level::Read => {
            if read.map(|r| ct_eq(token.as_bytes(), r.as_bytes())) == Some(true) {
                Ok(principal)
            } else {
                Err(Error::Auth)
            }
        }
        Level::Write => {
            // a valid read token is forbidden, not unauthenticated: no second challenge
            if read.map(|r| ct_eq(token.as_bytes(), r.as_bytes())) == Some(true) {
                Err(Error::Forbidden)
            } else {
                Err(Error::Auth)
            }
        }
    }
}

/// Constant-time token compare: length mismatch still exits early (token length is not
/// secret — it is observable in the request anyway), content compare never short-circuits.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// RFC 4648, no dependencies (the Workers runtime's `atob` is not reachable from workers-rs
/// without a Reflect call, which belongs to `platform` — a plain table keeps this testable).
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn v(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut it = s.bytes();
    loop {
        let a = match it.next() {
            None => break,
            Some(b) => v(b)?,
        };
        let (b_, c_, d_) = (it.next().map(|x| v(x).unwrap_or(0)), it.next().map(|x| v(x).unwrap_or(0)), it.next().map(|x| v(x).unwrap_or(0)));
        let (b_, c_, d_) = match (b_, c_, d_) {
            (Some(b), c, d) => (b, c, d),
            _ => return None, // a trailing 6-bit group alone is not valid base64
        };
        out.push((a << 2) | (b_ >> 4));
        if let Some(c) = c_ {
            out.push((b_ << 4) | (c >> 2));
            if let Some(d) = d_ {
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}
