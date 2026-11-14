//! a spec as a collection: a folder per tag, a request per operation,
//! `{{base_url}}` and one environment per server, path parameters as
//! variables, and security schemes as auth that points at secrets

use serde_json::Value;

use super::schema::{as_text, example, skeleton};
use super::{SpecError, Version, fingerprint, resolve, version};
use crate::curl::split_scheme;
use crate::model::{Auth, Body, KeyValue, Method, Request, SpecOrigin, Variable};
use crate::secrets::detect::{self, Place};
use crate::store::bundle::{Bundle, BundleItem};
use crate::vars::VarName;

/// operations past this many are left out
const MAX_OPERATIONS: usize = 5000;

const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];

/// what a spec became, and what the preview shows before it is written
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    /// the collection, ready to be written
    pub bundle: Bundle,

    /// how many requests and folders it holds
    pub requests: usize,

    /// how many folders it holds
    pub folders: usize,

    /// the secrets it asks for, by name
    pub secrets: Vec<String>,

    /// example values that looked like credentials, each as where it was;
    /// the values are left out and a placeholder takes their place
    pub credentials: Vec<String>,
}

/// what is built while the spec is read
#[derive(Default)]
struct Building {
    variables: Vec<Variable>,
    warnings: Vec<String>,
    credentials: Vec<String>,

    /// what the spec as a whole asks for, which the collection passes on
    /// to operations that say nothing of their own
    auth: Option<Auth>,
}

impl Building {
    fn warn(&mut self, warning: String) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    /// a variable named after `hint`, made once; a secret has no value
    fn variable(&mut self, hint: &str, value: &str, secret: bool) -> VarName {
        let name = VarName::from_hint(hint);

        if !self.variables.iter().any(|v| v.name == name) {
            self.variables.push(if secret {
                Variable::secret(name.clone())
            } else {
                Variable::new(name.clone(), value)
            });
        }

        name
    }
}

/// the base URL each server gives, its `{variables}` set to their defaults
fn servers(
    root: &Value,
    version: Version,
    origin: Option<&str>,
    building: &mut Building,
) -> Vec<(String, String)> {
    let mut found = Vec::new();

    match version {
        Version::Swagger2 => {
            let host = root.get("host").and_then(Value::as_str);
            let base = root.get("basePath").and_then(Value::as_str).unwrap_or("");
            let scheme = root
                .get("schemes")
                .and_then(Value::as_array)
                .and_then(|s| {
                    s.iter()
                        .filter_map(Value::as_str)
                        .find(|s| *s == "https")
                        .or_else(|| s.first().and_then(Value::as_str))
                })
                .unwrap_or("https");

            match host {
                Some(host) => found.push((host.to_owned(), format!("{scheme}://{host}{base}"))),
                None => found.push(("server".into(), base.to_owned())),
            }
        }
        Version::OpenApi3 => {
            for server in root
                .get("servers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(url) = server.get("url").and_then(Value::as_str) else {
                    continue;
                };

                let mut url = url.to_owned();

                for (name, variable) in server
                    .get("variables")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    let default = variable.get("default").map_or_else(String::new, as_text);

                    url = url.replace(&format!("{{{name}}}"), &default);
                }

                // without a description, the server is named by its URL,
                // once a relative one is resolved
                let label = server
                    .get("description")
                    .and_then(Value::as_str)
                    .map_or_else(String::new, ToOwned::to_owned);

                found.push((label, url));
            }
        }
    }

    // a relative server is relative to where the spec came from
    for (_, url) in &mut found {
        if url.contains("://") {
            continue;
        }

        match origin
            .and_then(|origin| crate::url::resolve(origin, if url.is_empty() { "/" } else { url }))
        {
            Some(absolute) => *url = absolute,
            None => building.warn(format!(
                "the server {url:?} is relative; set base_url to where the API is"
            )),
        }
    }

    for (label, url) in &mut found {
        while url.ends_with('/') {
            url.pop();
        }

        if label.is_empty() {
            label.clone_from(url);
        }
    }

    found
}

/// how one security scheme shows in a request
enum Scheme {
    Header(String, String),
    Query(String, String),
    Cookie(String, String),
    Bearer(String),
    Basic(String, String),
}

