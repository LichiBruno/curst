//! refreshing a collection from its spec: which endpoints are new, which
//! the spec changed, which it no longer has, and applying the ones the
//! user chose
//!
//! a request counts as changed only when the spec's operation changed, as
//! its fingerprint says: what the user edited in it never counts, and is
//! only replaced when the user says so

use crate::model::{Auth, Body, Collection, Item, KeyValue, Request, SpecOrigin, Variable};
use crate::store::bundle::BundleItem;
use crate::store::{CollectionDir, CollectionStore, StoreError};

use super::Converted;

/// an endpoint the spec has and the collection does not
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// the folder it goes in, named after its tag, or `None` for the top
    pub folder: Option<String>,
    /// the name it goes by
    pub name: String,
    /// the request the spec makes
    pub request: Request,
    /// the operation and its fingerprint now
    pub spec: SpecOrigin,
}

/// an endpoint the spec describes differently than when it was imported
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    /// where the request is, as slugs
    pub path: Vec<String>,

    /// the name the user knows it by
    pub name: String,
    /// the request as it is now
    pub current: Request,
    /// the request as the spec makes it now
    pub new: Request,
    /// the operation and its fingerprint now
    pub spec: SpecOrigin,
}

/// an endpoint the spec no longer has
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// where the request is, as slugs
    pub path: Vec<String>,
    /// the name it goes by
    pub name: String,
    /// the operation, as `GET /pets`
    pub operation: String,
}

/// what a refresh would do, before the user chooses
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// endpoints the spec has and the collection does not
    pub added: Vec<Added>,
    /// endpoints the spec describes differently now
    pub changed: Vec<Changed>,
    /// endpoints the spec no longer has
    pub removed: Vec<Removed>,

    /// endpoints the spec describes as before
    pub unchanged: usize,

    /// variables the new endpoints use that the collection does not have
    pub variables: Vec<Variable>,
}

