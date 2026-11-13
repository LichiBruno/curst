#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! the one check of the real OS keychain; it writes to the keychain of the
//! person running it, so it only runs when asked for:
//!
//! `cargo test -p curst-core --test keychain -- --ignored`
//!
//! macOS may show a keychain access prompt the first time

use curst_core::secrets::{Keychain, Secret, SecretKey, SecretStore};
use curst_core::vars::VarName;

#[test]
#[ignore = "writes to the real OS keychain"]
fn the_os_keychain_stores_reads_and_deletes() {
    let keychain = Keychain::open().unwrap();
    let key = SecretKey::Global {
        name: VarName::new("curst-manual-check").unwrap(),
    };

    keychain.probe().unwrap();
    keychain.set(&key, &Secret::new("first")).unwrap();
    keychain.set(&key, &Secret::new("second")).unwrap();

    assert_eq!(keychain.get(&key).unwrap(), Some(Secret::new("second")));

    keychain.delete(&key).unwrap();
    keychain.delete(&key).unwrap();

    assert_eq!(keychain.get(&key).unwrap(), None);
}
