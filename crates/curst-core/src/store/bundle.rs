//! bundles: a whole collection in one file, to share it or move it to
//! another machine
//!
//! a bundle holds the requests, folders, variables and environments, and
//! names every secret the collection uses, never its value. a bundle
//! from elsewhere is untrusted input: nothing in it decides a file name,
//! an id or a path, and an import always makes a new collection
//!
//! ```toml
//! curst_bundle = 1
//! name = "Shop"
//!
//! [[variables]]
//! name = "token"
//! secret = true
//!
//! [[environments]]
//! name = "Staging"
//!
//! [[items]]
//! type = "folder"
//! name = "Orders"
//!
//! [[items.items]]
//! type = "request"
//! name = "List orders"
//! method = "GET"
//! url = "{{base_url}}/orders"
//! ```

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::StoreError;
use super::collection::{CollectionDir, CollectionStore};
use super::format::{RequestFile, VariableFile, to_toml, variables_from_file, variables_to_file};
use super::fs::read_regular;
use super::library::Library;
use crate::model::{Collection, Item, Request, SpecOrigin, Variable};
use crate::secrets::detect;

/// the version of the format this curst writes and reads
pub const BUNDLE_VERSION: u32 = 1;

/// bundles larger than this are not read
const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// folders nested deeper than this are not read
const MAX_DEPTH: usize = 32;

#[derive(Debug, Serialize, Deserialize)]
struct BundleFile {
    curst_bundle: u32,
    name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    variables: Vec<VariableFile>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    environments: Vec<EnvironmentFile>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    items: Vec<ItemFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EnvironmentFile {
    name: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    variables: Vec<VariableFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ItemFile {
    Folder {
        name: String,

        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        items: Vec<ItemFile>,
    },
    Request(Box<RequestFile>),
}

/// a folder or a request in a bundle, by name only
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleItem {
    /// a folder and what is in it
    Folder {
        /// its name
        name: String,

        /// what it holds, in order
        items: Vec<BundleItem>,
    },

    /// a request
    Request {
        /// its name
        name: String,

        /// the request
        request: Request,

        /// the spec operation it was made from, when it was; a bundle file
        /// never carries it
        spec: Option<SpecOrigin>,
    },
}

/// what a bundle holds, read and checked
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// the collection's name
    pub name: String,

    /// the collection variables; a secret has no value
    pub variables: Vec<Variable>,

    /// each environment's name and variables
    pub environments: Vec<(String, Vec<Variable>)>,

    /// the folders and requests, in order
    pub items: Vec<BundleItem>,

    /// what was wrong but did not stop the reading, such as a secret's
    /// value written into the file
    pub warnings: Vec<String>,
}

fn items_to_file(items: &[Item]) -> Vec<ItemFile> {
    items
        .iter()
        .map(|item| match item {
            Item::Folder(folder) => ItemFile::Folder {
                name: folder.name.clone(),
                items: items_to_file(&folder.items),
            },
            Item::Request(saved) => {
                ItemFile::Request(Box::new(RequestFile::new(&saved.name, &saved.request)))
            }
        })
        .collect()
}

/// the collection as bundle text; secrets keep their names only
#[must_use]
pub fn bundle_text(collection: &Collection) -> String {
    let file = BundleFile {
        curst_bundle: BUNDLE_VERSION,
        name: collection.name.clone(),
        variables: variables_to_file(&collection.variables),
        environments: collection
            .environments
            .iter()
            .map(|e| EnvironmentFile {
                name: e.name.clone(),
                variables: variables_to_file(&e.variables),
            })
            .collect(),
        items: items_to_file(&collection.items),
    };

    to_toml(&file)
}

/// requests under `items` that hold what looks like a credential, each
/// with its path
fn request_credentials(items: &[Item], path: &str, found: &mut Vec<String>) {
    for item in items {
        match item {
            Item::Folder(folder) => {
                request_credentials(&folder.items, &format!("{path}{} › ", folder.name), found);
            }
            Item::Request(saved) => {
                if let Some(finding) = detect::detect(&saved.request).first() {
                    found.push(format!(
                        "{path}{}: {}",
                        saved.name,
                        finding.reason.describe()
                    ));
                }
            }
        }
    }
}

/// values a bundle would hold in plain text that look like credentials,
/// each as where it is and why: plain variables and request fields, since
/// secrets never go in
#[must_use]
pub fn plain_credentials(collection: &Collection) -> Vec<String> {
    let mut found = Vec::new();
    let mut variables = |place: &str, variables: &[Variable]| {
        for v in variables.iter().filter(|v| !v.secret) {
            if let Some(reason) = detect::check_pair(v.name.as_str(), &v.value, false) {
                found.push(format!(
                    "{place} variable {}: {}",
                    v.name,
                    reason.describe()
                ));
            }
        }
    };

    variables("the collection", &collection.variables);

    for environment in &collection.environments {
        variables(&format!("the {}", environment.name), &environment.variables);
    }

    request_credentials(&collection.items, "", &mut found);

    found
}

/// writes the collection as a bundle at `path`, which must not exist yet
///
/// # Errors
/// when something is at `path` already, or it cannot be written
pub fn export(collection: &Collection, path: &Path) -> Result<(), StoreError> {
    // a new file only: an existing one is never overwritten
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| StoreError::io(path, e))?;

