//! example values made from schemas: a parameter's value, and a body
//! that shows what the operation expects when the spec gives no example

use serde_json::{Map, Value, json};

use super::resolve;

/// how deep a made-up body goes
const MAX_DEPTH: usize = 8;

/// how many values a made-up body holds at most
const MAX_NODES: usize = 500;

/// a value as text for a parameter or a form field
pub(crate) fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// the example a parameter or media type gives, in the order the spec
/// families put them: `example`, the first of `examples`, then the schema's
/// own example, default or first allowed value
pub(crate) fn example(root: &Value, holder: &Value) -> Option<Value> {
    if let Some(example) = holder.get("example").or_else(|| holder.get("x-example")) {
        return Some(example.clone());
    }

    let first = holder.get("examples").and_then(|examples| match examples {
        Value::Object(map) => map.values().next(),
        Value::Array(list) => list.first(),
        _ => None,
    });

    if let Some(first) = first.and_then(|e| resolve(root, e)) {
        // an Example Object holds its value under `value`; a 3.1 schema's
        // `examples` list holds the values themselves
        return Some(first.get("value").cloned().unwrap_or_else(|| first.clone()));
    }

    let schema = holder
        .get("schema")
        .and_then(|s| resolve(root, s))
        .unwrap_or(holder);

    schema
        .get("example")
        .or_else(|| schema.get("default"))
        .or_else(|| schema.get("enum").and_then(|e| e.get(0)))
        .or_else(|| schema.get("examples").and_then(|e| e.get(0)))
        .cloned()
}

/// a value of the shape `schema` describes, its examples used where it
/// has them
pub(crate) fn skeleton(root: &Value, schema: &Value) -> Value {
    let mut budget = MAX_NODES;

    make(root, schema, 0, &mut Vec::new(), &mut budget)
}

fn make(
    root: &Value,
    schema: &Value,
    depth: usize,
    seen: &mut Vec<String>,
    budget: &mut usize,
) -> Value {
    if depth > MAX_DEPTH || *budget == 0 {
        return Value::Null;
    }

    *budget -= 1;

    // a schema that holds itself, such as a tree of nodes, stops the
    // second time it is met
    let reference = schema
        .get("$ref")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    if let Some(reference) = &reference {
        if seen.contains(reference) {
            return Value::Null;
        }

        seen.push(reference.clone());
    }

    let value = match resolve(root, schema) {
        Some(schema) => shape(root, schema, depth, seen, budget),
        None => Value::Null,
    };

    if reference.is_some() {
        seen.pop();
    }

    value
}

fn shape(
    root: &Value,
    schema: &Value,
    depth: usize,
    seen: &mut Vec<String>,
    budget: &mut usize,
) -> Value {
    if let Some(given) = schema
        .get("example")
        .or_else(|| schema.get("default"))
        .or_else(|| schema.get("examples").and_then(|e| e.get(0)))
        .or_else(|| schema.get("const"))
        .or_else(|| schema.get("enum").and_then(|e| e.get(0)))
    {
        return given.clone();
    }

    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        let mut merged = Map::new();

        for part in parts {
            if let Value::Object(map) = make(root, part, depth + 1, seen, budget) {
                merged.extend(map);
            }
        }

        return Value::Object(merged);
    }

    if let Some(first) = ["oneOf", "anyOf"]
        .iter()
        .find_map(|key| schema.get(*key).and_then(|v| v.get(0)))
    {
        return make(root, first, depth + 1, seen, budget);
    }

    // 3.1 allows a list of types, such as ["string", "null"]
    let kind = match schema.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null")
            .unwrap_or("null"),
        _ if schema.get("properties").is_some() => "object",
        _ if schema.get("items").is_some() => "array",
        _ => "",
    };

    match kind {
        "object" => {
            let mut object = Map::new();

            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    if *budget == 0 {
                        break;
                    }

                    object.insert(name.clone(), make(root, property, depth + 1, seen, budget));
                }
            }

            Value::Object(object)
        }
        "array" => match schema.get("items") {
            Some(items) => match make(root, items, depth + 1, seen, budget) {
                Value::Null => json!([]),
                item => json!([item]),
            },
            None => json!([]),
        },
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(false),
        "string" => json!(string_for(schema.get("format").and_then(Value::as_str))),
        _ => Value::Null,
    }
}

/// a string that fits a format, so the example passes a server's checks
fn string_for(format: Option<&str>) -> &'static str {
    match format {
        Some("date-time") => "2026-01-01T00:00:00Z",
        Some("date") => "2026-01-01",
        Some("time") => "00:00:00",
        Some("email") => "user@example.com",
        Some("uuid") => "00000000-0000-0000-0000-000000000000",
        Some("uri" | "url") => "https://example.com",
        Some("ipv4") => "192.0.2.1",
        Some("ipv6") => "2001:db8::1",
        _ => "string",
    }
}
