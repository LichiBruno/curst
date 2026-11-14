//! curl commands: a pasted one becomes a request, and a request can be
//! copied as one
//!
//! a command is only read, never run: options that would read a file,
//! such as `-d @body.json`, are reported and left out

mod shell;

#[cfg(test)]
mod tests;

pub use shell::{ShellError, quote, split};

use crate::model::{Auth, Body, KeyValue, Method, Request, percent_encode};
use crate::secrets::Secret;
use crate::secrets::detect::{self, Place};
use crate::vars::VarName;

/// why a command could not be read at all
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurlError {
    /// the quoting is broken
    #[error("{0}")]
    Shell(#[from] ShellError),

    /// the text does not start with `curl`
    #[error("this is not a curl command")]
    NotCurl,

    /// there is no URL in it
    #[error("the command has no URL")]
    NoUrl,

    /// an option needs a value and the command ends before it
    #[error("{0} needs a value")]
    MissingValue(String),
}

/// a request read from a curl command, and what in the command was left out
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// the request
    pub request: Request,

    /// options that were not used, each with why
    pub warnings: Vec<String>,
}

/// options that take a value and change nothing curst sends
const IGNORED_WITH_VALUE: [&str; 12] = [
    "-o",
    "--output",
    "-m",
    "--max-time",
    "--connect-timeout",
    "-w",
    "--write-out",
    "--retry",
    "--retry-delay",
    "--retry-max-time",
    "--limit-rate",
    "--max-redirs",
];

/// options without a value that change nothing curst sends
const IGNORED_FLAGS: [&str; 22] = [
    "-s",
    "--silent",
    "-S",
    "--show-error",
    "-v",
    "--verbose",
    "-i",
    "--include",
    "-L",
    "--location",
    "--compressed",
    "-f",
    "--fail",
    "--fail-with-body",
    "-g",
    "--globoff",
    "--http1.1",
    "--http2",
    "--http2-prior-knowledge",
    "--no-progress-meter",
    "--path-as-is",
    "-#",
];

/// options that take a value
const WITH_VALUE: [&str; 33] = [
    "-X",
    "--request",
    "-H",
    "--header",
    "-d",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-ascii",
    "--data-urlencode",
    "--json",
    "-F",
    "--form",
    "--form-string",
    "-u",
    "--user",
    "-b",
    "--cookie",
    "-A",
    "--user-agent",
    "-e",
    "--referer",
    "--url",
    "--oauth2-bearer",
    "-x",
    "--proxy",
    "-K",
    "--config",
    "-T",
    "--upload-file",
    "--cacert",
    "-E",
    "--cert",
];

/// short options with a value, which may be written attached: `-XPOST`
const SHORT_WITH_VALUE: &str = "XHdFubAexKTEomw";

fn takes_value(option: &str) -> bool {
    WITH_VALUE.contains(&option) || IGNORED_WITH_VALUE.contains(&option)
}

/// the words of a command, with options split from attached values:
/// `--header=a`, `-XPOST` and `-sSL` become what they stand for
fn options(words: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut value_next = false;

    for word in words {
        // an option's value is taken as written, even when it starts with -
        if std::mem::take(&mut value_next) {
            out.push(word);

            continue;
        }

        if let Some(rest) = word.strip_prefix("--") {
            match rest.split_once('=') {
                Some((name, value)) if !name.is_empty() => {
                    out.push(format!("--{name}"));
                    out.push(value.to_owned());
                }
                _ => {
                    value_next = takes_value(&word);
                    out.push(word);
                }
            }

            continue;
        }

        let short = word.len() > 2 && word.starts_with('-') && word.is_char_boundary(2);

        if !short {
            value_next = takes_value(&word);
            out.push(word);

            continue;
        }

        // bundled flags, the last of which may take the rest as its value
        let letters: Vec<char> = word[1..].chars().collect();

        for (index, letter) in letters.iter().enumerate() {
            out.push(format!("-{letter}"));

            if SHORT_WITH_VALUE.contains(*letter) {
                let rest: String = letters[index + 1..].iter().collect();

                if rest.is_empty() {
                    value_next = true;
                } else {
                    out.push(rest);
                }

                break;
            }
        }
    }

    out
}

/// what the options said, before it is made into a request
#[derive(Default)]
struct Reading {
    method: Option<Method>,
    url: Option<String>,
    headers: Vec<KeyValue>,
    data: Vec<String>,
    json: Option<String>,
    get: bool,
    head: bool,
    user: Option<String>,
    bearer: Option<String>,
    warnings: Vec<String>,
}