impl Plan {
    /// whether there is nothing to do
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

/// every request of a collection that came from a spec, with its path
fn origins(
    items: &[Item],
    parent: &[String],
    found: &mut Vec<(Vec<String>, String, Request, SpecOrigin)>,
) {
    for item in items {
        let mut path = parent.to_vec();

        match item {
            Item::Folder(folder) => {
                path.push(folder.slug.clone());
                origins(&folder.items, &path, found);
            }
            Item::Request(saved) => {
                if let Some(spec) = &saved.spec {
                    path.push(saved.slug.clone());
                    found.push((
                        path,
                        saved.name.clone(),
                        saved.request.clone(),
                        spec.clone(),
                    ));
                }
            }
        }
    }
}

/// every request a converted spec holds, with the folder it goes in
fn generated(items: &[BundleItem], folder: Option<&str>, found: &mut Vec<Added>) {
    for item in items {
        match item {
            BundleItem::Folder { name, items, .. } => generated(items, Some(name), found),
            BundleItem::Request {
                name,
                request,
                spec: Some(spec),
            } => found.push(Added {
                folder: folder.map(ToOwned::to_owned),
                name: name.clone(),
                request: request.clone(),
                spec: spec.clone(),
            }),
            BundleItem::Request { spec: None, .. } => {}
        }
    }
}

/// what refreshing `collection` from `converted` would do
#[must_use]
pub fn plan(collection: &Collection, converted: &Converted) -> Plan {
    let mut existing = Vec::new();
    let mut fresh = Vec::new();

    origins(&collection.items, &[], &mut existing);
    generated(&converted.bundle.items, None, &mut fresh);

    // a collection imported before auth was inherited sets none: what the
    // spec as a whole asks for goes on each request, as it did then
    if collection.auth == Auth::Inherit && converted.bundle.auth != Auth::Inherit {
        for new in &mut fresh {
            if new.request.auth == Auth::Inherit {
                new.request.auth = converted.bundle.auth.clone();
            }
        }
    }

    let mut plan = Plan::default();

    for (path, name, current, spec) in &existing {
        match fresh.iter().find(|f| f.spec.operation == spec.operation) {
            Some(new) if new.spec.fingerprint == spec.fingerprint => plan.unchanged += 1,
            Some(new) => plan.changed.push(Changed {
                path: path.clone(),
                name: name.clone(),
                current: current.clone(),
                new: new.request.clone(),
                spec: new.spec.clone(),
            }),
            None => plan.removed.push(Removed {
                path: path.clone(),
                name: name.clone(),
                operation: spec.operation.clone(),
            }),
        }
    }

    for new in fresh {
        if !existing
            .iter()
            .any(|(_, _, _, spec)| spec.operation == new.spec.operation)
        {
            plan.added.push(new);
        }
    }

    // the variables the spec now names that no scope of the collection has
    let known = |name: &str| {
        collection.variables.iter().any(|v| v.name.as_str() == name)
            || collection
                .environments
                .iter()
                .any(|e| e.variables.iter().any(|v| v.name.as_str() == name))
    };
    let spec_variables = converted
        .bundle
        .environments
        .first()
        .map_or(&converted.bundle.variables, |(_, variables)| variables);

    plan.variables = spec_variables
        .iter()
        .filter(|v| !known(v.name.as_str()))
        .cloned()
        .collect();

    plan
}

/// a request as lines of text, for a diff: every part, rows that are off
/// included, so a change in any of them shows
#[must_use]
pub fn request_lines(request: &Request) -> Vec<String> {
    let mut lines = vec![format!("{} {}", request.method.as_str(), request.url)];
    let row = |kind: &str, pair: &KeyValue, separator: &str| {
        let off = if pair.enabled { "" } else { "  (off)" };

        format!("{kind} {}{separator}{}{off}", pair.name, pair.value)
    };

    lines.extend(request.params.iter().map(|p| row("param", p, "=")));
    lines.extend(request.headers.iter().map(|h| row("header", h, ": ")));

    match &request.auth {
        Auth::Inherit => {}
        Auth::None => lines.push("auth none".into()),
        Auth::Basic { username, password } => {
            lines.push(format!("auth basic {username} : {password}"));
        }
        Auth::Bearer { token } => lines.push(format!("auth bearer {token}")),
    }

    match &request.body {
        Body::None => {}
        Body::Json(text) => {
            lines.push("body json".to_owned());
            lines.extend(text.lines().map(|l| format!("  {l}")));
        }
        Body::Text { content_type, text } => {
            lines.push(format!("body {content_type}"));
            lines.extend(text.lines().map(|l| format!("  {l}")));
        }
        Body::Form(fields) => {
            lines.push("body form".to_owned());
            lines.extend(fields.iter().map(|f| row("  field", f, "=")));
        }
    }

    lines
}

/// one line of a diff
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// in both
    Same(String),

    /// only in what is there now
    Removed(String),

    /// only in what the spec says
    Added(String),
}

/// the lines of `old` and `new`, those that differ marked, in order: the
/// longest common subsequence, as `diff` does
#[must_use]
pub fn diff(old: &[String], new: &[String]) -> Vec<DiffLine> {
    let (n, m) = (old.len(), new.len());
    let mut common = vec![vec![0usize; m + 1]; n + 1];

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }

    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();

    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            lines.push(DiffLine::Same(old[i].clone()));
            i += 1;
            j += 1;
        // on a tie the old line goes first, as `diff` shows a change
        } else if j < m && (i == n || common[i][j + 1] > common[i + 1][j]) {
            lines.push(DiffLine::Added(new[j].clone()));
            j += 1;
        } else {
            lines.push(DiffLine::Removed(old[i].clone()));
            i += 1;
        }
    }

    lines
}

/// which parts of a plan the user chose, each by its position in its list
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Choices {
    /// new endpoints to add
    pub add: Vec<usize>,
    /// changed endpoints to replace
    pub replace: Vec<usize>,
    /// removed endpoints to delete
    pub delete: Vec<usize>,
}

