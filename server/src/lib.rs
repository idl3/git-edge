//! git-edge: a complete git smart-HTTP server on a Cloudflare Worker + one Durable Object
//! per repo + R2 pack storage. CONTRACTS.md is the binding spec.

pub mod auth;
pub mod edge;
pub mod error;
pub mod jobs;
pub mod pack;
pub mod platform;
pub mod repo_do;
pub mod store;
pub mod wire;

use error::Error;

/// Per-request budget (CONTRACTS.md 7). Every Bucket and stub call charges first.
pub struct ReqBudget {
    pub max_subrequests: u32,
    pub used: u32,
    pub started_ms: f64,
    pub max_ms: f64,
}
impl ReqBudget {
    /// Paid-plan request budget (7.1): 9,000 subrequests, 240 s wall clock.
    pub fn paid() -> Self {
        Self {
            max_subrequests: 9_000,
            used: 0,
            started_ms: js_sys::Date::now(),
            max_ms: 240_000.0,
        }
    }
    pub fn charge(&mut self, n: u32) -> Result<(), Error> {
        self.used = self.used.saturating_add(n);
        if self.used > self.max_subrequests || js_sys::Date::now() - self.started_ms > self.max_ms {
            return Err(Error::Budget);
        }
        Ok(())
    }
}

#[worker::event(fetch)]
pub async fn fetch(
    req: worker::Request,
    env: worker::Env,
    _ctx: worker::Context,
) -> worker::Result<worker::Response> {
    edge::fetch(req, env).await
}