impl Reading {
    fn header(&mut self, name: &str, value: &str) {
        self.headers.push(KeyValue::new(name, value));
    }

    fn option(&mut self, name: &str, value: String) {
        match name {
            "-X" | "--request" => match value.parse::<Method>() {
                Ok(method) => self.method = Some(method),
                Err(_) => self
                    .warnings
                    .push(format!("the method {value} is not one curst sends")),
            },
            "-H" | "--header" => match value.split_once(':') {
                Some((name, value)) => self.header(name.trim(), value.trim()),
                // `Name;` is curl's way to send a header with no value
                None => match value.strip_suffix(';') {
                    Some(name) => self.header(name.trim(), ""),
                    None => self.warnings.push(format!("{value:?} is not a header")),
                },
            },
            "-d" | "--data" | "--data-binary" | "--data-ascii" => {
                if value.starts_with('@') {
                    self.warnings
                        .push(format!("{name} {value}: curst does not read files"));
                } else {
                    self.data.push(value);
                }
            }
            "--data-raw" => self.data.push(value),
            "--data-urlencode" => self.data_urlencode(&value),
            "--json" => {
                if value.starts_with('@') {
                    self.warnings
                        .push(format!("--json {value}: curst does not read files"));
                } else {
                    self.json.get_or_insert_with(String::new).push_str(&value);
                }
            }
            "-u" | "--user" => self.user = Some(value),
            "--oauth2-bearer" => self.bearer = Some(value),
            "-b" | "--cookie" => {
                if value.contains('=') {
                    self.header("Cookie", &value);
                } else {
                    self.warnings
                        .push(format!("{name} {value}: curst does not read cookie files"));
                }
            }
            "-A" | "--user-agent" => self.header("User-Agent", &value),
            "-e" | "--referer" => self.header("Referer", &value),
            "--url" => self.url = Some(value),
            "-F" | "--form" | "--form-string" => self.warnings.push(format!(
                "{name} {value}: multipart forms are not supported yet"
            )),
            "-x" | "--proxy" => self
                .warnings
                .push(format!("{name}: proxies are not supported")),
            "-K" | "--config" | "-T" | "--upload-file" => {
                self.warnings
                    .push(format!("{name} {value}: curst does not read files"));
            }
            _ => self.warnings.push(format!(
                "{name}: certificates are set when curst starts, with --cacert"
            )),
        }
    }

    /// `--data-urlencode`: `name=content`, `=content` or `content`, the
    /// content encoded
    fn data_urlencode(&mut self, value: &str) {
        if value.starts_with('@') || value.split_once('=').is_none() && value.contains('@') {
            self.warnings.push(format!(
                "--data-urlencode {value}: curst does not read files"
            ));

            return;
        }

        self.data.push(match value.split_once('=') {
            Some(("", content)) => percent_encode(content),
            Some((name, content)) => format!("{name}={}", percent_encode(content)),
            None => percent_encode(value),
        });
    }

    fn flag(&mut self, name: &str) {
        match name {
            "-G" | "--get" => self.get = true,
            "-I" | "--head" => self.head = true,
            "-k" | "--insecure" => self
                .warnings
                .push("-k: TLS checks stay on; start curst with -k to skip them".to_owned()),
            name if IGNORED_FLAGS.contains(&name) => {}
            name => self.warnings.push(format!("{name} is not supported")),
        }
    }
}

/// percent-decodes `text`, with `+` as a space, as form data is written
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        let hex = || {
            let digits = text.get(index + 1..index + 3)?;

            u8::from_str_radix(digits, 16).ok()
        };

        match byte {
            b'%' => match hex() {
                Some(decoded) => {
                    out.push(decoded);
                    index += 2;
                }
                None => out.push(byte),
            },
            b'+' => out.push(b' '),
            _ => out.push(byte),
        }

        index += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// `a=1&b=2` as pairs, or `None` when it does not look like form data
fn form_pairs(text: &str) -> Option<Vec<KeyValue>> {
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=')?;
            let plain = name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~%+[]".contains(&b));

            (plain && !name.is_empty()).then(|| KeyValue::new(decode(name), decode(value)))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|pairs| !pairs.is_empty())
}

