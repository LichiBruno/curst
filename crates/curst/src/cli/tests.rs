use std::sync::Mutex;

use curst_core::http::{Cancel, HttpError, HttpExecutor, RedirectRule, Response};
use curst_core::model::{Auth, KeyPlace, Request, Variable};
use curst_core::secrets::{EncryptedFile, Secret, SecretIndex, SecretKey, Secrets};
use curst_core::store::{Backend, CollectionStore, Library, Paths, Settings};
use curst_core::vars::VarName;

use super::*;

/// answers every request with `status` and `body`, and keeps what it was
/// sent
struct Fake {
    status: u16,
    body: &'static str,
    sent: Mutex<Vec<Request>>,
}

fn fake(status: u16, body: &'static str) -> Fake {
    Fake {
        status,
        body,
        sent: Mutex::new(Vec::new()),
    }
}

impl HttpExecutor for Fake {
    fn execute(
        &self,
        request: &Request,
        _: &Cancel,
        _: &RedirectRule,
    ) -> Result<Response, HttpError> {
        let mut response = crate::app::tests::response(self.body);

        response.status = self.status;
        response.status_line = format!("HTTP/1.1 {}", self.status);

        if let Ok(mut sent) = self.sent.lock() {
            sent.push(request.clone());
        }

        Ok(response)
    }
}

fn library() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(Paths::under(dir.path()));

    (dir, library)
}

fn args(request: &str) -> RunArgs {
    RunArgs {
        request: request.into(),
        env: None,
        allow_host: Vec::new(),
        json: false,
    }
}

fn no_passphrase() -> Result<Secret, Fail> {
    Err(Fail::setup("asked for a passphrase"))
}

/// a collection whose request sends an API key kept in the encrypted file
fn with_secret(library: &Library) {
    let dir = library.create_collection("Shop").unwrap();
    let api_key = VarName::new("api_key").unwrap();

    dir.save_settings("Shop", &[Variable::secret(api_key.clone())])
        .unwrap();
    dir.create_request(
        &[],
        "Orders",
        &Request {
            url: "https://api.shop.example/orders".into(),
            auth: Auth::ApiKey {
                name: "X-Api-Key".into(),
                value: "{{api_key}}".into(),
                place: KeyPlace::Header,
            },
            ..Request::default()
        },
    )
    .unwrap();

    // the registry gives it its id once it is loaded
    let (collections, _) = load(library).unwrap();
    let key = SecretKey::Collection {
        collection: collections[0].id.clone(),
        name: api_key,
    };
    let paths = library.paths();

    Settings::set_backend(&paths.config_file(), Some(Backend::EncryptedFile)).unwrap();
    Secrets::new(
        SecretIndex::new(paths.secrets_index_file()),
        std::sync::Arc::new(EncryptedFile::new(paths.secrets_file(), Secret::new("pw"))),
    )
    .set(&key, &Secret::new("sk_live_1"))
    .unwrap();
}

#[test]
fn a_spec_is_imported_and_its_requests_run_by_name() {
    let (_dir, library) = library();
    let spec = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../curst-core/src/openapi/fixtures/store-3.0.json"
    );
    let executor = fake(200, "{\"up\":true}");
    let report = transfer::import(
        &library,
        &executor,
        &ImportArgs {
            source: spec.into(),
            spec: None,
        },
    )
    .unwrap();

    assert!(report.starts_with("imported Store: "), "{report}");
    assert!(report.contains("left out"), "{report}");

    assert!(
        report.contains("environment in use: Production"),
        "{report}"
    );

    // the first environment is in use, as after an import in the UI
    run::run(
        &library,
        &executor,
        &args("Store/GET /health"),
        &no_passphrase,
    )
    .unwrap();

    assert!(
        executor.sent.lock().unwrap()[0]
            .url
            .contains("store.example.com/api/health")
    );
    executor.sent.lock().unwrap().clear();

    // with none in use, base_url is not defined, and the error says why
    let (collections, _) = load(&library).unwrap();

    library
        .registry()
        .set_active_environment(&collections[0].root, None)
        .unwrap();

    let unchosen = run::run(
        &library,
        &executor,
        &args("Store/GET /health"),
        &no_passphrase,
    );

    assert!(unchosen.is_err_and(|f| f.message.contains("--env: Production, Staging")));

    // a name with a slash of its own, any case, and the body on stdout
    let staging = RunArgs {
        env: Some("staging".into()),
        ..args("store/get /HEALTH")
    };
    let answer = run::run(&library, &executor, &staging, &no_passphrase).unwrap();

    assert!(
        executor.sent.lock().unwrap()[0]
            .url
            .starts_with("https://staging.store.example.com/api")
    );
    let (mut out, mut err) = (Vec::new(), Vec::new());

    answer.print(&mut out, &mut err, false).unwrap();

    assert_eq!(answer.code(), 0);
    assert_eq!(out, b"{\"up\":true}");
    assert!(String::from_utf8_lossy(&err).starts_with("HTTP/1.1 200 · "));

    let wrong = run::run(&library, &executor, &args("Store/nothing"), &no_passphrase);

    assert!(wrong.is_err_and(|f| f.code == 2 && f.message.contains("Store")));
}

