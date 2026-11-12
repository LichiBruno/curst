//! the small part of URL handling curst needs: origins and resolving a
//! redirect target, without any I/O

/// scheme, host and port, compared to decide whether a redirect leaves the
/// server the credentials were meant for
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// the scheme, in lower case
    pub scheme: String,

    /// the host, in lower case
    pub host: String,

    /// the port, filled in from the scheme when the URL has none
    pub port: Option<u16>,
}

/// the origin of an absolute URL, or `None` when it has no scheme or host
///
/// ```
/// use curst_core::url::origin;
/// let a = origin("https://API.example.com/v1").unwrap();
/// let b = origin("https://api.example.com:443/v2").unwrap();
/// assert_eq!(a, b);
/// assert_ne!(a, origin("http://api.example.com/v1").unwrap());
/// ```
#[must_use]
pub fn origin(url: &str) -> Option<Origin> {
    let (scheme, rest) = split_scheme(url)?;
    let authority = rest
        .strip_prefix("//")?
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, Some(port)),
        _ => (host_port, None),
    };

    if host.is_empty() {
        return None;
    }

    let scheme = scheme.to_ascii_lowercase();
    let port = match port {
        Some(port) if !port.is_empty() => Some(port.parse().ok()?),
        _ => default_port(&scheme),
    };

    Some(Origin {
        host: host.to_ascii_lowercase(),
        scheme,
        port,
    })
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn split_scheme(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));

    valid.then_some((scheme, rest))
}

/// resolves a `Location` header against the URL that returned it, as
/// RFC 3986 section 5 describes
///
/// ```
/// use curst_core::url::resolve;
/// let base = "https://h/a/b/c?x=1";
/// assert_eq!(resolve(base, "d").as_deref(), Some("https://h/a/b/d"));
/// assert_eq!(resolve(base, "../d").as_deref(), Some("https://h/a/d"));
/// assert_eq!(resolve(base, "/d").as_deref(), Some("https://h/d"));
/// assert_eq!(resolve(base, "//other/d").as_deref(), Some("https://other/d"));
/// assert_eq!(resolve(base, "http://x/").as_deref(), Some("http://x/"));
/// ```
#[must_use]
pub fn resolve(base: &str, location: &str) -> Option<String> {
    let location = location.trim();

    if split_scheme(location).is_some_and(|(_, rest)| rest.starts_with("//")) {
        return Some(location.to_owned());
    }

    let (scheme, rest) = split_scheme(base)?;
    let after_slashes = rest.strip_prefix("//")?;

    if let Some(network) = location.strip_prefix("//") {
        return Some(format!("{scheme}://{network}"));
    }

    let authority_end = after_slashes
        .find(['/', '?', '#'])
        .unwrap_or(after_slashes.len());
    let (authority, tail) = after_slashes.split_at(authority_end);
    let prefix = format!("{scheme}://{authority}");
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let (base_path, base_rest) = tail.split_at(path_end);
    let base_query = base_rest.split('#').next().unwrap_or_default();

    if location.is_empty() {
        return Some(format!("{prefix}{base_path}{base_query}"));
    }

    if location.starts_with('#') {
        return Some(format!("{prefix}{base_path}{base_query}{location}"));
    }

    if location.starts_with('?') {
        return Some(format!("{prefix}{base_path}{location}"));
    }

    let loc_end = location.find(['?', '#']).unwrap_or(location.len());
    let (loc_path, loc_rest) = location.split_at(loc_end);
    let merged = if loc_path.starts_with('/') {
        loc_path.to_owned()
    } else if base_path.is_empty() {
        format!("/{loc_path}")
    } else {
        let dir = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);

        format!("{dir}/{loc_path}")
    };

    Some(format!(
        "{prefix}{}{loc_rest}",
        remove_dot_segments(&merged)
    ))
}

fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len().saturating_sub(1);

    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {
                if index == last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }

                if index == last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }

    let joined = out.join("/");

    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        let o = origin("HTTPS://user:pw@Example.COM:8443/p?q#f").unwrap();

        assert_eq!(
            o,
            Origin {
                scheme: "https".into(),
                host: "example.com".into(),
                port: Some(8443)
            }
        );
        assert_eq!(origin("http://h").unwrap().port, Some(80));
        assert_eq!(origin("http://[::1]:9000/").unwrap().host, "[::1]");
        assert_eq!(origin("http://[::1]/").unwrap().port, Some(80));
        assert_eq!(origin("ftp://h/").unwrap().port, None);
        assert_eq!(origin("http://h:/").unwrap().port, Some(80));
        assert!(origin("http://h:notaport/").is_none());
        assert!(origin("/relative").is_none());
        assert!(origin("http:///nohost").is_none());
        assert!(origin("1http://h/").is_none());
    }

    #[test]
    fn rfc_3986_examples() {
        let base = "http://a/b/c/d;p?q";
        let cases = [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            (";x", "http://a/b/c/;x"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../g", "http://a/g"),
            ("../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            ("..g", "http://a/b/c/..g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/../h", "http://a/b/c/h"),
            ("https://x/y", "https://x/y"),
        ];

        for (location, expected) in cases {
            assert_eq!(
                resolve(base, location).as_deref(),
                Some(expected),
                "{location}"
            );
        }
    }

    #[test]
    fn base_without_path() {
        assert_eq!(resolve("http://h", "x").as_deref(), Some("http://h/x"));
        assert_eq!(resolve("http://h?q", "?r").as_deref(), Some("http://h?r"));
        assert_eq!(resolve("relative", "x"), None);
    }
}
