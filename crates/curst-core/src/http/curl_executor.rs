use std::io;

use curl::easy::{Easy2, Handler, HttpVersion, List, SslOpt, WriteError};

use super::{
    Cancel, Config, HttpError, HttpExecutor, RedirectRule, Response, ResponseBody, StoppedRedirect,
    Timings,
};
use crate::model::{Auth, KeyPlace, KeyValue, Method, Request};
use crate::url;

const USER_AGENT: &str = concat!("curst/", env!("CARGO_PKG_VERSION"));

/// an [`HttpExecutor`] backed by libcurl
/// redirects are followed here rather than by libcurl: with a custom method
/// such as `PUT`, libcurl repeats that method after a 303, and every hop has
/// to be visible to decide where credentials may go
#[derive(Debug, Clone, Default)]
pub struct CurlExecutor {
    config: Config,
}

impl CurlExecutor {
    /// an executor with the given settings
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

/// one request on the way through a chain of redirects
struct Hop {
    method: Method,
    url: String,
    body: Option<Vec<u8>>,
    with_credentials: bool,
}

impl HttpExecutor for CurlExecutor {
    fn execute(
        &self,
        request: &Request,
        cancel: &Cancel,
        rule: &RedirectRule,
    ) -> Result<Response, HttpError> {
        let mut easy = Easy2::new(Collector::new(cancel.clone(), self.config.body_cap));
        let start = url::origin(&request.full_url());
        let mut hop = Hop {
            method: request.method,
            url: first_url(request),
            body: request.body.encode(),
            with_credentials: true,
        };
        let mut timings = Timings::default();
        let mut redirects = 0;

        loop {
            let mut response = perform(&mut easy, request, &hop, &self.config, cancel)?;

            timings = add(timings, response.timings);

            let location = response
                .header("location")
                .filter(|_| self.config.follow_redirects)
                .and_then(|location| url::resolve(&hop.url, location));
            let Some(next) = location.filter(|_| is_redirect(response.status)) else {
                response.timings = timings;
                response.redirects = redirects;

                return Ok(response);
            };

            let to_get = response.status == 303 && hop.method != Method::Head
                || matches!(response.status, 301 | 302) && hop.method == Method::Post;

            if !rule.allows(&next) {
                response.timings = timings;
                response.redirects = redirects;
                response.stopped = Some(StoppedRedirect {
                    url: next,
                    method: if to_get { Method::Get } else { hop.method },
                    keeps_body: !to_get && hop.body.is_some(),
                });

                return Ok(response);
            }

            if redirects == self.config.max_redirects {
                return Err(HttpError::TooManyRedirects);
            }

            redirects += 1;

            if to_get {
                hop.method = Method::Get;
                hop.body = None;
            }

            hop.with_credentials = hop.with_credentials && url::origin(&next) == start;
            hop.url = next;
        }
    }
}

/// the URL the request starts at: an API key that goes in the query is
/// added there, and only there, never to where a redirect points
fn first_url(request: &Request) -> String {
    match &request.auth {
        Auth::ApiKey {
            name,
            value,
            place: KeyPlace::Query,
        } if !name.is_empty() => {
            let mut keyed = request.clone();

            keyed
                .params
                .push(KeyValue::new(name.clone(), value.clone()));
            keyed.full_url()
        }
        _ => request.full_url(),
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn add(a: Timings, b: Timings) -> Timings {
    Timings {
        dns: a.dns + b.dns,
        connect: a.connect + b.connect,
        tls: a.tls + b.tls,
        first_byte: a.first_byte + b.first_byte,
        total: a.total + b.total,
    }
}

fn perform(
    easy: &mut Easy2<Collector>,
    request: &Request,
    hop: &Hop,
    config: &Config,
    cancel: &Cancel,
) -> Result<Response, HttpError> {
    easy.reset();
    easy.get_mut().clear();

    configure(easy, request, hop, config).map_err(|e| map_error(&e))?;

    let outcome = easy.perform();

    if let Some(err) = easy.get_mut().io_error.take() {
        return Err(HttpError::Io(err));
    }

    if let Err(err) = outcome {
        return Err(if cancel.is_canceled() {
            HttpError::Canceled
        } else {
            map_error(&err)
        });
    }

    finish(easy, &hop.url).map_err(|e| map_error(&e))
}

fn configure(
    easy: &mut Easy2<Collector>,
    request: &Request,
    hop: &Hop,
    config: &Config,
) -> Result<(), curl::Error> {
    easy.url(&hop.url)?;
    easy.http_version(HttpVersion::V2TLS)?;
    easy.progress(true)?;
    easy.connect_timeout(config.connect_timeout)?;

    // an empty list asks for every encoding libcurl can decode
    easy.accept_encoding("")?;

    // libcurl ignores a CA file while the system store is in use
    easy.ssl_options(SslOpt::new().native_ca(config.ca_file.is_none()))?;

    if let Some(ca_file) = &config.ca_file {
        easy.cainfo(ca_file)?;
    }

    if config.insecure {
        easy.ssl_verify_peer(false)?;
        easy.ssl_verify_host(false)?;
    }

    if let Some(timeout) = config.timeout {
        easy.timeout(timeout)?;
    }

    match (hop.method, &hop.body) {
        (Method::Get, None) => {}
        (Method::Head, _) => easy.nobody(true)?,
        (Method::Post, _) => easy.post(true)?,
        (method, _) => easy.custom_request(method.as_str())?,
    }

    match (&hop.body, hop.method) {
        (_, Method::Head) => {}
        (Some(bytes), _) => easy.post_fields_copy(bytes)?,
        // without fields libcurl sends a POST chunked, waiting for a body
        (None, Method::Post) => easy.post_fields_copy(&[])?,
        (None, _) => {}
    }

    let credential = |name: &str| {
        ["authorization", "cookie", "proxy-authorization"]
            .iter()
            .any(|c| name.eq_ignore_ascii_case(c))
    };
    let mut headers = List::new();

    for header in request.headers.iter().filter(|h| {
        h.enabled && !h.name.is_empty() && (hop.with_credentials || !credential(&h.name))
    }) {
        if header.value.is_empty() {
            headers.append(&format!("{};", header.name))?;
        } else {
            headers.append(&format!("{}: {}", header.name, header.value))?;
        }
    }

    let has = |name: &str| request.header(name).next().is_some();
    let body_type = hop.body.as_ref().and(request.body.content_type());

    match body_type {
        _ if has("content-type") => {}
        Some(content_type) => headers.append(&format!("Content-Type: {content_type}"))?,
        // libcurl labels every POST as a form unless told otherwise
        None if hop.method == Method::Post => headers.append("Content-Type:")?,
        None => {}
    }

    if !has("user-agent") {
        headers.append(&format!("User-Agent: {USER_AGENT}"))?;
    }

    if !has("expect") {
        headers.append("Expect:")?;
    }

    match &request.auth {
        _ if !hop.with_credentials => {}
        Auth::Inherit
        | Auth::None
        | Auth::ApiKey {
            place: KeyPlace::Query,
            ..
        } => {}
        Auth::Basic { username, password } => {
            let mut auth = curl::easy::Auth::new();

            auth.basic(true);
            easy.http_auth(&auth)?;
            easy.username(username)?;
            easy.password(password)?;
        }
        Auth::Bearer { token } => {
            if !has("authorization") {
                headers.append(&format!("Authorization: Bearer {token}"))?;
            }
        }
        Auth::ApiKey {
            name,
            value,
            place: KeyPlace::Header,
        } => {
            if !name.is_empty() && !has(name) {
                headers.append(&format!("{name}: {value}"))?;
            }
        }
    }

    easy.http_headers(headers)
}

fn finish(easy: &mut Easy2<Collector>, url: &str) -> Result<Response, curl::Error> {
    let timings = Timings {
        dns: easy.namelookup_time()?,
        connect: easy.connect_time()?,
        tls: easy.appconnect_time()?,
        first_byte: easy.starttransfer_time()?,
        total: easy.total_time()?,
    };
    let status = u16::try_from(easy.response_code()?).unwrap_or_default();
    let collector = easy.get_mut();

    Ok(Response {
        status,
        status_line: std::mem::take(&mut collector.status_line),
        headers: std::mem::take(&mut collector.headers),
        body: std::mem::replace(&mut collector.body, ResponseBody::new(0)),
        timings,
        effective_url: url.to_owned(),
        redirects: 0,
        stopped: None,
    })
}

fn map_error(err: &curl::Error) -> HttpError {
    let detail = err
        .extra_description()
        .map_or_else(|| err.description().to_owned(), ToOwned::to_owned);

    if err.is_url_malformed() || err.is_unsupported_protocol() {
        HttpError::InvalidUrl(detail)
    } else if err.is_couldnt_resolve_host() || err.is_couldnt_resolve_proxy() {
        HttpError::Resolve(detail)
    } else if err.is_couldnt_connect() {
        HttpError::Connect(detail)
    } else if err.is_operation_timedout() {
        HttpError::Timeout(detail)
    } else if err.is_too_many_redirects() {
        HttpError::TooManyRedirects
    } else if err.is_ssl_connect_error()
        || err.is_peer_failed_verification()
        || err.is_ssl_certproblem()
        || err.is_ssl_cacert_badfile()
    {
        HttpError::Tls(detail)
    } else {
        HttpError::Transfer(detail)
    }
}

struct Collector {
    cancel: Cancel,
    cap: usize,
    status_line: String,
    headers: Vec<(String, String)>,
    body: ResponseBody,
    io_error: Option<io::Error>,
}

impl Collector {
    fn new(cancel: Cancel, cap: usize) -> Self {
        Self {
            cancel,
            cap,
            status_line: String::new(),
            headers: Vec::new(),
            body: ResponseBody::new(cap),
            io_error: None,
        }
    }

    fn clear(&mut self) {
        self.status_line.clear();
        self.headers.clear();
        self.body = ResponseBody::new(self.cap);
        self.io_error = None;
    }
}

impl Handler for Collector {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        match self.body.push(data) {
            Ok(()) => Ok(data.len()),
            Err(err) => {
                self.io_error = Some(err);

                Ok(0)
            }
        }
    }

    // libcurl passes the headers of every response, including 1xx and
    // redirect hops, so a new status line starts the list over
    fn header(&mut self, data: &[u8]) -> bool {
        let line = String::from_utf8_lossy(data);
        let line = line.trim_end_matches(['\r', '\n']);

        if line.starts_with("HTTP/") {
            line.clone_into(&mut self.status_line);
            self.headers.clear();
        } else if let Some((name, value)) = line.split_once(':') {
            self.headers
                .push((name.trim().to_owned(), value.trim().to_owned()));
        }

        true
    }

    fn progress(&mut self, _dltotal: f64, _dlnow: f64, _ultotal: f64, _ulnow: f64) -> bool {
        !self.cancel.is_canceled()
    }
}
