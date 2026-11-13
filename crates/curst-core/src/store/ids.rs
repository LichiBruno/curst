//! random ids for collections and environments

use super::StoreError;
use crate::model::{EnvId, LocalId};

fn random_hex() -> Result<String, StoreError> {
    use std::fmt::Write;

    let mut bytes = [0_u8; 8];

    getrandom::fill(&mut bytes).map_err(|e| StoreError::Invalid(format!("no randomness: {e}")))?;

    Ok(bytes.iter().fold(String::with_capacity(16), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");

        hex
    }))
}

pub(crate) fn local_id() -> Result<LocalId, StoreError> {
    LocalId::new(&random_hex()?).ok_or_else(|| StoreError::Invalid("bad random id".into()))
}

pub(crate) fn env_id() -> Result<EnvId, StoreError> {
    EnvId::new(&random_hex()?).ok_or_else(|| StoreError::Invalid("bad random id".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_hex() {
        let a = local_id().unwrap();
        let b = local_id().unwrap();

        assert_eq!(a.as_str().len(), 16);
        assert_ne!(a, b);
        assert_eq!(env_id().unwrap().as_str().len(), 16);
    }
}
