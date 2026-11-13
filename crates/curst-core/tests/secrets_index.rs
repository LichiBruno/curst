#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use curst_core::secrets::{MemoryStore, Secret, SecretIndex, SecretKey, Secrets};
use curst_core::vars::VarName;

const FILE: &str = "CURST_TEST_SECRETS_INDEX";
const WORKER: &str = "CURST_TEST_WORKER";
const WORKERS: usize = 4;
const PER_WORKER: usize = 20;

/// the body of one child process started by the test below; run on its own
/// it does nothing
#[test]
fn index_worker() {
    let (Some(file), Ok(worker)) = (env::var_os(FILE), env::var(WORKER)) else {
        return;
    };

    let secrets = Secrets::new(
        SecretIndex::new(PathBuf::from(file)),
        Arc::new(MemoryStore::new()),
    );

    for i in 0..PER_WORKER {
        let key = SecretKey::Global {
            name: VarName::new(&format!("w{worker}-{i}")).unwrap(),
        };

        secrets.set(&key, &Secret::new("v")).unwrap();
    }
}

#[test]
fn processes_updating_the_index_at_once_lose_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secrets-index.toml");
    let children: Vec<_> = (0..WORKERS)
        .map(|worker| {
            Command::new(env::current_exe().unwrap())
                .args(["--exact", "index_worker", "--quiet"])
                .env(FILE, &file)
                .env(WORKER, worker.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();

    for mut child in children {
        assert!(child.wait().unwrap().success());
    }

    let entries = SecretIndex::new(&file).entries().unwrap();

    assert_eq!(entries.len(), WORKERS * PER_WORKER);
}
