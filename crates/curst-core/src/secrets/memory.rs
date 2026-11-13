//! a store that lives in memory: for tests, and for a session that keeps
//! nothing

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use super::{Secret, SecretError, SecretKey, SecretStore};

/// secrets held in memory and lost when it is dropped
#[derive(Debug, Default)]
pub struct MemoryStore {
    values: Mutex<BTreeMap<SecretKey, Secret>>,
}

impl MemoryStore {
    /// an empty store
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// every key that has a value
    #[must_use]
    pub fn keys(&self) -> Vec<SecretKey> {
        self.values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let values = self.values.lock().unwrap_or_else(PoisonError::into_inner);

        Ok(values.get(key).cloned())
    }

    fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError> {
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);

        values.insert(key.clone(), value.clone());

        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);

        values.remove(key);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_come_back_and_go() {
        let store = MemoryStore::new();
        let key = SecretKey::Global {
            name: crate::vars::VarName::new("t").unwrap(),
        };

        assert_eq!(store.get(&key).unwrap(), None);

        store.set(&key, &Secret::new("v")).unwrap();

        assert_eq!(store.get(&key).unwrap(), Some(Secret::new("v")));
        assert_eq!(store.keys(), std::slice::from_ref(&key));

        store.delete(&key).unwrap();
        store.delete(&key).unwrap();

        assert_eq!(store.keys(), Vec::new());
        assert!(store.probe().is_ok());
        assert_eq!(store.keys(), Vec::new());
    }
}
