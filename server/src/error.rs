use worker::Response;

/// The single error enum (CONTRACTS.md section 10).
#[derive(Debug)]
pub enum Error {
    Protocol(String),
    Auth,
    Forbidden,
    NotFound,
    Conflict(String),
    Unpack(String),
    Budget,
    Limit(String),
    Storage(String),
    Internal(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
            Error::Auth => write!(f, "authentication required"),
            Error::Forbidden => write!(f, "forbidden"),
            Error::NotFound => write!(f, "not found"),
            Error::Conflict(m) => write!(f, "conflict: {m}"),
            Error::Unpack(m) => write!(f, "unpack failed: {m}"),
            Error::Budget => write!(f, "request budget exhausted"),
            Error::Limit(m) => write!(f, "limit exceeded: {m}"),
            Error::Storage(m) => write!(f, "storage error: {m}"),
            Error::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<worker::Error> for Error {
    fn from(e: worker::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

/// Crossing into worker::Error only ever happens at a DO/fetch boundary where the runtime
/// turns it into a 500; Storage/Internal in the DO take this path deliberately (A2 rollback).
impl From<Error> for worker::Error {
    fn from(e: Error) -> Self {
        worker::Error::RustError(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

impl Error {
    /// Rebuild the DO's Error from its JSON error body (wire::http::do_error_response).
    pub async fn from_do_response(mut resp: worker::Response) -> Error {
        #[derive(serde::Deserialize)]
        struct E {
            error: Option<String>,
            message: Option<String>,
        }
        let parsed: Option<E> = resp.json().await.ok();
        match parsed {
            Some(e) => {
                let msg = e.message.unwrap_or_default();
                match e.error.as_deref() {
                    Some("protocol") => Error::Protocol(msg),
                    Some("auth") => Error::Auth,
                    Some("forbidden") => Error::Forbidden,
                    Some("notfound") => Error::NotFound,
                    Some("conflict") => Error::Conflict(msg),
                    Some("unpack") => Error::Unpack(msg),
                    Some("budget") => Error::Budget,
                    Some("limit") => Error::Limit(msg),
                    _ => Error::Storage(msg),
                }
            }
            None => Error::Storage(format!("DO returned {}", resp.status_code())),
        }
    }

    /// Message carried to the client inside `unpack <msg>` / `ERR <msg>`.
    pub fn message(&self) -> String {
        match self {
            Error::Protocol(m) | Error::Unpack(m) | Error::Limit(m) | Error::Storage(m)
            | Error::Internal(m) | Error::Conflict(m) => m.clone(),
            Error::Auth => "authentication required".into(),
            Error::Forbidden => "forbidden".into(),
            Error::NotFound => "not found".into(),
            Error::Budget => "request budget exhausted".into(),
        }
    }

    /// Pre-response mapping (section 10, amended by A2).
    pub fn status(&self) -> u16 {
        match self {
            Error::Protocol(_) => 400,
            Error::Auth => 401,
            Error::Forbidden => 403,
            Error::NotFound => 404,
            Error::Conflict(_) => 409,
            Error::Budget | Error::Limit(_) => 413,
            Error::Unpack(_) | Error::Storage(_) | Error::Internal(_) => 500,
        }
    }
}

/// Map a Result into a worker Response with the section-10 statuses.
/// `git_pkt` wraps the message as one pkt-line `ERR <msg>\n` for git-protocol POSTs.
pub fn respond(r: Result<Response, Error>, git_pkt: bool) -> worker::Result<Response> {
    match r {
        Ok(resp) => Ok(resp),
        Err(e) => {
            let status = e.status();
            let body = if git_pkt {
                let msg = format!("ERR {}\n", e.message());
                let len = msg.len() + 4;
                format!("{len:04x}{msg}")
            } else {
                format!("{}\n", e.message())
            };
            let mut resp = Response::ok(body)?;
            if matches!(e, Error::Auth) {
                resp.headers_mut().set("WWW-Authenticate", "Basic realm=\"git-edge\"").ok();
            }
            Ok(resp.with_status(status))
        }
    }
}
