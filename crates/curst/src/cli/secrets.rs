//! `curst secrets`: the secrets curst keeps, by name; a value is never
//! shown

use std::fmt::Write as _;

use curst_core::secrets::{SecretIndex, SecretKey};
use curst_core::store::Library;

use super::{Ask, Fail};

/// how a list names a key: its scope, by the collection's and the
/// environment's names where they are known, and the variable
fn label(key: &SecretKey, collections: &[crate::app::Workspace]) -> String {
    let collection = |id: &curst_core::model::LocalId| {
        collections
            .iter()
            .find(|w| w.id == *id)
            .map_or_else(|| format!("{id} (gone)"), |w| w.collection.name.clone())
    };

    match key {
        SecretKey::Global { name } => format!("global {name}"),
        SecretKey::Collection {
            collection: id,
            name,
        } => {
            format!("{} {name}", collection(id))
        }
        SecretKey::Environment {
            collection: id,
            environment,
            name,
        } => {
            let env = collections
                .iter()
                .find(|w| w.id == *id)
                .and_then(|w| w.collection.environment(environment))
                .map_or_else(|| environment.to_string(), |e| e.name.clone());

            format!("{} › {env} {name}", collection(id))
        }
        SecretKey::Probe => "probe".into(),
    }
}

/// every secret in the index, one per line, with the hosts it may go to
pub(crate) fn list(library: &Library) -> Result<String, Fail> {
    let entries = SecretIndex::new(library.paths().secrets_index_file())
        .entries()
        .map_err(|e| Fail::setup(format!("the secrets index: {e}")))?;
    let (collections, _) = super::load(library)?;
    let mut out = String::new();

    for entry in entries.iter().filter(|e| e.key != SecretKey::Probe) {
        let hosts = if entry.hosts.is_empty() {
            "not approved for any host yet".to_owned()
        } else {
            entry.hosts.join(", ")
        };

        let _ = writeln!(out, "{}\t{hosts}", label(&entry.key, &collections));
    }

    Ok(out)
}

/// deletes the secrets of collections this machine no longer knows
pub(crate) fn prune(library: &Library, ask: Ask<'_>) -> Result<String, Fail> {
    // a registry that cannot be read must never make every secret look
    // forgotten
    let known: Vec<_> = library
        .registry()
        .entries()
        .map_err(|e| {
            Fail::setup(format!(
                "the registry could not be read, so nothing was deleted: {e}"
            ))
        })?
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    let store = super::open_store(library.paths(), ask)?;
    let deleted = store
        .prune(&known)
        .map_err(|e| Fail::setup(format!("the secrets could not be pruned: {e}")))?;

    Ok(match deleted {
        0 => "nothing to prune".into(),
        1 => "deleted 1 secret of a collection that is gone".into(),
        n => format!("deleted {n} secrets of collections that are gone"),
    })
}
