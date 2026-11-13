//! the index and a store together, kept in step
//!
//! the order of every change keeps one rule: a stored secret always has an
//! index entry. on a write the entry comes first, then the value; on a
//! delete the value goes first, then the entry. a crash can leave an entry
//! with no value, which `prune` removes, but never a value with no entry

use std::sync::Arc;

use super::index::Locked;
use super::{IndexEntry, Secret, SecretError, SecretIndex, SecretKey, SecretScope, SecretStore};
use crate::model::LocalId;

/// secrets with their index
#[derive(Clone)]
pub struct Secrets {
    pub(super) index: SecretIndex,
    store: Arc<dyn SecretStore>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// stores `value` and records `key`, undoing the record when the store
/// fails and the record is new
fn put(
    locked: &mut Locked<'_>,
    store: &dyn SecretStore,
    key: &SecretKey,
    value: &Secret,
    hosts: Vec<String>,
) -> Result<(), SecretError> {
    let added = !locked.contains(key);

    if added {
        locked.entries.push(IndexEntry {
            key: key.clone(),
            hosts,
        });
        locked.save()?;
    }

    if let Err(err) = store.set(key, value) {
        if added {
            locked.entries.retain(|e| e.key != *key);
            locked.save()?;
        }

        return Err(err);
    }

    Ok(())
}

impl Secrets {
    /// `store` tracked by `index`
    pub fn new(index: SecretIndex, store: Arc<dyn SecretStore>) -> Self {
        Self { index, store }
    }

    /// the store in use
    #[must_use]
    pub fn store(&self) -> &Arc<dyn SecretStore> {
        &self.store
    }

    /// every secret in use
    ///
    /// # Errors
    /// when the index cannot be read
    pub fn list(&self) -> Result<Vec<IndexEntry>, SecretError> {
        self.index.entries()
    }

    /// the value of `key`
    ///
    /// # Errors
    /// when the store cannot be read
    pub fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        self.store.get(key)
    }

    /// stores `value` under `key`
    ///
    /// # Errors
    /// when the index or the store cannot be written; the index is left as
    /// it was
    pub fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError> {
        self.index
            .locked(|locked| put(locked, self.store.as_ref(), key, value, Vec::new()))
    }

    /// deletes `key`
    ///
    /// # Errors
    /// when the store or the index cannot be written
    pub fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.index.locked(|locked| {
            self.store.delete(key)?;
            locked.entries.retain(|e| e.key != *key);
            locked.save()
        })
    }

    /// how many secrets `scope` holds, to show before deleting it
    ///
    /// # Errors
    /// when the index cannot be read
    pub fn count(&self, scope: &SecretScope) -> Result<usize, SecretError> {
        Ok(self.list()?.iter().filter(|e| e.key.is_in(scope)).count())
    }

    /// deletes every secret of `scope` and returns how many there were
    ///
    /// # Errors
    /// when the store or the index cannot be written; the secrets deleted
    /// so far are gone from both
    pub fn delete_scope(&self, scope: &SecretScope) -> Result<usize, SecretError> {
        self.delete_where(|key| key.is_in(scope))
    }

    fn delete_where(&self, doomed: impl Fn(&SecretKey) -> bool) -> Result<usize, SecretError> {
        self.index.locked(|locked| {
            let keys: Vec<SecretKey> = locked
                .entries
                .iter()
                .map(|e| e.key.clone())
                .filter(|key| doomed(key))
                .collect();
            let mut deleted = 0;

            for key in &keys {
                if let Err(err) = self.store.delete(key) {
                    locked.save()?;

                    return Err(err);
                }

                locked.entries.retain(|e| e.key != *key);
                deleted += 1;
            }

            locked.save()?;

            Ok(deleted)
        })
    }

    /// moves the secret of a renamed variable, with its approved hosts
    ///
    /// # Errors
    /// [`SecretError::NotFound`] when `from` has no value, or when the
    /// store or the index cannot be written
    pub fn rename(&self, from: &SecretKey, to: &SecretKey) -> Result<(), SecretError> {
        // moving onto itself would delete the value it just wrote
        if from == to {
            return Ok(());
        }

        self.index.locked(|locked| {
            let value = self
                .store
                .get(from)?
                .ok_or_else(|| SecretError::NotFound(from.to_string()))?;
            let hosts = locked
                .entries
                .iter()
                .find(|e| e.key == *from)
                .map_or_else(Vec::new, |e| e.hosts.clone());

            put(locked, self.store.as_ref(), to, &value, hosts)?;
            self.store.delete(from)?;
            locked.entries.retain(|e| e.key != *from);
            locked.save()
        })
    }

    /// copies every secret to `target` in one write and checks each copy;
    /// the originals stay; returns the secrets now in `target` and the keys
    /// to delete from the old store once the switch is recorded
    ///
    /// # Errors
    /// when a copy fails or reads back differently
    pub fn copy_to(
        &self,
        target: Arc<dyn SecretStore>,
    ) -> Result<(Self, Vec<SecretKey>), SecretError> {
        self.index.locked(|locked| {
            let mut values = Vec::new();

            for entry in &locked.entries {
                // an entry whose value was lost has nothing to copy
                if let Some(value) = self.store.get(&entry.key)? {
                    values.push((entry.key.clone(), value));
                }
            }

            target.set_all(&values)?;

            for (key, value) in &values {
                if target.get(key)?.as_ref() != Some(value) {
                    return Err(SecretError::Backend(format!(
                        "{key} did not copy correctly"
                    )));
                }
            }

            let keys = values.into_iter().map(|(key, _)| key).collect();

            Ok((Self::new(self.index.clone(), target), keys))
        })
    }

    /// deletes these values from this store, leaving the index alone: the
    /// last step of a switch, once the copies are in use
    ///
    /// # Errors
    /// when the store cannot be written
    pub fn forget_values(&self, keys: &[SecretKey]) -> Result<(), SecretError> {
        keys.iter().try_for_each(|key| self.store.delete(key))
    }

    /// copies every secret to `target`, checks each copy, then deletes the
    /// originals; returns the secrets now in `target`
    ///
    /// # Errors
    /// when a copy fails or reads back differently; the originals are then
    /// all still in place
    pub fn migrate(&self, target: Arc<dyn SecretStore>) -> Result<Self, SecretError> {
        let (moved, keys) = self.copy_to(target)?;

        self.forget_values(&keys)?;

        Ok(moved)
    }

    /// deletes the secrets of collections this machine no longer knows,
    /// returning how many; `known` must come from a registry that was read
    /// without error
    ///
    /// # Errors
    /// when the store or the index cannot be written
    pub fn prune(&self, known: &[LocalId]) -> Result<usize, SecretError> {
        self.delete_where(|key| key.collection().is_some_and(|c| !known.contains(c)))
    }
}

#[cfg(test)]
mod tests;
