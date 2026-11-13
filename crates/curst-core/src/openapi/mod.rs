//! `OpenAPI` and Swagger specs: a spec becomes a collection, with a folder
//! per tag and a request per operation
//!
//! a spec is untrusted input. only references inside it (`#/…`) are
//! followed, never files or URLs, and its size, its nesting and the
//! examples made from it are all capped. values in it that look like
//! credentials never reach a request: placeholders take their place

mod convert;
mod discover;
pub mod refresh;
mod schema;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde_json::Value;

pub use convert::{Converted, convert};
pub use discover::{Fetched, discover, fetch_with, specs_in};

/// the largest spec that is read
pub const MAX_BYTES: usize = 20 * 1024 * 1024;

/// how many `$ref`s in a row are followed before giving up
const MAX_REFS: usize = 32;

/// why a spec could not be used
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// it is neither JSON nor YAML
    #[error("not JSON or YAML: {0}")]
    Syntax(String),

    /// it has no `openapi` or `swagger` field
    #[error("not an OpenAPI or Swagger spec")]
    NotASpec,

    /// a version curst does not read
    #[error("{0} is not a version curst reads; it reads Swagger 2.0 and OpenAPI 3.0 and 3.1")]
    Version(String),

    /// too large to read
    #[error("the spec is larger than {} MB", MAX_BYTES / 1024 / 1024)]
    TooLarge,
}

/// which family of specs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// `swagger: "2.0"`
    Swagger2,

    /// `openapi: 3.0.x` or `3.1.x`
    OpenApi3,
}

/// a spec's text, JSON or YAML, as a value
///
/// # Errors
/// when it is too large or neither JSON nor YAML
pub fn parse(text: &str) -> Result<Value, SpecError> {
    if text.len() > MAX_BYTES {
        return Err(SpecError::TooLarge);
    }

    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text).map_err(|e| SpecError::Syntax(e.to_string()));
    }

    // YAML anchors are expanded with a limit, so an alias bomb is refused
    serde_yaml_ng::from_str(text).map_err(|e| SpecError::Syntax(e.to_string()))
}

/// the version a spec declares
///
/// # Errors
/// when it declares none, or one curst does not read
pub fn version(spec: &Value) -> Result<Version, SpecError> {
    let text = |key: &str| {
        spec.get(key)
            .map(|v| v.as_str().map_or_else(|| v.to_string(), ToOwned::to_owned))
    };

    if let Some(swagger) = text("swagger") {
        return if swagger.starts_with('2') {
            Ok(Version::Swagger2)
        } else {
            Err(SpecError::Version(format!("Swagger {swagger}")))
        };
    }

    match text("openapi") {
        Some(v) if v.starts_with("3.0") || v.starts_with("3.1") => Ok(Version::OpenApi3),
        Some(v) => Err(SpecError::Version(format!("OpenAPI {v}"))),
        None => Err(SpecError::NotASpec),
    }
}

/// whether a value at the top of a file is a spec, for picking specs out
/// of a folder
#[must_use]
pub fn is_spec(value: &Value) -> bool {
    value.get("openapi").is_some() || value.get("swagger").is_some()
}

/// follows `$ref`s inside the spec until a value that is not one; `None`
/// for a reference to another file or URL, or one that leads nowhere
pub(crate) fn resolve<'a>(root: &'a Value, value: &'a Value) -> Option<&'a Value> {
    let mut current = value;

    for _ in 0..MAX_REFS {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return Some(current);
        };

        current = root.pointer(reference.strip_prefix('#')?)?;
    }

    None
}

/// a value with every `$ref` inside it replaced by what it points to,
/// keys sorted, as text: the same operation always gives the same text,
/// however the spec orders it. a reference met again inside itself stays
/// as written, so a recursive schema ends
fn canonical(root: &Value, value: &Value, seen: &mut Vec<String>) -> Value {
    if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        if seen.iter().any(|s| s == reference) || seen.len() >= MAX_REFS {
            return value.clone();
        }

        let Some(target) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
        else {
            return value.clone();
        };

        seen.push(reference.to_owned());

        let resolved = canonical(root, target, seen);

        seen.pop();

        return resolved;
    }

    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> = map
                .iter()
                .map(|(key, value)| (key, canonical(root, value, seen)))
                .collect();

            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| canonical(root, v, seen)).collect())
        }
        other => other.clone(),
    }
}

/// FNV-1a, 64 bits: fixed for good, unlike the standard library's hasher,
/// so a fingerprint written today still matches next year
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// a fingerprint of what the spec says about one operation: its own
/// fields, the parameters of its path, and what both refer to
pub(crate) fn fingerprint(root: &Value, path_item: &Value, operation: &Value) -> String {
    let path_parameters = path_item.get("parameters").cloned().unwrap_or(Value::Null);
    let whole = serde_json::json!({
        "operation": operation,
        "path_parameters": path_parameters,
    });
    let text = canonical(root, &whole, &mut Vec::new()).to_string();

    format!("{:016x}", fnv1a(&text))
}

/// a spec file's text, refused when too large before it is read
///
/// # Errors
/// when it cannot be read or is too large
pub fn read_file(path: &std::path::Path) -> Result<String, crate::store::StoreError> {
    use crate::store::StoreError;

    let size = std::fs::metadata(path)
        .map_err(|e| StoreError::io(path, e))?
        .len();

    if usize::try_from(size).map_or(true, |size| size > MAX_BYTES) {
        return Err(StoreError::Invalid(format!(
            "{} is larger than {} MB, too large for a spec",
            path.display(),
            MAX_BYTES / 1024 / 1024
        )));
    }

    std::fs::read_to_string(path).map_err(|e| StoreError::io(path, e))
}

impl crate::store::Library {
    /// makes a new collection from a converted spec, recording where it
    /// came from so it can be fetched again; nothing is left behind when
    /// it fails
    ///
    /// # Errors
    /// when the collection cannot be written
    pub fn import_spec(
        &self,
        converted: Converted,
        source: &crate::model::Source,
    ) -> Result<(crate::store::CollectionDir, crate::store::bundle::Bundle), crate::store::StoreError>
    {
        use crate::store::CollectionStore;

        self.import_with(converted.bundle, |dir, bundle| {
            crate::store::bundle::fill(dir, bundle)?;
            dir.save_source(Some(source))
        })
    }
}
