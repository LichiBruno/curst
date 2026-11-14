//! `curst run`: one saved request, sent as the UI would send it, with its
//! collection's variables, its inherited auth and its secrets

use std::collections::HashMap;
use std::io::Write;

use curst_core::http::{Cancel, HttpExecutor, RedirectRule, Response};
use curst_core::model::{EnvId, Item, LocalId, SavedRequest};
use curst_core::secrets::{Secret, SecretIndex, SecretKey};
use curst_core::store::Library;
use curst_core::vars::{Origin, Resolution, Scopes, SecretValues, VarName, resolve_request};

use super::{Ask, Fail, RunArgs};
use crate::app::Workspace;

/// the request at `path` under `items`, as the slugs that reach it; each
/// part is a name or a slug, and a name may hold a `/` of its own, as
/// `GET /pets` does
fn find<'a>(items: &'a [Item], path: &str, slugs: &mut Vec<String>) -> Option<&'a SavedRequest> {
    for item in items {
        for label in [item.name(), item.slug()] {
            let rest = if path.eq_ignore_ascii_case(label) {
                Some("")
            } else {
                path.get(..label.len())
                    .filter(|head| head.eq_ignore_ascii_case(label))
                    .and_then(|_| path[label.len()..].strip_prefix('/'))
            };
            let Some(rest) = rest else {
                continue;
            };

            slugs.push(item.slug().to_owned());

            let found = match item {
                Item::Request(saved) if rest.is_empty() => Some(saved),
                Item::Folder(folder) if !rest.is_empty() => find(&folder.items, rest, slugs),
                _ => None,
            };

            if found.is_some() {
                return found;
            }

            slugs.pop();
        }
    }

    None
}

/// the collection and the request `path` names
fn locate<'a>(
    collections: &'a [Workspace],
    path: &str,
) -> Result<(&'a Workspace, Vec<String>, &'a SavedRequest), Fail> {
    let path = path.trim().trim_matches('/');

    for workspace in collections {
        let name = &workspace.collection.name;
        let rest = path
            .get(..name.len())
            .filter(|head| head.eq_ignore_ascii_case(name))
            .and_then(|_| path[name.len()..].strip_prefix('/'));
        let mut slugs = Vec::new();

        if let Some(saved) =
            rest.and_then(|rest| find(&workspace.collection.items, rest, &mut slugs))
        {
            return Ok((workspace, slugs, saved));
        }
    }

    let names: Vec<&str> = collections
        .iter()
        .map(|w| w.collection.name.as_str())
        .collect();

    Err(Fail::setup(format!(
        "no saved request is at {path}; give it as collection/folder/request, the collections are: {}",
        names.join(", ")
    )))
}

/// secret values fetched for this run
struct Values {
    fetched: HashMap<SecretKey, Secret>,
    collection: LocalId,
    environment: Option<EnvId>,
}

impl SecretValues for Values {
    fn secret(&self, origin: Origin, name: &VarName) -> Option<&str> {
        let key = SecretKey::for_variable(
            origin,
            Some(&self.collection),
            self.environment.as_ref(),
            name.clone(),
        )?;

        self.fetched.get(&key).map(Secret::expose)
    }
}

/// the answer to the request, or why it could not be sent
pub(crate) enum Answer {
    Response(Box<Response>),
    Failed(String),
}

impl Answer {
    /// the exit code: 0 for 1xx to 3xx, 1 for 4xx and 5xx, 3 for a request
    /// that could not be sent
    pub(crate) fn code(&self) -> u8 {
        match self {
            Self::Response(response) if response.status >= 400 => 1,
            Self::Response(_) => 0,
            Self::Failed(_) => 3,
        }
    }

