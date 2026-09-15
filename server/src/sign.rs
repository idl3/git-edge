//! Signed pack URIs (CONTRACTS.md A30): a `packfile-uris` URI is fetched by the
//! client's `git http-fetch` with no auth headers, so the URL itself carries the
//! capability — an HMAC-SHA256 over repo + pack + expiry.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use worker::Env;

use crate::error::Error;

type HmacSha256 = Hmac<Sha256>;

/// `v1` domain-separates the scheme; \n can't appear in a seg_ok route piece.
fn msg(repo: &str, pack: &str, exp: i64) -> Vec<u8> {
    format!("v1\n{repo}\n{pack}\n{exp}").into_bytes()
}

fn mac(key: &str, msg: &[u8]) -> Result<HmacSha256, Error> {
    let mut m =
        HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| Error::Internal("bad signing key".into()))?;
    m.update(msg);
    Ok(m)
}

fn hex(mac: &[u8]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `s=` query value for `GET /<repo>/_packs/<pack>.pack?e=<exp>&s=<sig>`.
pub fn pack_sig(key: &str, repo: &str, pack: &str, exp: i64) -> Result<String, Error> {
    Ok(hex(&mac(key, &msg(repo, pack, exp))?.finalize().into_bytes()))
}

/// Constant-time check; expiry is the caller's concern (it reports its own error).
pub fn pack_sig_ok(key: &str, repo: &str, pack: &str, exp: i64, sig: &str) -> bool {
    if sig.len() != 64 {
        return false;
    }
    let Ok(sig) = sig
        .as_bytes()
        .chunks_exact(2)
        .map(|c| std::str::from_utf8(c).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .ok_or(())
    else {
        return false;
    };
    mac(key, &msg(repo, pack, exp))
        .map(|m| m.verify_slice(&sig).is_ok())
        .unwrap_or(false)
}

/// The feature switch: GE_URL_SIGNING_KEY unset or weak (<32 bytes) disables C1 —
/// never minted, never served. Configure via `wrangler secret put`, not [vars].
pub fn signing_key(env: &Env) -> Option<String> {
    env.var("GE_URL_SIGNING_KEY")
        .ok()
        .map(|v| v.to_string())
        .filter(|k| k.len() >= 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn round_trip() {
        let s = pack_sig(K, "rid", "pid", 123).unwrap();
        assert_eq!(s.len(), 64);
        assert!(pack_sig_ok(K, "rid", "pid", 123, &s));
    }

    #[test]
    fn rejects() {
        let s = pack_sig(K, "rid", "pid", 123).unwrap();
        for (r, p, e) in [("other", "pid", 123), ("rid", "other", 123), ("rid", "pid", 124)] {
            assert!(!pack_sig_ok(K, r, p, e, &s), "{r} {p} {e}");
        }
        assert!(!pack_sig_ok("wrong-key-wrong-key-wrong-key-wrong", "rid", "pid", 123, &s));
        assert!(!pack_sig_ok(K, "rid", "pid", 123, "not-hex"));
        assert!(!pack_sig_ok(K, "rid", "pid", 123, &s[..62]));
        // exactly 64 bytes with a multibyte char at an odd offset — the old
        // str-slicing decode panicked mid-char; the bytes-level decode must not
        let bad = format!("{}é{}", "a".repeat(61), "c");
        assert_eq!(bad.len(), 64);
        assert!(!pack_sig_ok(K, "rid", "pid", 123, &bad));
    }
}