    file.write_all(bundle_text(collection).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| StoreError::io(path, e))
}

fn variables(
    file: Vec<VariableFile>,
    place: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Variable>, String> {
    let read = variables_from_file(file)?;

    warnings.extend(read.warnings.into_iter().map(|w| format!("{place}: {w}")));

    Ok(read.variables)
}

fn entries(files: Vec<ItemFile>, depth: usize) -> Result<Vec<BundleItem>, String> {
    if depth > MAX_DEPTH {
        return Err(format!("folders are nested more than {MAX_DEPTH} deep"));
    }

    files
        .into_iter()
        .map(|file| match file {
            ItemFile::Folder { name, items } => Ok(BundleItem::Folder {
                name,
                items: entries(items, depth + 1)?,
            }),
            ItemFile::Request(file) => {
                let (name, request) = file.into_request()?;

                Ok(BundleItem::Request {
                    name,
                    request,
                    spec: None,
                })
            }
        })
        .collect()
}

/// the bundle at `path`, read and checked
///
/// # Errors
/// when it cannot be read, is too large, is not a bundle, is of a version
/// this curst does not know, or holds something invalid
pub fn read(path: &Path) -> Result<Bundle, StoreError> {
    let size = fs::metadata(path)
        .map_err(|e| StoreError::io(path, e))?
        .len();

    if size > MAX_BYTES {
        return Err(StoreError::Invalid(format!(
            "{} is larger than {} MB, too large for a bundle",
            path.display(),
            MAX_BYTES / 1024 / 1024
        )));
    }

    let text =
        read_regular(path)?.ok_or_else(|| StoreError::NotFound(path.display().to_string()))?;
    let file: BundleFile =
        toml::from_str(&text).map_err(|e| StoreError::parse(path, e.message()))?;

    if file.curst_bundle != BUNDLE_VERSION {
        return Err(StoreError::Invalid(format!(
            "{} is a bundle of version {}, and this curst reads version {BUNDLE_VERSION}",
            path.display(),
            file.curst_bundle
        )));
    }

    let mut warnings = Vec::new();
    let invalid = |message: String| StoreError::parse(path, message);
    let variables_read =
        variables(file.variables, "the collection", &mut warnings).map_err(invalid)?;
    let mut environments = Vec::new();

    for environment in file.environments {
        let place = format!("the {} environment", environment.name);
        let read = variables(environment.variables, &place, &mut warnings).map_err(invalid)?;

        environments.push((environment.name, read));
    }

    Ok(Bundle {
        name: file.name,
        variables: variables_read,
        environments,
        items: entries(file.items, 0).map_err(invalid)?,
        warnings,
    })
}

fn create_entries(
    dir: &CollectionDir,
    parent: &[String],
    entries: &[BundleItem],
) -> Result<(), StoreError> {
    for entry in entries {
        match entry {
            BundleItem::Folder { name, items } => {
                let slug = dir.create_folder(parent, name)?;
                let mut path = parent.to_vec();

                path.push(slug);
                create_entries(dir, &path, items)?;
            }
            BundleItem::Request {
                name,
                request,
                spec,
            } => {
                dir.create_from_spec(parent, name, request, spec.as_ref())?;
            }
        }
    }

    Ok(())
}

pub(crate) fn fill(dir: &CollectionDir, bundle: &Bundle) -> Result<(), StoreError> {
    dir.save_settings(&bundle.name, &bundle.variables)?;

    for (name, variables) in &bundle.environments {
        let mut environment = dir.create_environment(name)?;

        environment.variables.clone_from(variables);
        dir.save_environment(&environment)?;
    }

    create_entries(dir, &[], &bundle.items)
}

impl Library {
    /// makes a new collection, with new ids, from the bundle at `path`;
    /// when anything fails, nothing of it is left behind
    ///
    /// # Errors
    /// when the bundle cannot be read, or the collection cannot be written
    pub fn import_bundle(&self, path: &Path) -> Result<(CollectionDir, Bundle), StoreError> {
        self.import_with(read(path)?, fill)
    }

    /// makes the collection and has `fill` write what is in it, removing
    /// the collection again when that fails
    pub(crate) fn import_with(
        &self,
        bundle: Bundle,
        fill: impl FnOnce(&CollectionDir, &Bundle) -> Result<(), StoreError>,
    ) -> Result<(CollectionDir, Bundle), StoreError> {
        let dir = self.create_collection(&bundle.name)?;

        if let Err(err) = fill(&dir, &bundle) {
            let _ = self.delete_collection(&dir);

            return Err(err);
        }

        Ok((dir, bundle))
    }
}

#[cfg(test)]
mod tests;
