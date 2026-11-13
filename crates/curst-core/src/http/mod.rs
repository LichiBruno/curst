//! sending requests: the [`HttpExecutor`] trait, its libcurl implementation
//! and the response model

mod curl_executor;
mod response;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use curl_executor::CurlExecutor;
pub use response::{BodyKind, Response, ResponseBody, StoppedRedirect, Timings};

use crate::model::Request;
use crate::url::Origin;

/// which redirects may be followed
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RedirectRule {
    /// any, as [`Config::follow_redirects`] allows
    #[default]
    Any,

    /// only those that stay within these origins; a request that carries
    /// secrets may go only where every one of them is approved
    Within(Vec<Origin>),
}

impl RedirectRule {
    /// whether a redirect to `url` may be followed
    #[must_use]
    pub fn allows(&self, url: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Within(origins) => {
                crate::url::origin(url).is_some_and(|origin| origins.contains(&origin))
            }
        }
    }
}

/// sends a request and waits for the whole response
/// implementations block, so callers run them on a worker thread
pub trait HttpExecutor: Send + Sync {
    /// sends `request`, stopping early when `cancel` is triggered; a
    /// redirect `redirects` does not allow is not followed, and the
    /// response says where it pointed
    ///
    /// # Errors
    /// any [`HttpError`], including [`HttpError::Cancelled`]
    fn execute(
        &self,
        request: &Request,
        cancel: &Cancel,
        redirects: &RedirectRule,
    ) -> Result<Response, HttpError>;
}

/// a handle that stops a request in flight, cheap to clone and share
/// between threads
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// a handle that has not been triggered
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// stops the request this handle was given to
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// whether [`Cancel::cancel`] was called
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// transfer settings shared by every request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// limit for the whole transfer, `None` for no limit
    pub timeout: Option<Duration>,

    /// limit for establishing the connection
    pub connect_timeout: Duration,

    /// whether redirects are followed
    pub follow_redirects: bool,

    /// how many redirects are followed before giving up
    pub max_redirects: u32,

    /// how many body bytes are kept in memory; the rest goes to a
    /// temporary file
    pub body_cap: usize,

    /// skips the certificate and host name checks, like `curl -k`
    pub insecure: bool,

    /// a PEM bundle trusted instead of the system certificates, like
    /// `curl --cacert`
    pub ca_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
            connect_timeout: Duration::from_secs(10),
            follow_redirects: true,
            max_redirects: 10,
            body_cap: 10 * 1024 * 1024,
            insecure: false,
            ca_file: None,
        }
    }
}

/// why a request failed
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// the URL is malformed or uses an unsupported scheme
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// the host name could not be resolved
    #[error("could not resolve host: {0}")]
    Resolve(String),

    /// the connection could not be established
    #[error("could not connect: {0}")]
    Connect(String),

    /// the TLS handshake or certificate check failed
    #[error("TLS error: {0}")]
    Tls(String),

    /// the transfer took longer than the configured timeout
    #[error("timed out: {0}")]
    Timeout(String),

    /// more redirects than [`Config::max_redirects`]
    #[error("too many redirects")]
    TooManyRedirects,

    /// the request was cancelled through its [`Cancel`] handle
    #[error("cancelled")]
    Cancelled,

    /// the body could not be stored
    #[error("could not store the response body: {0}")]
    Io(#[from] std::io::Error),

    /// any other transfer failure
    #[error("{0}")]
    Transfer(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_shared_between_clones() {
        let cancel = Cancel::new();
        let other = cancel.clone();

        assert!(!other.is_cancelled());

        cancel.cancel();

        assert!(other.is_cancelled());
    }

    #[test]
    fn redirect_rules() {
        let within = RedirectRule::Within(vec![crate::url::origin("https://a.example").unwrap()]);

        assert!(RedirectRule::Any.allows("http://anything/"));
        assert!(RedirectRule::default().allows("relative"));
        assert!(within.allows("https://A.example:443/x"));
        assert!(!within.allows("http://a.example/x"));
        assert!(!within.allows("https://a.example:8443/x"));
        assert!(!within.allows("https://b.example/x"));
        assert!(!within.allows("not a url"));
    }

    #[test]
    fn default_config() {
        let config = Config::default();

        assert_eq!(config.body_cap, 10 * 1024 * 1024);
        assert!(config.follow_redirects);
        assert!(!config.insecure);
        assert_eq!(config.ca_file, None);
    }
}
