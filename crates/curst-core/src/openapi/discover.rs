//! finding a spec: the ones in a folder, and the one behind a URL, which
//! may be the spec itself or a documentation page that loads it
//!
//! the page is only read, never run: the spec's address is looked for in
//! what Swagger UI and Redoc are configured with, then at the usual paths

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MAX_BYTES, is_spec, parse};

/// how deep a folder is searched
const MAX_DEPTH: usize = 6;

/// how many files a folder search looks at
const MAX_FILES: usize = 5000;

/// how many addresses a URL search fetches at most
const MAX_FETCHES: usize = 12;

/// where specs usually are, tried when a page does not say
const USUAL_PATHS: [&str; 8] = [
    "/openapi.json",
    "/swagger.json",
    "/v3/api-docs",
    "/openapi.yaml",
    "/swagger/v1/swagger.json",
    "/api-docs",
    "/v2/api-docs",
    "/swagger.yaml",
];

/// the specs in a folder and the folders under it, as paths relative to
/// it, the shallowest first; files git ignores are left out
#[must_use]
pub fn specs_in(folder: &Path) -> Vec<String> {
    let walk = ignore::WalkBuilder::new(folder)
        .max_depth(Some(MAX_DEPTH))
        .require_git(false)
        .build();
    let mut found: Vec<PathBuf> = Vec::new();

    for entry in walk.filter_map(Result::ok).take(MAX_FILES) {
        let path = entry.path();
        let wanted = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "json" | "yaml" | "yml"));
        let small = entry.metadata().is_ok_and(|m| {
            m.is_file() && usize::try_from(m.len()).is_ok_and(|len| len <= MAX_BYTES)
        });

        if wanted && small && looks_like_spec(path) {
            found.push(path.to_owned());
        }
    }

    found.sort_by_key(|p| (p.components().count(), p.clone()));
    found
        .into_iter()
        .filter_map(|p| p.strip_prefix(folder).ok().map(|p| p.display().to_string()))
        .collect()
}

/// whether a file is a spec: a cheap look at its start, then a full read
fn looks_like_spec(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let start: String = text.chars().take(4096).collect();

    (start.contains("openapi") || start.contains("swagger"))
        && parse(&text).is_ok_and(|value| is_spec(&value))
}

/// what one fetch gave back
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// where it ended up, after redirects
    pub url: String,

    /// the body as text
    pub body: String,
}

/// the quoted text after each `key` in `text`, such as the URL in
/// `url: "/openapi.json"`
fn quoted_after(text: &str, key: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;

    while let Some(at) = rest.find(key) {
        rest = &rest[at + key.len()..];

        let trimmed = rest.trim_start();
        let Some(quote) = trimmed
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'))
        else {
            continue;
        };
        let inner = &trimmed[1..];

        if let Some(end) = inner.find(quote) {
            found.push(inner[..end].to_owned());
        }
    }

    found
}

/// the addresses a documentation page names for its spec, in the order
/// worth trying
fn named_in(page: &str) -> Vec<String> {
    let mut found = Vec::new();

    // Redoc
    found.extend(quoted_after(page, "spec-url="));
    found.extend(quoted_after(page, "Redoc.init("));

    // Swagger UI, inline or in its initializer, and its config file
    for key in ["url:", "\"url\":", "configUrl:", "\"configUrl\":"] {
        found.extend(quoted_after(page, key));
    }

    // a script that holds the configuration: Swagger UI's initializer,
    // or the init script NestJS and others write, never the UI itself
    found.extend(quoted_after(page, "src=").into_iter().filter(|src| {
        let lower = src.to_ascii_lowercase();

        lower.contains("swagger") && (lower.contains("init") || lower.contains("config"))
    }));

    found.retain(|url| {
        let lower = url.to_ascii_lowercase();

        !url.is_empty()
            && !url.starts_with('#')
            && ![".css", ".png", ".ico", ".svg", ".html"]
                .iter()
                .any(|ext| lower.ends_with(ext))
    });

    found
}

/// the object written after a `swaggerDoc` key, as some servers put the
/// whole spec in the script that starts Swagger UI
fn embedded_spec(body: &str) -> Option<Value> {
    let mut rest = body;

    while let Some(at) = rest.find("swaggerDoc") {
        rest = &rest[at + "swaggerDoc".len()..];

        let after = rest.trim_start_matches(['"', '\'', ' ']).trim_start();
        let Some(object) = after.strip_prefix(':').map(str::trim_start) else {
            continue;
        };

        if let Some(text) = balanced(object)
            && let Ok(value) = serde_json::from_str::<Value>(text)
            && is_spec(&value)
        {
            return Some(value);
        }
    }

    None
}