#[test]
fn secrets_come_from_the_store_and_go_only_where_approved() {
    let (_dir, library) = library();
    let executor = fake(201, "{}");

    with_secret(&library);

    // a passphrase is needed, and none can be asked for
    let locked = run::run(&library, &executor, &args("Shop/Orders"), &no_passphrase);

    assert!(locked.is_err_and(|f| f.message.contains("asked for a passphrase")));

    // the key would go to a host it was never approved for
    let ask = || Ok(Secret::new("pw"));
    let refused = run::run(&library, &executor, &args("Shop/Orders"), &ask);

    assert!(refused.is_err_and(|f| {
        f.code == 2
            && f.message.contains("{{api_key}}")
            && f.message.contains("--allow-host api.shop.example")
    }));
    assert!(executor.sent.lock().unwrap().is_empty());

    // allowed for this run, it goes, with its value
    let allowed = RunArgs {
        allow_host: vec!["api.shop.example".into()],
        json: true,
        ..args("shop/orders")
    };
    let answer = run::run(&library, &executor, &allowed, &ask).unwrap();
    let sent = executor.sent.lock().unwrap();

    assert_eq!(
        sent[0].auth,
        Auth::ApiKey {
            name: "X-Api-Key".into(),
            value: "sk_live_1".into(),
            place: KeyPlace::Header,
        }
    );

    let (mut out, mut err) = (Vec::new(), Vec::new());

    answer.print(&mut out, &mut err, true).unwrap();

    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();

    assert_eq!(json["status"], 201);
    assert_eq!(json["body"], "{}");
    assert_eq!(err, b"");

    // the list names it, never its value
    let list = secrets::list(&library).unwrap();

    assert!(list.contains("Shop api_key"), "{list}");
    assert!(!list.contains("sk_live_1"));
}

#[test]
fn answers_and_failures_give_their_exit_codes() {
    let response = |status| {
        run::Answer::Response(Box::new({
            let mut r = crate::app::tests::response("");

            r.status = status;
            r
        }))
    };

    assert_eq!(response(204).code(), 0);
    assert_eq!(response(302).code(), 0);
    assert_eq!(response(404).code(), 1);
    assert_eq!(response(503).code(), 1);
    assert_eq!(run::Answer::Failed("refused".into()).code(), 3);
}

#[test]
fn an_export_with_plain_credentials_needs_force() {
    let (dir, library) = library();
    let shop = library.create_collection("Shop").unwrap();

    shop.create_request(
        &[],
        "Orders",
        &Request {
            url: "https://h/".into(),
            auth: Auth::Bearer {
                token: "sk_live_0123456789abcdefghijklmn".into(),
            },
            ..Request::default()
        },
    )
    .unwrap();

    let export = |force| {
        transfer::export(
            &library,
            &ExportArgs {
                collection: "shop".into(),
                file: dir.path().join(if force {
                    "a.curst.toml"
                } else {
                    "b.curst.toml"
                }),
                force,
            },
        )
    };

    assert!(export(false).is_err_and(|f| f.message.contains("--force")));
    assert!(export(true).is_ok_and(|r| r.starts_with("wrote Shop")));

    // and it comes back in as a bundle
    let report = transfer::import(
        &library,
        &fake(200, ""),
        &ImportArgs {
            source: dir.path().join("a.curst.toml").display().to_string(),
            spec: None,
        },
    )
    .unwrap();

    assert_eq!(report, "imported Shop from the bundle");
}

#[test]
fn pruning_deletes_only_what_no_collection_owns() {
    let (_dir, library) = library();

    with_secret(&library);

    let gone = SecretKey::Collection {
        collection: curst_core::model::LocalId::new("gone").unwrap(),
        name: VarName::new("old").unwrap(),
    };
    let paths = library.paths();
    let secrets = Secrets::new(
        SecretIndex::new(paths.secrets_index_file()),
        std::sync::Arc::new(EncryptedFile::new(paths.secrets_file(), Secret::new("pw"))),
    );

    secrets.set(&gone, &Secret::new("x")).unwrap();

    let ask = || Ok(Secret::new("pw"));

    assert_eq!(
        secrets::prune(&library, &ask).unwrap(),
        "deleted 1 secret of a collection that is gone"
    );
    assert_eq!(secrets::prune(&library, &ask).unwrap(), "nothing to prune");
    assert!(secrets::list(&library).unwrap().contains("Shop api_key"));
}

#[test]
fn without_a_terminal_the_passphrase_comes_from_the_environment() {
    // tests run without a terminal; with CURST_PASSPHRASE unset the error
    // names it
    if std::env::var("CURST_PASSPHRASE").is_err() {
        let (_dir, library) = library();

        assert!(passphrase(library.paths()).is_err_and(|f| f.message.contains("CURST_PASSPHRASE")));
    }
}
