use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use tempfile::NamedTempFile;

/// a response as received, after any redirects
#[derive(Debug)]
pub struct Response {
    /// the status code
    pub status: u16,

    /// the status line, for example `HTTP/2 200`
    pub status_line: String,

    /// the headers of the final response, in the order received
    pub headers: Vec<(String, String)>,

    /// the body
    pub body: ResponseBody,

    /// how long each phase took
    pub timings: Timings,

    /// the URL of the final response
    pub effective_url: String,

    /// how many redirects were followed
    pub redirects: u32,

    /// a redirect that was not followed because its target is not approved
    /// for the secrets in the request
    pub stopped: Option<StoppedRedirect>,
}

/// a redirect left for the user to follow, with the request it would send
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppedRedirect {
    /// where it points, resolved against the URL that answered
    pub url: String,

    /// the method the next request uses: a 303, or a 301 or 302 after a
    /// POST, turns into a GET
    pub method: crate::model::Method,

    /// whether the next request keeps the body
    pub keeps_body: bool,
}

impl Response {
    /// the first header whose name matches, ignoring case
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// what kind of content the body holds
    #[must_use]
    pub fn kind(&self) -> BodyKind {
        BodyKind::detect(self.header("content-type"), self.body.preview())
    }

    /// the body as indented JSON, when it is complete and valid JSON
    #[must_use]
    pub fn pretty_json(&self) -> Option<String> {
        if self.body.is_truncated() {
            return None;
        }

        let value: serde_json::Value = serde_json::from_slice(self.body.preview()).ok()?;

        serde_json::to_string_pretty(&value).ok()
    }
}

/// time from the start of the transfer to the end of each phase, as libcurl
/// reports it, so each value includes the ones before it
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timings {
    /// name resolution done
    pub dns: Duration,

    /// TCP connection established
    pub connect: Duration,

    /// TLS handshake done, zero for plain HTTP
    pub tls: Duration,

    /// first response byte received
    pub first_byte: Duration,

    /// transfer complete
    pub total: Duration,
}

/// what a body holds, used to choose how to show it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// JSON text
    Json,

    /// any other text
    Text,

    /// binary data, shown as type and size only
    Binary,
}

impl BodyKind {
    /// classifies a body from its `Content-Type` and its first bytes
    ///
    /// ```
    /// use curst_core::http::BodyKind;
    ///
    /// assert_eq!(BodyKind::detect(Some("application/problem+json"), b"{}"), BodyKind::Json);
    /// assert_eq!(BodyKind::detect(Some("image/png"), b"\x89PNG"), BodyKind::Binary);
    /// assert_eq!(BodyKind::detect(None, b"plain"), BodyKind::Text);
    /// ```
    #[must_use]
    pub fn detect(content_type: Option<&str>, bytes: &[u8]) -> Self {
        let sniffed = || {
            if bytes.contains(&0) {
                Self::Binary
            } else {
                Self::Text
            }
        };
        let Some(content_type) = content_type else {
            return sniffed();
        };

        let mime = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();

        if mime == "application/json" || mime.ends_with("+json") {
            return Self::Json;
        }

        let textual = mime.starts_with("text/")
            || mime.ends_with("+xml")
            || matches!(
                mime.as_str(),
                "application/xml"
                    | "application/javascript"
                    | "application/x-www-form-urlencoded"
                    | "application/graphql"
                    | "application/yaml"
                    | "application/x-yaml"
                    | "application/toml"
                    | "application/x-ndjson"
                    | ""
            );

        if textual { sniffed() } else { Self::Binary }
    }
}

/// a response body: the first bytes in memory, and the whole body in an
/// owner-only temporary file when it is larger than the cap
#[derive(Debug)]
pub struct ResponseBody {
    preview: Vec<u8>,
    spill: Option<NamedTempFile>,
    len: u64,
    cap: usize,
}