fn looks_like_json(text: &str) -> bool {
    let text = text.trim_start();

    (text.starts_with('{') || text.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
}

/// splits a URL into the part before its query, its query as pairs, and
/// the user and password written in it
fn split_url(url: &str) -> (String, Vec<KeyValue>, Option<(String, String)>) {
    // a fragment is never sent
    let url = url.split('#').next().unwrap_or_default();
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let params = query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((name, value)) => KeyValue::new(decode(name), decode(value)),
            None => KeyValue::new(decode(part), ""),
        })
        .collect();
    let (scheme, rest) = base.split_once("://").unwrap_or(("", base));
    let host_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(host_end);

    match authority.rsplit_once('@') {
        Some((info, host)) => {
            let (user, password) = info.split_once(':').unwrap_or((info, ""));
            let prefix = if scheme.is_empty() {
                String::new()
            } else {
                format!("{scheme}://")
            };

            (
                format!("{prefix}{host}{path}"),
                params,
                Some((decode(user), decode(password))),
            )
        }
        None => (base.to_owned(), params, None),
    }
}

/// the request a curl command sends
///
/// # Errors
/// when the quoting is broken, it is not a curl command, it has no URL, or
/// an option lacks its value
pub fn parse(command: &str) -> Result<Parsed, CurlError> {
    let mut words = options(split(command)?).into_iter();
    let program = words.next().ok_or(CurlError::NotCurl)?;
    let name = program.rsplit(['/', '\\']).next().unwrap_or_default();

    if name != "curl" && name != "curl.exe" {
        return Err(CurlError::NotCurl);
    }

    let mut reading = Reading::default();

    while let Some(word) = words.next() {
        if word == "--" {
            if let Some(url) = words.next() {
                reading.url.get_or_insert(url);
            }

            continue;
        }

        if !word.starts_with('-') || word == "-" {
            if reading.url.is_some() {
                reading
                    .warnings
                    .push(format!("only one URL is used, not {word}"));
            } else {
                reading.url = Some(word);
            }

            continue;
        }

        if !takes_value(&word) {
            reading.flag(&word);

            continue;
        }

        let Some(value) = words.next() else {
            return Err(CurlError::MissingValue(word));
        };

        if !IGNORED_WITH_VALUE.contains(&word.as_str()) {
            reading.option(&word, value);
        }
    }

    finish(reading)
}

/// the body the data makes, its kind picked from its declared content
/// type, and from the data itself when there is none
fn body(json: Option<String>, data: Option<String>, declared: Option<&str>) -> Body {
    let data = match (json, data) {
        (Some(json), data) => {
            return Body::Json(data.map_or_else(|| json.clone(), |data| format!("{data}{json}")));
        }
        (None, Some(data)) => data,
        (None, None) => return Body::None,
    };
    let lower = declared.map(str::to_ascii_lowercase);
    let text = |content_type: &str, data: String| Body::Text {
        content_type: content_type.to_owned(),
        text: data,
    };

    match (declared, lower.as_deref()) {
        (Some(_), Some(t)) if t.starts_with("application/json") => Body::Json(data),
        (Some(declared), Some(t)) if t.starts_with("application/x-www-form-urlencoded") => {
            form_pairs(&data).map_or_else(|| text(declared, data.clone()), Body::Form)
        }
        (Some(declared), _) => text(declared, data),
        (None, _) if looks_like_json(&data) => Body::Json(data),
        (None, _) => form_pairs(&data).map_or_else(
            || text("application/x-www-form-urlencoded", data.clone()),
            Body::Form,
        ),
    }
}