    /// the body on `out` and the status on `err`, or everything as JSON
    /// on `out`
    pub(crate) fn print(
        &self,
        out: &mut impl Write,
        err: &mut impl Write,
        json: bool,
    ) -> std::io::Result<()> {
        let response = match self {
            Self::Failed(why) => return writeln!(err, "curst: not sent: {why}"),
            Self::Response(response) => response,
        };
        let total = response.timings.total.as_millis();

        if json {
            let headers: Vec<[&str; 2]> = response
                .headers
                .iter()
                .map(|(name, value)| [name.as_str(), value.as_str()])
                .collect();
            let value = serde_json::json!({
                "status": response.status,
                "status_line": response.status_line,
                "url": response.effective_url,
                "redirects": response.redirects,
                "headers": headers,
                "timings_ms": {
                    "dns": response.timings.dns.as_millis(),
                    "connect": response.timings.connect.as_millis(),
                    "tls": response.timings.tls.as_millis(),
                    "first_byte": response.timings.first_byte.as_millis(),
                    "total": total,
                },
                "body": String::from_utf8_lossy(response.body.preview()),
                "body_truncated": response.body.is_truncated(),
            });

            return writeln!(out, "{value}");
        }

        out.write_all(response.body.preview())?;
        out.flush()?;
        writeln!(err, "{} · {total} ms", response.status_line.trim())?;

        if response.body.is_truncated() {
            writeln!(
                err,
                "curst: the body was cut at what curst keeps in memory; raise http_max_body_mb in config.toml for all of it"
            )?;
        }

        if let Some(stopped) = &response.stopped {
            writeln!(
                err,
                "curst: the redirect to {} was not followed, as the request's secrets are not approved there",
                stopped.url
            )?;
        }

        Ok(())
    }
}

/// whether `origin`, as `https://host:443`, is one of the hosts given
fn allowed(origin: &str, hosts: &[String]) -> bool {
    let host = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .rsplit_once(':')
        .map_or(origin, |(host, _)| host);

    hosts.iter().any(|given| {
        let given = given.trim().trim_end_matches('/');

        given.eq_ignore_ascii_case(host) || given.eq_ignore_ascii_case(origin)
    })
}

/// how far the request's redirects may go: within the origins its secrets
/// are approved for; with `--allow-host`, within the request's own origin
fn redirect_rule(
    index: &SecretIndex,
    secrets: &[SecretKey],
    url: &str,
    hosts: &[String],
) -> Result<RedirectRule, Fail> {
    let refused = |e: &dyn std::fmt::Display| Fail::setup(format!("the secrets index: {e}"));

    match index.unapproved(secrets, url) {
        Ok(None) => index.redirect_rule(secrets).map_err(|e| refused(&e)),
        Ok(Some(approval)) if allowed(&approval.origin, hosts) => Ok(RedirectRule::Within(
            curst_core::url::origin(url).into_iter().collect(),
        )),
        Ok(Some(approval)) => {
            let names: Vec<String> = approval
                .keys
                .iter()
                .filter_map(|k| k.name().map(|n| format!("{{{{{n}}}}}")))
                .collect();

            Err(Fail::setup(format!(
                "{} would go to {}, which is not approved for {}: approve it once in curst, or pass --allow-host {}",
                names.join(", "),
                approval.origin,
                if names.len() == 1 { "it" } else { "them" },
                approval
                    .origin
                    .split_once("://")
                    .map_or(approval.origin.as_str(), |(_, rest)| rest)
                    .rsplit_once(':')
                    .map_or(approval.origin.as_str(), |(host, _)| host),
            )))
        }
        Err(e) => Err(refused(&e)),
    }
}

/// why a request with variables that are not defined was not sent, and
/// the usual reason: no environment chosen
fn undefined(workspace: &Workspace, unchosen: bool, resolution: &Resolution) -> Fail {
    let names: Vec<String> = resolution
        .missing
        .iter()
        .chain(&resolution.cyclic)
        .map(|n| format!("{{{{{n}}}}}"))
        .collect();
    let environments: Vec<&str> = workspace
        .collection
        .environments
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    let mut message = format!(
        "not sent: {} not defined, or defined in terms of themselves",
        names.join(", ")
    );

    if unchosen && !environments.is_empty() {
        message.push_str("; no environment is chosen, pick one with --env: ");
        message.push_str(&environments.join(", "));
    }

    Fail::setup(message)
}