/// the scheme a security requirement names, as placeholders for secrets
fn scheme(root: &Value, version: Version, key: &str, building: &mut Building) -> Option<Scheme> {
    let schemes = match version {
        Version::Swagger2 => root.get("securityDefinitions"),
        Version::OpenApi3 => root
            .get("components")
            .and_then(|c| c.get("securitySchemes")),
    }?;
    let scheme = resolve(root, schemes.get(key)?)?;
    let kind = scheme.get("type").and_then(Value::as_str)?;
    let placeholder = |name: &VarName| format!("{{{{{name}}}}}");

    Some(match kind {
        "apiKey" => {
            let name = scheme.get("name").and_then(Value::as_str)?.to_owned();
            let variable = building.variable(&name, "", true);

            match scheme.get("in").and_then(Value::as_str) {
                Some("query") => Scheme::Query(name, placeholder(&variable)),
                Some("cookie") => Scheme::Cookie(name, placeholder(&variable)),
                _ => Scheme::Header(name, placeholder(&variable)),
            }
        }
        "basic" => basic(building),
        "http" => match scheme
            .get("scheme")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("basic") => basic(building),
            _ => Scheme::Bearer(placeholder(&building.variable("token", "", true))),
        },
        "oauth2" | "openIdConnect" => {
            building.warn(format!(
                "{key} is {kind}: curst sends {{{{access_token}}}} as a bearer token; get one and set it"
            ));

            Scheme::Bearer(placeholder(&building.variable("access_token", "", true)))
        }
        _ => return None,
    })
}

fn basic(building: &mut Building) -> Scheme {
    let username = building.variable("username", "", false);
    let password = building.variable("password", "", true);

    Scheme::Basic(format!("{{{{{username}}}}}"), format!("{{{{{password}}}}}"))
}

/// a folder in the making: its name and requests
struct Folder {
    name: String,
    items: Vec<BundleItem>,
}

/// the operation's parameters with its path's, the operation's own winning
fn parameters<'a>(root: &'a Value, path_item: &'a Value, operation: &'a Value) -> Vec<&'a Value> {
    let mut list: Vec<&Value> = Vec::new();

    for holder in [path_item, operation] {
        for parameter in holder
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| resolve(root, p))
        {
            let key = |p: &Value| (p.get("name").cloned(), p.get("in").cloned());

            list.retain(|old| key(old) != key(parameter));
            list.push(parameter);
        }
    }

    list
}

/// the body an operation sends, from its first content type curst can make
fn body_3(root: &Value, operation: &Value, building: &mut Building, label: &str) -> Body {
    let Some(body) = operation.get("requestBody").and_then(|b| resolve(root, b)) else {
        return Body::None;
    };
    let Some(content) = body.get("content").and_then(Value::as_object) else {
        return Body::None;
    };

    let pick = |wanted: &dyn Fn(&str) -> bool| {
        content
            .iter()
            .find(|(kind, _)| wanted(&kind.to_ascii_lowercase()))
    };
    let json = pick(&|k| k == "application/json" || k.ends_with("+json") || k == "*/*");
    let form = pick(&|k| k == "application/x-www-form-urlencoded");
    let multipart = pick(&|k| k == "multipart/form-data");

    if let Some((_, media)) = json {
        return Body::Json(json_text(root, media));
    }

    if let Some((_, media)) = form.or(multipart) {
        if multipart.is_some() && form.is_none() {
            building.warn(format!(
                "{label}: multipart bodies are sent as plain form fields, and files cannot be attached yet"
            ));
        }

        return Body::Form(form_fields(root, media.get("schema")));
    }

    match content.iter().next() {
        Some((kind, media)) => Body::Text {
            content_type: kind.clone(),
            text: example(root, media).map_or_else(String::new, |e| as_text(&e)),
        },
        None => Body::None,
    }
}

/// a JSON body's text: the example the spec gives, or one made from the
/// schema, pretty-printed
fn json_text(root: &Value, media: &Value) -> String {
    let value = example(root, media).unwrap_or_else(|| {
        media
            .get("schema")
            .map_or(Value::Object(serde_json::Map::new()), |s| skeleton(root, s))
    });

    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// a form's fields, from the properties of its schema
fn form_fields(root: &Value, schema: Option<&Value>) -> Vec<KeyValue> {
    let Some(schema) = schema.and_then(|s| resolve(root, s)) else {
        return Vec::new();
    };

    schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, property)| {
            let value = resolve(root, property)
                .and_then(|p| example(root, p))
                .map_or_else(String::new, |e| as_text(&e));

            KeyValue::new(name, value)
        })
        .collect()
}