/// makes the request from what the options said
fn finish(mut reading: Reading) -> Result<Parsed, CurlError> {
    let url = reading.url.take().ok_or(CurlError::NoUrl)?;
    let (url, mut params, user_info) = split_url(&url);
    let mut headers = std::mem::take(&mut reading.headers);
    let mut auth = Auth::None;
    let content_type = headers
        .iter()
        .position(|h| h.name.eq_ignore_ascii_case("content-type"));
    let data = (!reading.data.is_empty()).then(|| reading.data.join("&"));

    // -G sends the data in the query instead
    let data = if reading.get {
        if let Some(data) = data {
            params.extend(form_pairs(&data).unwrap_or_else(|| vec![KeyValue::new(data, "")]));
        }

        None
    } else {
        data
    };
    let declared = content_type.map(|i| headers[i].value.clone());
    let body = body(reading.json.take(), data, declared.as_deref());

    // the body's kind says its content type
    if body != Body::None
        && let Some(index) = content_type
    {
        headers.remove(index);
    }

    // the weakest first: a user in the URL, then -u, then --oauth2-bearer,
    // and an Authorization header over all of them, as curl sends it
    if let Some((user, password)) = user_info {
        auth = Auth::Basic {
            username: user,
            password,
        };
    }

    if let Some(user) = reading.user.take() {
        let (username, password) = user.split_once(':').unwrap_or((&user, ""));

        auth = Auth::Basic {
            username: username.to_owned(),
            password: password.to_owned(),
        };
    }

    if let Some(token) = reading.bearer.take() {
        auth = Auth::Bearer { token };
    }

    let authorization = headers
        .iter()
        .position(|h| h.name.eq_ignore_ascii_case("authorization"));

    if let Some(index) = authorization {
        if auth != Auth::None {
            reading
                .warnings
                .push("the Authorization header is sent instead of -u".to_owned());
            auth = Auth::None;
        }

        if let Some(found) = auth_from_header(&headers[index].value) {
            auth = found;
            headers.remove(index);
        }
    }

    let method = match (reading.head, reading.method) {
        (true, _) => Method::Head,
        (false, Some(method)) => method,
        (false, None) if body != Body::None => Method::Post,
        (false, None) => Method::Get,
    };

    Ok(Parsed {
        request: Request {
            method,
            url,
            params,
            headers,
            body,
            auth,
        },
        warnings: reading.warnings,
    })
}

/// the auth an `Authorization` header holds, when curst has a kind for it
fn auth_from_header(value: &str) -> Option<Auth> {
    let (scheme, rest) = value.split_once(' ')?;
    let rest = rest.trim();

    if scheme.eq_ignore_ascii_case("bearer") && !rest.is_empty() {
        return Some(Auth::Bearer {
            token: rest.to_owned(),
        });
    }

    if scheme.eq_ignore_ascii_case("basic") {
        let decoded = decode_base64(rest)?;
        let text = String::from_utf8(decoded).ok()?;
        let (username, password) = text.split_once(':')?;

        return Some(Auth::Basic {
            username: username.to_owned(),
            password: password.to_owned(),
        });
    }

    None
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;

    for byte in text.trim_end_matches('=').bytes() {
        let value = ALPHABET.iter().position(|b| *b == byte)?;

        buffer = (buffer << 6) | u32::try_from(value).ok()?;
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }

    Some(out)
}

/// percent-encodes a query part, leaving `{{name}}` templates as they are
fn encode_query_part(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}") else {
            break;
        };

        // only a real variable name stays as it is
        let inner = &rest[start + 2..start + end];

        if VarName::new(inner).is_none() {
            out.push_str(&percent_encode(&rest[..start + 2]));
            rest = &rest[start + 2..];

            continue;
        }

        out.push_str(&percent_encode(&rest[..start]));
        out.push_str(&rest[start..start + end + 2]);
        rest = &rest[start + end + 2..];
    }

    out.push_str(&percent_encode(rest));

    out
}

/// `url` with the enabled `params` added to its query, encoded, and
/// `{{name}}` variables left as they are
#[must_use]
pub fn url_with_params(url: &str, params: &[KeyValue]) -> String {
    let mut url = url.to_owned();
    let query: Vec<String> = params
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| {
            format!(
                "{}={}",
                encode_query_part(&p.name),
                encode_query_part(&p.value)
            )
        })
        .collect();

    if !query.is_empty() {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&query.join("&"));
    }

    url
}

/// the value of an `Authorization` header for basic auth
#[must_use]
pub fn basic_value(username: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let bytes = format!("{username}:{password}").into_bytes();
    let mut out = String::from("Basic ");

    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);

        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[((n >> shift) & 63) as usize])
            } else {
                '='
            });
        }
    }

    out
}