/// the environment `wanted` names, or the one chosen in curst
fn environment<'a>(
    workspace: &'a Workspace,
    wanted: Option<&str>,
) -> Result<Option<&'a curst_core::model::Environment>, Fail> {
    let collection = &workspace.collection;
    let Some(wanted) = wanted else {
        return Ok(workspace
            .active_environment
            .as_ref()
            .and_then(|id| collection.environment(id)));
    };

    collection
        .environments
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(wanted.trim()))
        .map(Some)
        .ok_or_else(|| {
            let names: Vec<&str> = collection
                .environments
                .iter()
                .map(|e| e.name.as_str())
                .collect();

            Fail::setup(format!(
                "{} has no environment called {wanted}; it has: {}",
                collection.name,
                names.join(", ")
            ))
        })
}

/// sends the request `args` names and gives its answer
pub(crate) fn run(
    library: &Library,
    executor: &dyn HttpExecutor,
    args: &RunArgs,
    ask: Ask<'_>,
) -> Result<Answer, Fail> {
    let (collections, globals) = super::load(library)?;
    let (workspace, slugs, saved) = locate(&collections, &args.request)?;
    let name = &workspace.collection.name;

    if !workspace.trusted {
        return Err(Fail::setup(format!(
            "{name} is not trusted, so nothing in it is sent: trust it in curst first"
        )));
    }

    let environment = environment(workspace, args.env.as_deref())?;
    let request = workspace
        .collection
        .with_inherited_auth(&slugs, &saved.request);
    let mut values = Values {
        fetched: HashMap::new(),
        collection: workspace.id.clone(),
        environment: environment.map(|e| e.id.clone()),
    };
    let resolve = |values: &Values| {
        let scopes = Scopes {
            environment: environment.map_or(&[], |e| e.variables.as_slice()),
            collection: &workspace.collection.variables,
            global: &globals,
            secrets: Some(values),
        };

        resolve_request(&request, &scopes)
    };
    let (_, first) = resolve(&values);
    if !first.missing.is_empty() || !first.cyclic.is_empty() {
        return Err(undefined(workspace, environment.is_none(), &first));
    }

    // the secrets it needs, from the store
    let keys: Vec<SecretKey> = first
        .unavailable
        .iter()
        .filter_map(|(origin, name)| {
            SecretKey::for_variable(
                *origin,
                Some(&values.collection),
                values.environment.as_ref(),
                name.clone(),
            )
        })
        .collect();

    if !keys.is_empty() {
        let store = super::open_store(library.paths(), ask)?;

        for key in keys {
            let value = store
                .get(&key)
                .map_err(|e| Fail::setup(format!("the secrets could not be read: {e}")))?;
            let Some(value) = value else {
                let name = key.name().map_or_else(String::new, ToString::to_string);

                return Err(Fail::setup(format!(
                    "{{{{{name}}}}} has no value yet: set it in curst first"
                )));
            };

            values.fetched.insert(key, value);
        }
    }

    let (resolved, resolution) = resolve(&values);
    let secrets: Vec<SecretKey> = resolution
        .secrets
        .iter()
        .filter_map(|(origin, name)| {
            SecretKey::for_variable(
                *origin,
                Some(&values.collection),
                values.environment.as_ref(),
                name.clone(),
            )
        })
        .collect();
    let rule = if secrets.is_empty() {
        RedirectRule::Any
    } else {
        let index = SecretIndex::new(library.paths().secrets_index_file());

        redirect_rule(&index, &secrets, &resolved.full_url(), &args.allow_host)?
    };

    Ok(match executor.execute(&resolved, &Cancel::new(), &rule) {
        Ok(response) => Answer::Response(Box::new(response)),
        Err(err) => Answer::Failed(err.to_string()),
    })
}
