//! taking values out of an answer for a workflow: a path into a JSON body,
//! or a header
//!
//! a value is only taken from a whole, successful answer: a failed status,
//! a body cut short, or a redirect left unfollowed gives nothing

use serde_json::Value;

use crate::http::Response;
use crate::model::Extract;

/// one part of a JSON path
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Key(String),
    Index(usize),
}

/// the parts of `path`: `$.data.token`, `data.token`, `items[0].id` and
/// `$["odd key"]` all work; `$` alone is the whole body
fn parts(path: &str) -> Result<Vec<Part>, String> {
    let bad = || format!("{path:?} is not a JSON path such as $.data.token");
    let mut rest = path.trim();

    rest = rest.strip_prefix('$').unwrap_or(rest);

    let mut parts = Vec::new();

    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').ok_or_else(bad)?;
            let inner = after[..end].trim();

            rest = &after[end + 1..];

            let quoted = inner
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));

            match quoted {
                Some(key) => parts.push(Part::Key(key.to_owned())),
                None => parts.push(Part::Index(inner.parse().map_err(|_| bad())?)),
            }

            continue;
        }

        rest = rest.strip_prefix('.').unwrap_or(rest);

        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];

        if key.is_empty() {
            return Err(bad());
        }

        parts.push(Part::Key(key.to_owned()));
        rest = &rest[end..];
    }

    Ok(parts)
}

/// the value at `path` in `body`, as text: a string as it is, anything
/// else as JSON
///
/// # Errors
/// when the body is not JSON, the path is not one, or nothing is there
pub fn json_value(body: &str, path: &str) -> Result<String, String> {
    let parts = parts(path)?;
    let root: Value =
        serde_json::from_str(body).map_err(|_| "the answer is not JSON".to_owned())?;
    let mut value = &root;

    for part in &parts {
        let next = match part {
            Part::Key(key) => value.get(key.as_str()),
            Part::Index(index) => value.get(*index),
        };

        value = next.ok_or_else(|| format!("the answer has nothing at {path}"))?;
    }

    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Null => Err(format!("the answer has null at {path}")),
        other => Ok(other.to_string()),
    }
}

/// the value `from` points to in `response`
///
/// # Errors
/// when the answer did not succeed, is not whole, or does not hold it
pub fn extract(response: &Response, from: &Extract) -> Result<String, String> {
    if !(200..300).contains(&response.status) {
        return Err(format!("the answer was {}", response.status_line));
    }

    if response.stopped.is_some() {
        return Err("the answer was a redirect that was not followed".into());
    }

    match from {
        Extract::Header(name) => response
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_owned())
            .ok_or_else(|| format!("the answer has no {name} header")),
        Extract::Json(path) => {
            if response.body.is_truncated() {
                return Err("the answer is too large to read whole".into());
            }

            json_value(&String::from_utf8_lossy(response.body.preview()), path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{
        "access_token": "abc",
        "data": { "items": [ { "id": 7 }, { "id": 8 } ], "odd key": true },
        "nothing": null
    }"#;

    #[test]
    fn paths_reach_into_objects_and_lists() {
        let value = |path: &str| json_value(BODY, path);

        assert_eq!(value("$.access_token"), Ok("abc".into()));
        assert_eq!(value("access_token"), Ok("abc".into()));
        assert_eq!(value("data.items[1].id"), Ok("8".into()));
        assert_eq!(value("$.data[\"odd key\"]"), Ok("true".into()));
        assert_eq!(value("$['data']['items'][0]"), Ok("{\"id\":7}".into()));
        assert!(value("$.missing").unwrap_err().contains("nothing at"));
        assert!(value("$.nothing").unwrap_err().contains("null"));
        assert!(value("$.data.items[x]").is_err());
        assert!(value("$..x").is_err());
        assert!(
            json_value("<html>", "$.a")
                .unwrap_err()
                .contains("not JSON")
        );
    }

    #[test]
    fn only_a_whole_successful_answer_gives_a_value() {
        use crate::http::{ResponseBody, Timings};

        let response = |status: u16, body: &str| Response {
            status,
            status_line: format!("HTTP/1.1 {status}"),
            headers: vec![("X-Token".into(), " t1 ".into())],
            body: ResponseBody::from_bytes(body.as_bytes().to_vec()),
            timings: Timings::default(),
            effective_url: "http://h/".into(),
            redirects: 0,
            stopped: None,
        };
        let json = Extract::Json("$.access_token".into());

        assert_eq!(extract(&response(200, BODY), &json), Ok("abc".into()));
        assert_eq!(
            extract(&response(200, ""), &Extract::Header("x-token".into())),
            Ok("t1".into())
        );
        assert!(
            extract(&response(401, BODY), &json)
                .unwrap_err()
                .contains("401")
        );
        assert!(
            extract(&response(200, ""), &Extract::Header("Other".into()))
                .unwrap_err()
                .contains("no Other header")
        );
    }
}
