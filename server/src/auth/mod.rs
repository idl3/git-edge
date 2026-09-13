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

/// Returns the principal string recorded in the reflog.
pub fn authenticate(req: &Request, env: &Env, need: Level) -> Result<String, Error> {
    let hdr = req
        .headers()
        .get("authorization")
        .map_err(|e| Error::Internal(e.to_string()))?
        .ok_or(Error::Auth)?;
    let (token, principal) = if let Some(t) = hdr.strip_prefix("Bearer ") {
        (t.trim().to_string(), "bearer".to_string())
    } else if let Some(b) = hdr.strip_prefix("Basic ") {
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
    if token == write {
        return Ok(principal);
    }
    match need {
        Level::Read => {
            if token == secret(env, "GE_READ_TOKEN")? {
                Ok(principal)
            } else {
                Err(Error::Auth)
            }
        }
        Level::Write => {
            // a valid read token is forbidden, not unauthenticated: no second challenge
            if token == secret(env, "GE_READ_TOKEN")? {
                Err(Error::Forbidden)
            } else {
                Err(Error::Auth)
            }
        }
    }
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
