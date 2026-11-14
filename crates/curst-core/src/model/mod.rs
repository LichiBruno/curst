//! the request and collection model, free of any I/O

mod collection;

use std::fmt;
use std::str::FromStr;

pub use collection::{
    Capture, Collection, EnvId, Environment, Extract, Folder, Item, LocalId, SavedRequest, Source,
    SpecOrigin, Step, Variable, Workflow,
};

/// an HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// `GET`
    #[default]
    Get,

    /// `POST`
    Post,

    /// `PUT`
    Put,

    /// `PATCH`
    Patch,

    /// `DELETE`
    Delete,

    /// `HEAD`
    Head,

    /// `OPTIONS`
    Options,
}

impl Method {
    /// every method, in the order a picker shows them
    pub const ALL: [Self; 7] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Patch,
        Self::Delete,
        Self::Head,
        Self::Options,
    ];

    /// the method name as sent on the wire
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    /// the next method in [`Method::ALL`], wrapping around
    #[must_use]
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);

        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// the previous method in [`Method::ALL`], wrapping around
    #[must_use]
    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);

        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// a method name curst does not support
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown HTTP method {0:?}")]
pub struct UnknownMethod(pub String);

impl FromStr for Method {
    type Err = UnknownMethod;

    /// parses a method name, ignoring case
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(name))
            .ok_or_else(|| UnknownMethod(name.to_owned()))
    }
}

/// a name and value pair that can be switched off without being deleted,
/// used for query parameters, headers and form fields
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    /// the name
    pub name: String,

    /// the value
    pub value: String,

    /// whether the pair is sent
    pub enabled: bool,
}

impl KeyValue {
    /// an enabled pair
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }
}

/// the body of a request
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Body {
    /// no body
    #[default]
    None,

    /// JSON text, sent as `application/json`
    Json(String),

    /// any text, sent with the given content type
    Text {
        /// the `Content-Type` header value
        content_type: String,

        /// the body text
        text: String,
    },

    /// form fields, sent as `application/x-www-form-urlencoded`
    Form(Vec<KeyValue>),
}

impl Body {
    /// the content type this body is sent with, if it has one
    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Json(_) => Some("application/json"),
            Self::Text { content_type, .. } => Some(content_type),
            Self::Form(_) => Some("application/x-www-form-urlencoded"),
        }
    }

    /// the bytes sent on the wire, or `None` when there is no body
    #[must_use]
    pub fn encode(&self) -> Option<Vec<u8>> {
        match self {
            Self::None => None,
            Self::Json(text) | Self::Text { text, .. } => Some(text.as_bytes().to_vec()),
            Self::Form(fields) => Some(encode_pairs(fields).into_bytes()),
        }
    }
}

/// how a request authenticates
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Auth {
    /// what the folder or collection above says; a collection that says
    /// nothing means none
    #[default]
    Inherit,

    /// no authentication
    None,

    /// HTTP basic authentication
    Basic {
        /// the user name
        username: String,

        /// the password
        password: String,
    },

    /// an `Authorization: Bearer` token
    Bearer {
        /// the token
        token: String,
    },

    /// a key under a name of its own, such as `X-Api-Key`, in a header or
    /// the query
    ApiKey {
        /// the header or parameter name
        name: String,

        /// the key
        value: String,

        /// where it goes
        place: KeyPlace,
    },
}

/// where an API key goes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyPlace {
    /// a request header
    #[default]
    Header,

    /// a query parameter
    Query,
}

/// an HTTP request as the user edits it
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Request {
    /// the method
    pub method: Method,

    /// the URL, which may already contain a query string
    pub url: String,

    /// query parameters appended to the URL
    pub params: Vec<KeyValue>,

    /// request headers
    pub headers: Vec<KeyValue>,

    /// the body
    pub body: Body,

    /// the authentication
    pub auth: Auth,
}

impl Request {
    /// the URL with the enabled query parameters appended
    ///
    /// ```
    /// use curst_core::model::{KeyValue, Request};
    ///
    /// let request = Request {
    ///     url: "https://example.com/search?lang=en".into(),
    ///     params: vec![KeyValue::new("q", "rust & curl")],
    ///     ..Request::default()
    /// };
    /// assert_eq!(
    ///     request.full_url(),
    ///     "https://example.com/search?lang=en&q=rust%20%26%20curl"
    /// );
    /// ```
    #[must_use]
    pub fn full_url(&self) -> String {
        let query = encode_pairs(&self.params);

        if query.is_empty() {
            return self.url.clone();
        }

        let (base, fragment) = match self.url.split_once('#') {
            Some((base, fragment)) => (base, Some(fragment)),
            None => (self.url.as_str(), None),
        };

        let separator = if !base.contains('?') {
            "?"
        } else if base.ends_with('?') || base.ends_with('&') {
            ""
        } else {
            "&"
        };

        let mut url = format!("{base}{separator}{query}");

        if let Some(fragment) = fragment {
            url.push('#');
            url.push_str(fragment);
        }

        url
    }