/// writes the chosen parts of `plan` to the collection: new endpoints in
/// the folder of their tag, made when missing; changed ones replaced in
/// place, keeping their name; removed ones deleted; and the variables the
/// new endpoints need, in every environment or in the collection
///
/// # Errors
/// when a file cannot be written; what was written before stays
pub fn apply(
    dir: &CollectionDir,
    collection: &Collection,
    plan: &Plan,
    choices: &Choices,
) -> Result<(), StoreError> {
    let mut folders: Vec<(String, String)> = collection
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Folder(folder) => Some((folder.name.clone(), folder.slug.clone())),
            Item::Request(_) => None,
        })
        .collect();

    for added in choices.add.iter().filter_map(|i| plan.added.get(*i)) {
        let parent = match &added.folder {
            None => Vec::new(),
            Some(name) => {
                let slug = if let Some((_, slug)) = folders.iter().find(|(n, _)| n == name) {
                    slug.clone()
                } else {
                    let slug = dir.create_folder(&[], name)?;

                    folders.push((name.clone(), slug.clone()));

                    slug
                };

                vec![slug]
            }
        };

        dir.create_from_spec(&parent, &added.name, &added.request, Some(&added.spec))?;
    }

    for changed in choices.replace.iter().filter_map(|i| plan.changed.get(*i)) {
        dir.replace_from_spec(&changed.path, &changed.new, &changed.spec)?;
    }

    for removed in choices.delete.iter().filter_map(|i| plan.removed.get(*i)) {
        dir.delete(&removed.path)?;
    }

    // the variables new endpoints need, where the others are
    let needed: Vec<&Variable> = plan
        .variables
        .iter()
        .filter(|_| !choices.add.is_empty())
        .collect();

    if needed.is_empty() {
        return Ok(());
    }

    if collection.environments.is_empty() {
        let mut variables = collection.variables.clone();

        variables.extend(needed.into_iter().cloned());

        return dir.save_settings(&collection.name, &variables);
    }

    for environment in &collection.environments {
        let mut environment = environment.clone();

        environment
            .variables
            .extend(needed.iter().map(|v| (*v).clone()));
        dir.save_environment(&environment)?;
    }

    Ok(())
}

/// why a collection's spec could not be loaded again
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// the file or folder is not there any more, or no longer holds the
    /// spec picked in it: the user is asked where it is now
    Missing(String),

    /// anything else, such as a spec that no longer parses
    Failed(String),
}

/// the spec a source points to, and where it was found, which a relative
/// server is resolved against; `fetch` fetches a URL
///
/// # Errors
/// [`LoadError::Missing`] when the file or folder is gone, otherwise
/// [`LoadError::Failed`]
pub fn load_source(
    source: &crate::model::Source,
    fetch: impl FnMut(&str) -> Result<super::Fetched, String>,
) -> Result<(serde_json::Value, Option<String>), LoadError> {
    let location = source.location.trim();
    let lower = location.to_ascii_lowercase();

    if lower.starts_with("http://") || lower.starts_with("https://") {
        let (value, found_at) = super::discover(location, fetch).map_err(LoadError::Failed)?;

        return Ok((value, Some(found_at)));
    }

    let path = std::path::Path::new(location);

    if !path.exists() {
        return Err(LoadError::Missing(format!(
            "there is nothing at {location} any more"
        )));
    }

    let file = if path.is_dir() {
        let specs = super::specs_in(path);
        let picked = match (&source.picked, specs.len()) {
            (Some(name), _) if specs.contains(name) => name.clone(),
            (Some(name), _) => {
                return Err(LoadError::Missing(format!(
                    "{name} is no longer a spec in {location}"
                )));
            }
            (None, 1) => specs[0].clone(),
            (None, 0) => {
                return Err(LoadError::Missing(format!(
                    "no spec is in {location} any more"
                )));
            }
            (None, _) => {
                return Err(LoadError::Missing(format!(
                    "{location} holds several specs; give the file"
                )));
            }
        };

        path.join(picked)
    } else {
        path.to_owned()
    };

    let text = super::read_file(&file).map_err(|e| LoadError::Failed(e.to_string()))?;
    let value = super::parse(&text).map_err(|e| LoadError::Failed(e.to_string()))?;

    Ok((value, None))
}