/// takes out every value of `request` that looks like a credential: the
/// value is dropped, never kept, and a secret placeholder takes its place
fn without_credentials(request: &mut Request, label: &str, building: &mut Building) {
    for finding in detect::detect(request) {
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
            Place::UrlUserInfo | Place::UrlQuery(_) => continue,
        };

        let variable = building.variable(&hint, "", true);
        let (scheme, _) = split_scheme(slot);

        *slot = format!("{scheme}{{{{{variable}}}}}");
        building
            .credentials
            .push(format!("{label}: {hint}, {}", finding.reason.describe()));
    }
}

/// what an operation's security requirement puts in a request
#[derive(Default)]
struct Secured {
    params: Vec<KeyValue>,
    headers: Vec<KeyValue>,
    cookies: Vec<String>,
    auth: Auth,

    /// the parameters the schemes stand for, as their lowercase name and
    /// where they go
    taken: Vec<(String, &'static str)>,
}

/// the first security requirement of an operation, or the spec's, as
/// placeholders for secrets
fn secured(
    root: &Value,
    version: Version,
    operation: &Value,
    label: &str,
    building: &mut Building,
) -> Secured {
    let requirement = operation
        .get("security")
        .or_else(|| root.get("security"))
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_object);
    let mut out = Secured::default();

    for key in requirement.into_iter().flat_map(|r| r.keys()) {
        match scheme(root, version, key, building) {
            Some(Scheme::Header(name, value)) => {
                out.taken.push((name.to_ascii_lowercase(), "header"));
                out.headers.push(KeyValue::new(name, value));
            }
            Some(Scheme::Query(name, value)) => {
                out.taken.push((name.to_ascii_lowercase(), "query"));
                out.params.push(KeyValue::new(name, value));
            }
            Some(Scheme::Cookie(name, value)) => {
                out.taken.push((name.to_ascii_lowercase(), "cookie"));
                out.cookies.push(format!("{name}={value}"));
            }
            Some(Scheme::Bearer(token)) => out.auth = Auth::Bearer { token },
            Some(Scheme::Basic(username, password)) => {
                out.auth = Auth::Basic { username, password };
            }
            None => building.warn(format!(
                "{label}: the security scheme {key} is not one curst knows"
            )),
        }
    }

    out
}

/// one operation, where it is in the spec
struct Located<'a> {
    path: &'a str,
    path_item: &'a Value,
    method: &'a str,
    operation: &'a Value,
    label: &'a str,
}

/// one operation as a request
fn request(root: &Value, version: Version, op: &Located, building: &mut Building) -> Request {
    let Located {
        path,
        path_item,
        method,
        operation,
        label,
    } = *op;
    let mut url = format!("{{{{base_url}}}}{path}");
    let mut form = Vec::new();
    let mut body = Body::None;

    // the security schemes go in first, so a parameter of the same name
    // never brings in its example
    let Secured {
        mut params,
        mut headers,
        mut cookies,
        auth,
        taken,
    } = secured(root, version, operation, label, building);

    // an operation with its own requirement says exactly what it uses;
    // one without inherits the spec's, set on the collection
    let auth = if operation.get("security").is_some() {
        match auth {
            Auth::Inherit => Auth::None,
            auth => auth,
        }
    } else {
        if auth != Auth::Inherit {
            building.auth.get_or_insert(auth);
        }

        Auth::Inherit
    };

    for parameter in parameters(root, path_item, operation) {
        let (Some(name), Some(place)) = (
            parameter.get("name").and_then(Value::as_str),
            parameter.get("in").and_then(Value::as_str),
        ) else {
            continue;
        };

        if taken
            .iter()
            .any(|(n, p)| *n == name.to_ascii_lowercase() && *p == place)
        {
            continue;
        }

        let required = parameter
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(place == "path");
        let value = example(root, parameter).map_or_else(String::new, |e| as_text(&e));
        let pair = KeyValue {
            name: name.to_owned(),
            value: value.clone(),
            enabled: required,
        };

        match place {
            "path" => {
                let variable = building.variable(name, &value, false);

                url = url.replace(&format!("{{{name}}}"), &format!("{{{{{variable}}}}}"));
            }
            "query" => params.push(pair),
            "header" => {
                // these come from the body's kind and the auth
                if !["accept", "content-type", "authorization"]
                    .contains(&name.to_ascii_lowercase().as_str())
                {
                    headers.push(pair);
                }
            }
            "cookie" => cookies.push(format!("{name}={value}")),
            "formData" => form.push(pair),
            "body" => {
                let media = serde_json::json!({ "schema": parameter.get("schema").cloned().unwrap_or(Value::Null) });

                body = Body::Json(json_text(root, &media));
            }
            _ => {}
        }
    }

    if !cookies.is_empty() {
        headers.push(KeyValue::new("Cookie", cookies.join("; ")));
    }

    if !form.is_empty() {
        body = Body::Form(form);
    }

    if version == Version::OpenApi3 {
        body = body_3(root, operation, building, label);
    }

    let mut request = Request {
        method: method.parse().unwrap_or(Method::Get),
        url,
        params,
        headers,
        body,
        auth,
    };

    without_credentials(&mut request, label, building);

    request
}