/// the request as a curl command, one option per line; variables stay as
/// `{{name}}`, so no secret value is ever in it
#[must_use]
pub fn emit(request: &Request) -> String {
    let url = url_with_params(&request.url, &request.params);
    let mut parts = vec![format!("curl {}", quote(&url))];
    let has_body = request.body != Body::None;

    match request.method {
        Method::Head => parts.push("-I".into()),
        Method::Get if !has_body => {}
        method => parts.push(format!("-X {}", method.as_str())),
    }

    for header in request
        .headers
        .iter()
        .filter(|h| h.enabled && !h.name.is_empty())
    {
        parts.push(format!(
            "-H {}",
            quote(&format!("{}: {}", header.name, header.value))
        ));
    }

    match &request.auth {
        Auth::Inherit | Auth::None => {}
        Auth::Basic { username, password } => {
            parts.push(format!("-u {}", quote(&format!("{username}:{password}"))));
        }
        Auth::Bearer { token } => {
            parts.push(format!(
                "-H {}",
                quote(&format!("Authorization: Bearer {token}"))
            ));
        }
    }

    // --data-raw, so a body starting with @ is never read as a file name
    match &request.body {
        Body::None => {}
        Body::Json(text) => {
            parts.push(format!("-H {}", quote("Content-Type: application/json")));
            parts.push(format!("--data-raw {}", quote(text)));
        }
        Body::Text { content_type, text } => {
            parts.push(format!(
                "-H {}",
                quote(&format!("Content-Type: {content_type}"))
            ));
            parts.push(format!("--data-raw {}", quote(text)));
        }
        Body::Form(fields) => {
            let encoded: Vec<String> = fields
                .iter()
                .filter(|f| f.enabled && !f.name.is_empty())
                .map(|f| {
                    format!(
                        "{}={}",
                        encode_query_part(&f.name),
                        encode_query_part(&f.value)
                    )
                })
                .collect();

            parts.push(format!("--data-raw {}", quote(&encoded.join("&"))));
        }
    }

    parts.join(" \\\n  ")
}

/// a credential taken out of an imported request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// the variable that holds it now
    pub name: VarName,

    /// the value, to keep as a secret
    pub value: Secret,
}

/// a pasted command, with every value that looks like a credential taken
/// out and replaced by a `{{name}}` placeholder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// the request, holding placeholders only
    pub request: Request,

    /// the values taken out, to keep as secrets
    pub credentials: Vec<Credential>,

    /// options that were not used
    pub warnings: Vec<String>,
}

/// a variable name made from a header or parameter name
#[must_use]
pub fn name_for(text: &str) -> String {
    let name: String = text
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = name.trim_matches('_');

    if name.is_empty() {
        "secret".to_owned()
    } else {
        name.to_owned()
    }
}

/// splits an auth scheme off a header value: `Bearer abc` keeps `Bearer `
/// in the field and only `abc` is the secret
#[must_use]
pub fn split_scheme(value: &str) -> (&str, &str) {
    ["Bearer ", "Basic ", "bearer ", "basic "]
        .iter()
        .find_map(|scheme| value.strip_prefix(scheme).map(|rest| (*scheme, rest)))
        .unwrap_or(("", value))
}

/// reads a pasted curl command and takes its credentials out; `taken`
/// says whether a variable name is already in use where they will be kept
///
/// # Errors
/// as [`parse`]
pub fn import(command: &str, taken: impl Fn(&str) -> bool) -> Result<Imported, CurlError> {
    let Parsed {
        mut request,
        warnings,
    } = parse(command)?;
    let mut credentials: Vec<Credential> = Vec::new();

    for finding in detect::detect(&request) {
        let (hint, slot): (String, &mut String) = match finding.place {
            Place::Param(i) => (request.params[i].name.clone(), &mut request.params[i].value),
            Place::Header(i) => (
                request.headers[i].name.clone(),
                &mut request.headers[i].value,
            ),
            Place::FormField(i) => match &mut request.body {
                Body::Form(fields) => (fields[i].name.clone(), &mut fields[i].value),
                _ => continue,
            },
            Place::Password => match &mut request.auth {
                Auth::Basic { password, .. } => ("password".into(), password),
                _ => continue,
            },
            Place::Token => match &mut request.auth {
                Auth::Bearer { token } => ("token".into(), token),
                _ => continue,
            },
            // parsing moved these into the parameters and the auth
            Place::UrlUserInfo | Place::UrlQuery(_) => continue,
        };

        let base = name_for(&hint);
        let free =
            |name: &str| !taken(name) && !credentials.iter().any(|c| c.name.as_str() == name);
        let name = std::iter::once(base.clone())
            .chain((2..1000).map(|n| format!("{base}_{n}")))
            .find(|name| free(name))
            .and_then(|name| VarName::new(&name));
        let Some(name) = name else {
            continue;
        };

        let (scheme, secret) = split_scheme(slot);
        let value = Secret::new(secret);

        *slot = format!("{scheme}{{{{{name}}}}}");
        credentials.push(Credential { name, value });
    }

    Ok(Imported {
        request,
        credentials,
        warnings,
    })
}
