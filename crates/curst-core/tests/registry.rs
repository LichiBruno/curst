#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use curst_core::store::Registry;

const FILE: &str = "CURST_TEST_REGISTRY";
const WORKER: &str = "CURST_TEST_WORKER";
const WORKERS: usize = 4;
const PER_WORKER: usize = 25;

/// the body of one child process started by the test below; run on its own
/// it does nothing
#[test]
fn registry_worker() {
    let (Some(file), Ok(worker)) = (env::var_os(FILE), env::var(WORKER)) else {
        return;
    };

    let file = PathBuf::from(file);
    let registry = Registry::new(&file);
    let root = file.parent().unwrap();

    for i in 0..PER_WORKER {
        registry
            .register(&root.join(format!("w{worker}-{i}")), true)
            .unwrap();
    }
}

#[test]
fn processes_updating_the_registry_at_once_lose_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("registry.toml");
    let children: Vec<_> = (0..WORKERS)
        .map(|worker| {
            Command::new(env::current_exe().unwrap())
                .args(["--exact", "registry_worker", "--quiet"])
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

    let entries = Registry::new(&file).entries().unwrap();
    let mut ids: Vec<_> = entries.iter().map(|e| e.id.clone()).collect();

    ids.sort();
    ids.dedup();

    assert_eq!(entries.len(), WORKERS * PER_WORKER);
    assert_eq!(ids.len(), WORKERS * PER_WORKER);
}