impl ResponseBody {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            preview: Vec::new(),
            spill: None,
            len: 0,
            cap,
        }
    }

    /// a body held in memory, for tests and for building responses by hand
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        let cap = bytes.len();

        Self {
            preview: bytes,
            spill: None,
            len,
            cap,
        }
    }

    /// a body built as a transfer would build it, keeping at most `cap`
    /// bytes in memory
    ///
    /// # Errors
    /// fails when the temporary file for a body over the cap cannot be
    /// created or written
    pub fn with_cap(bytes: &[u8], cap: usize) -> io::Result<Self> {
        let mut body = Self::new(cap);

        body.push(bytes)?;

        Ok(body)
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> io::Result<()> {
        self.len += data.len() as u64;

        if let Some(file) = &mut self.spill {
            return file.write_all(data);
        }

        if self.preview.len() + data.len() <= self.cap {
            self.preview.extend_from_slice(data);

            return Ok(());
        }

        let room = self.cap - self.preview.len();
        let mut file = tempfile::Builder::new().prefix("curst-").tempfile()?;

        file.write_all(&self.preview)?;
        file.write_all(data)?;
        self.preview.extend_from_slice(&data[..room]);
        self.spill = Some(file);

        Ok(())
    }

    /// the bytes held in memory: the whole body, or its first bytes when
    /// [`ResponseBody::is_truncated`] is true
    #[must_use]
    pub fn preview(&self) -> &[u8] {
        &self.preview
    }

    /// whether the body was larger than the cap
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.spill.is_some()
    }

    /// the size of the whole body in bytes
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// whether the body is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// the temporary file holding the whole body, when it was over the cap
    #[must_use]
    pub fn spill_path(&self) -> Option<&Path> {
        self.spill.as_ref().map(NamedTempFile::path)
    }

    /// writes the whole body to `path`
    ///
    /// # Errors
    ///
    /// fails with [`io::ErrorKind::AlreadyExists`] when `path` exists, so a
    /// file is never overwritten, and with any other I/O error on the way
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let mut out = OpenOptions::new().write(true).create_new(true).open(path)?;

        match &self.spill {
            Some(spill) => {
                let mut source = File::open(spill.path())?;

                io::copy(&mut source, &mut out)?;
            }
            None => out.write_all(&self.preview)?,
        }

        out.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(content_type: Option<&str>, body: &[u8]) -> Response {
        Response {
            status: 200,
            status_line: "HTTP/1.1 200 OK".into(),
            headers: content_type.map_or_else(Vec::new, |ct| {
                vec![("Content-Type".to_owned(), ct.to_owned())]
            }),
            body: ResponseBody::from_bytes(body.to_vec()),
            timings: Timings::default(),
            effective_url: "http://h/".into(),
            redirects: 0,
            stopped: None,
        }
    }

    #[test]
    fn kinds() {
        let cases: [(Option<&str>, &[u8], BodyKind); 9] = [
            (
                Some("application/json; charset=utf-8"),
                b"{}",
                BodyKind::Json,
            ),
            (Some("Application/JSON"), b"{}", BodyKind::Json),
            (Some("text/html"), b"<p>", BodyKind::Text),
            (Some("text/plain"), b"a\0b", BodyKind::Binary),
            (Some("application/atom+xml"), b"<a/>", BodyKind::Text),
            (Some("application/yaml"), b"a: 1", BodyKind::Text),
            (Some("application/octet-stream"), b"abc", BodyKind::Binary),
            (None, b"abc", BodyKind::Text),
            (None, b"\0\x01", BodyKind::Binary),
        ];

        for (content_type, bytes, kind) in cases {
            assert_eq!(
                BodyKind::detect(content_type, bytes),
                kind,
                "{content_type:?}"
            );
        }
    }

    #[test]
    fn pretty_json_keeps_key_order_and_big_numbers() {
        let r = response(
            Some("application/json"),
            br#"{"z":1,"a":12345678901234567890123}"#,
        );

        assert_eq!(
            r.pretty_json().as_deref(),
            Some("{\n  \"z\": 1,\n  \"a\": 12345678901234567890123\n}")
        );
        assert_eq!(r.kind(), BodyKind::Json);
        assert_eq!(r.header("content-TYPE"), Some("application/json"));
        assert_eq!(r.header("missing"), None);
    }

    #[test]
    fn pretty_json_is_none_for_invalid_json() {
        assert_eq!(response(None, b"{nope").pretty_json(), None);
    }

    #[test]
    fn with_cap_matches_a_transfer() {
        let body = ResponseBody::with_cap(b"abcdef", 4).unwrap();

        assert!(body.is_truncated());
        assert_eq!(body.preview(), b"abcd");
        assert_eq!(body.len(), 6);
    }

    #[test]
    fn small_body_stays_in_memory() {
        let mut body = ResponseBody::new(8);

        body.push(b"abcd").unwrap();
        body.push(b"efgh").unwrap();

        assert_eq!(body.preview(), b"abcdefgh");
        assert!(!body.is_truncated());
        assert_eq!(body.len(), 8);
        assert!(body.spill_path().is_none());
    }

    #[test]
    fn large_body_spills_to_an_owner_only_file() {
        let mut body = ResponseBody::new(4);

        body.push(b"abc").unwrap();
        body.push(b"defg").unwrap();
        body.push(b"hi").unwrap();

        assert_eq!(body.preview(), b"abcd");
        assert!(body.is_truncated());
        assert_eq!(body.len(), 9);

        let spill = body.spill_path().unwrap().to_owned();

        assert_eq!(std::fs::read(&spill).unwrap(), b"abcdefghi");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&spill).unwrap().permissions().mode();

            assert_eq!(mode & 0o777, 0o600);
        }

        let r = Response {
            body,
            ..response(Some("application/json"), b"")
        };

        assert_eq!(r.pretty_json(), None);

        drop(r);

        assert!(!spill.exists());
    }

    #[test]
    fn save_writes_the_whole_body_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("big.bin");
        let mut big = ResponseBody::new(2);

        big.push(b"0123456789").unwrap();
        big.save_to(&target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"0123456789");

        let err = big.save_to(&target).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        let small = ResponseBody::from_bytes(b"hi".to_vec());
        let target = dir.path().join("small.txt");

        small.save_to(&target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"hi");
        assert!(!small.is_empty());
        assert!(ResponseBody::from_bytes(Vec::new()).is_empty());
    }
}