/// every operation as a request: in a folder for its first tag, or on
/// their own, and how many there are
fn operations(
    root: &Value,
    version: Version,
    building: &mut Building,
) -> (Vec<Folder>, Vec<BundleItem>, usize) {
    let mut folders: Vec<Folder> = Vec::new();
    let mut top: Vec<BundleItem> = Vec::new();
    let mut count = 0;

    let paths = root.get("paths").and_then(Value::as_object);

    'paths: for (path, item) in paths.into_iter().flatten() {
        let Some(path_item) = resolve(root, item) else {
            building.warn(format!(
                "{path} refers to another file, which curst does not read"
            ));

            continue;
        };

        for method in METHODS {
            let Some(operation) = path_item.get(method) else {
                continue;
            };

            if count == MAX_OPERATIONS {
                building.warn(format!(
                    "only the first {MAX_OPERATIONS} operations were imported"
                ));

                break 'paths;
            }

            count += 1;

            let operation_key = format!("{} {path}", method.to_ascii_uppercase());
            let label = operation
                .get("summary")
                .and_then(Value::as_str)
                .or_else(|| operation.get("operationId").and_then(Value::as_str))
                .filter(|s| !s.trim().is_empty())
                .map_or_else(|| operation_key.clone(), |s| s.trim().to_owned());
            let op = Located {
                path,
                path_item,
                method,
                operation,
                label: &label,
            };
            let request = request(root, version, &op, building);
            let item = BundleItem::Request {
                name: label,
                request,
                spec: Some(SpecOrigin {
                    operation: operation_key,
                    fingerprint: fingerprint(root, path_item, operation),
                }),
            };
            let tag = operation
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str);

            match tag {
                Some(tag) => match folders.iter_mut().find(|f| f.name == tag) {
                    Some(folder) => folder.items.push(item),
                    None => folders.push(Folder {
                        name: tag.to_owned(),
                        items: vec![item],
                    }),
                },
                None => top.push(item),
            }
        }
    }

    (folders, top, count)
}

/// a spec as a collection; `origin` is where it was fetched, which a
/// relative server is resolved against
///
/// # Errors
/// when it is not a spec of a version curst reads
pub fn convert(root: &Value, origin: Option<&str>) -> Result<Converted, SpecError> {
    let version = version(root)?;
    let mut building = Building::default();
    let name = root
        .get("info")
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Imported API")
        .to_owned();
    let found = servers(root, version, origin, &mut building);
    let base = found
        .first()
        .map_or_else(String::new, |(_, url)| url.clone());
    building.variable("base_url", &base, false);

    let (folders, top, count) = operations(root, version, &mut building);

    // each server is an environment holding every variable, base_url set
    // to that server, so staging and production keep their own values and
    // secrets; a spec without servers keeps them in the collection
    let secrets = building
        .variables
        .iter()
        .filter(|v| v.secret)
        .map(|v| v.name.to_string())
        .collect();
    let (variables, environments) = if found.is_empty() {
        (building.variables, Vec::new())
    } else {
        let environments = found
            .iter()
            .map(|(label, url)| {
                let variables = building
                    .variables
                    .iter()
                    .map(|v| {
                        if v.name.as_str() == "base_url" {
                            Variable::new(v.name.clone(), url.clone())
                        } else {
                            v.clone()
                        }
                    })
                    .collect();

                (label.clone(), variables)
            })
            .collect();

        (Vec::new(), environments)
    };
    let folder_count = folders.len();
    let mut items: Vec<BundleItem> = folders
        .into_iter()
        .map(|f| BundleItem::Folder {
            name: f.name,
            auth: Auth::Inherit,
            items: f.items,
        })
        .collect();

    items.extend(top);

    Ok(Converted {
        bundle: Bundle {
            name,
            variables,
            environments,
            items,
            auth: building.auth.unwrap_or_default(),
            workflows: Vec::new(),
            warnings: building.warnings,
        },
        requests: count,
        folders: folder_count,
        secrets,
        credentials: building.credentials,
    })
}