    /// the enabled headers whose name matches, ignoring case
    pub fn header(&self, name: &str) -> impl Iterator<Item = &KeyValue> {
        self.headers
            .iter()
            .filter(move |h| h.enabled && h.name.eq_ignore_ascii_case(name))
    }
}

fn encode_pairs(pairs: &[KeyValue]) -> String {
    pairs
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| format!("{}={}", percent_encode(&p.name), percent_encode(&p.value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// percent-encodes everything except the RFC 3986 unreserved characters
///
/// ```
/// assert_eq!(curst_core::model::percent_encode("a b/é"), "a%20b%2F%C3%A9");
/// ```
#[must_use]
pub fn percent_encode(text: &str) -> String {
    use fmt::Write;

    let mut out = String::with_capacity(text.len());

    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_params(url: &str, params: Vec<KeyValue>) -> Request {
        Request {
            url: url.into(),
            params,
            ..Request::default()
        }
    }

    #[test]
    fn method_cycles_both_ways() {
        assert_eq!(Method::Get.next(), Method::Post);
        assert_eq!(Method::Options.next(), Method::Get);
        assert_eq!(Method::Get.previous(), Method::Options);

        for method in Method::ALL {
            assert_eq!(method.next().previous(), method);
        }

        assert_eq!(Method::Patch.to_string(), "PATCH");
    }

    #[test]
    fn methods_parse_ignoring_case() {
        for method in Method::ALL {
            assert_eq!(method.as_str().to_lowercase().parse(), Ok(method));
        }

        assert_eq!(
            "TRACE".parse::<Method>(),
            Err(UnknownMethod("TRACE".into()))
        );
    }

    #[test]
    fn full_url_without_params_is_unchanged() {
        assert_eq!(
            with_params("http://h/p?a=1", vec![]).full_url(),
            "http://h/p?a=1"
        );
    }

    #[test]
    fn full_url_skips_disabled_and_nameless_params() {
        let mut off = KeyValue::new("off", "1");
        off.enabled = false;

        let request = with_params(
            "http://h/p",
            vec![KeyValue::new("a", "1"), off, KeyValue::new("", "x")],
        );

        assert_eq!(request.full_url(), "http://h/p?a=1");
    }

    #[test]
    fn full_url_joins_existing_query_and_keeps_fragment() {
        let params = vec![KeyValue::new("b", "2")];

        assert_eq!(
            with_params("http://h/p?a=1", params.clone()).full_url(),
            "http://h/p?a=1&b=2"
        );
        assert_eq!(
            with_params("http://h/p?", params.clone()).full_url(),
            "http://h/p?b=2"
        );
        assert_eq!(
            with_params("http://h/p#top", params).full_url(),
            "http://h/p?b=2#top"
        );
    }

    #[test]
    fn body_encoding_and_content_type() {
        assert_eq!(Body::None.encode(), None);
        assert_eq!(Body::None.content_type(), None);

        let json = Body::Json("{}".into());

        assert_eq!(json.content_type(), Some("application/json"));
        assert_eq!(json.encode(), Some(b"{}".to_vec()));

        let text = Body::Text {
            content_type: "text/plain".into(),
            text: "hi".into(),
        };

        assert_eq!(text.content_type(), Some("text/plain"));
        assert_eq!(text.encode(), Some(b"hi".to_vec()));

        let form = Body::Form(vec![KeyValue::new("a b", "c&d"), KeyValue::new("e", "")]);

        assert_eq!(
            form.content_type(),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(form.encode(), Some(b"a%20b=c%26d&e=".to_vec()));
    }

    #[test]
    fn header_lookup_ignores_case_and_disabled() {
        let mut off = KeyValue::new("Accept", "x");
        off.enabled = false;

        let request = Request {
            headers: vec![KeyValue::new("accept", "a"), off],
            ..Request::default()
        };
        let found: Vec<_> = request.header("ACCEPT").map(|h| h.value.as_str()).collect();

        assert_eq!(found, ["a"]);
    }
}