/// the `{…}` at the start of `text`, its braces matched outside strings
fn balanced(text: &str) -> Option<&str> {
    if !text.starts_with('{') {
        return None;
    }

    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for (at, c) in text.char_indices() {
        if let Some(open) = quote {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                c if c == open => quote = None,
                _ => {}
            }

            continue;
        }

        match c {
            '"' | '\'' => quote = Some(c),
            '{' => depth += 1,
            '}' => {
                depth -= 1;

                if depth == 0 {
                    return Some(&text[..=at]);
                }
            }
            _ => {}
        }
    }

    None
}

/// where some frameworks, such as `NestJS`, serve the spec next to its
/// documentation page: `/docs` has it at `/docs-json` and `/docs-yaml`
fn siblings(url: &str) -> Vec<String> {
    let page = url
        .split(['#', '?'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');

    // only a page with a path has a sibling
    if page.matches('/').count() < 3 {
        return Vec::new();
    }

    vec![format!("{page}-json"), format!("{page}-yaml")]
}

/// the spec behind `start`: the spec itself, a page that names it or holds
/// it, an address next to the page, or one of the usual paths; with where
/// it was found
///
/// # Errors
/// when no spec is found within the fetches allowed, or a fetch fails
/// before anything was found
pub fn discover(
    start: &str,
    mut fetch: impl FnMut(&str) -> Result<Fetched, String>,
) -> Result<(Value, String), String> {
    // a fragment is never sent: the address is the same without it
    let start = start.split('#').next().unwrap_or(start);
    let mut queue: Vec<String> = vec![start.to_owned()];
    let mut tried: Vec<String> = Vec::new();
    let mut first_error = None;
    let mut usual_added = false;

    while let Some(url) = (!queue.is_empty()).then(|| queue.remove(0)) {
        if tried.contains(&url) {
            continue;
        }

        if tried.len() == MAX_FETCHES {
            break;
        }

        tried.push(url.clone());

        let fetched = match fetch(&url) {
            Ok(fetched) => fetched,
            Err(err) => {
                first_error.get_or_insert(err);

                continue;
            }
        };

        if let Ok(value) = parse(&fetched.body)
            && is_spec(&value)
        {
            return Ok((value, fetched.url));
        }

        if let Some(value) = embedded_spec(&fetched.body) {
            return Ok((value, fetched.url));
        }

        // what the page names comes before the usual paths
        let named: Vec<String> = named_in(&fetched.body)
            .iter()
            .filter_map(|candidate| crate::url::resolve(&fetched.url, candidate))
            .collect();

        for (index, url) in named.into_iter().enumerate() {
            queue.insert(index, url);
        }

        if !usual_added {
            usual_added = true;
            queue.extend(siblings(start));
            queue.extend(
                USUAL_PATHS
                    .iter()
                    .filter_map(|path| crate::url::resolve(&fetched.url, path)),
            );
        }
    }

    Err(match first_error {
        Some(err) if tried.len() == 1 => err,
        _ => format!(
            "no OpenAPI or Swagger spec was found at {start} or the {} places it points to",
            tried.len().saturating_sub(1)
        ),
    })
}

/// fetches one address for [`discover`] with `executor`: a GET that asks
/// for JSON or YAML, `headers` added; anything but a 2xx answer, or a body
/// too large to keep whole, is an error
///
/// # Errors
/// when the request fails, the answer is not a success, or it is too large
pub fn fetch_with(
    executor: &dyn crate::http::HttpExecutor,
    url: &str,
    headers: &[crate::model::KeyValue],
    cancel: &crate::http::Cancel,
    redirects: &crate::http::RedirectRule,
) -> Result<Fetched, String> {
    use crate::model::{KeyValue, Request};

    let mut all = vec![KeyValue::new(
        "Accept",
        "application/json, application/yaml, text/yaml, text/html;q=0.8, */*;q=0.5",
    )];

    all.extend(headers.iter().cloned());

    let request = Request {
        url: url.to_owned(),
        headers: all,
        ..Request::default()
    };
    let response = executor
        .execute(&request, cancel, redirects)
        .map_err(|e| e.to_string())?;

    if !(200..300).contains(&response.status) {
        return Err(format!("{url} answered {}", response.status_line));
    }

    if response.body.is_truncated() {
        return Err(format!("{url} is too large to read as a spec"));
    }

    Ok(Fetched {
        url: response.effective_url,
        body: String::from_utf8_lossy(response.body.preview()).into_owned(),
    })
}
