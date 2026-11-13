use super::*;
use crate::model::{Auth, Body, KeyValue, Method, Request};
use crate::store::bundle::BundleItem;

const SHELTER: &str = include_str!("fixtures/shelter-2.0.yaml");
const STORE: &str = include_str!("fixtures/store-3.0.json");
const NOTES: &str = include_str!("fixtures/notes-3.1.yaml");

fn converted(text: &str, origin: Option<&str>) -> Converted {
    convert(&parse(text).unwrap(), origin).unwrap()
}

/// every request, with the folder it is in and the operation it came from
fn requests(converted: &Converted) -> Vec<(String, String, Request)> {
    let mut found = Vec::new();

    for item in &converted.bundle.items {
        match item {
            BundleItem::Folder { name, items } => {
                for inner in items {
                    if let BundleItem::Request { request, spec, .. } = inner {
                        let operation = spec.as_ref().unwrap().operation.clone();

                        found.push((name.clone(), operation, request.clone()));
                    }
                }
            }
            BundleItem::Request { request, spec, .. } => {
                let operation = spec.as_ref().unwrap().operation.clone();

                found.push((String::new(), operation, request.clone()));
            }
        }
    }

    found
}

fn request(converted: &Converted, operation: &str) -> Request {
    requests(converted)
        .into_iter()
        .find(|(_, op, _)| op == operation)
        .map_or_else(|| panic!("no {operation}"), |(_, _, request)| request)
}

/// a variable's value: in the first environment, where the variables of a
/// spec with servers are, or in the collection
fn variable(converted: &Converted, name: &str) -> Option<String> {
    let variables = converted
        .bundle
        .environments
        .first()
        .map_or(&converted.bundle.variables, |(_, variables)| variables);

    variables
        .iter()
        .find(|v| v.name.as_str() == name)
        .map(|v| v.value.clone())
}

fn json(body: &Body) -> serde_json::Value {
    let Body::Json(text) = body else {
        panic!("{body:?}");
    };

    serde_json::from_str(text).unwrap()
}

#[test]
fn swagger_2_becomes_a_collection() {
    let shelter = converted(SHELTER, None);
    let folders: Vec<(String, String)> = requests(&shelter)
        .into_iter()
        .map(|(folder, op, _)| (folder, op))
        .collect();

    assert_eq!(shelter.bundle.name, "Shelter");

    // one server: one environment, named after it, holding the variables
    assert_eq!(shelter.bundle.environments[0].0, "shelter.example.com");
    assert_eq!(shelter.bundle.variables, []);
    assert_eq!(
        folders,
        [
            ("pets".into(), "GET /pets".into()),
            ("pets".into(), "POST /pets".into()),
            ("photos".into(), "PUT /pets/{petId}/photo".into()),
        ]
    );
    assert_eq!((shelter.requests, shelter.folders), (3, 2));
    assert_eq!(
        variable(&shelter, "base_url").as_deref(),
        Some("https://shelter.example.com/v1")
    );

    // the required parameter is on, the optional one off, with its default
    let list = request(&shelter, "GET /pets");

    assert_eq!(list.url, "{{base_url}}/pets");
    assert_eq!(
        list.params,
        [
            KeyValue {
                name: "limit".into(),
                value: "20".into(),
                enabled: false
            },
            KeyValue::new("species", "cat"),
        ]
    );
    assert_eq!(
        list.auth,
        Auth::Basic {
            username: "{{username}}".into(),
            password: "{{password}}".into()
        }
    );

    // a body parameter becomes JSON from its schema, its example kept
    let add = request(&shelter, "POST /pets");

    assert_eq!(add.method, Method::Post);
    assert_eq!(
        json(&add.body),
        serde_json::json!({"name": "Rex", "born": "2026-01-01", "vaccinated": false})
    );

    // the path parameter is a variable, with its example
    let photo = request(&shelter, "PUT /pets/{petId}/photo");

    assert_eq!(photo.url, "{{base_url}}/pets/{{petid}}/photo");
    assert_eq!(variable(&shelter, "petid").as_deref(), Some("7"));
    assert_eq!(
        photo.body,
        Body::Form(vec![KeyValue {
            name: "caption".into(),
            value: String::new(),
            enabled: false
        }])
    );
    assert_eq!(photo.headers[0].name, "X-Trace");
    assert_eq!(shelter.secrets, ["password"]);
}

#[test]
fn openapi_3_0_becomes_a_collection() {
    let store = converted(STORE, None);

    assert_eq!(store.bundle.name, "Store");

    // each server is an environment, its variables filled in
    let environments: Vec<(&str, &str)> = store
        .bundle
        .environments
        .iter()
        .map(|(name, vars)| (name.as_str(), vars[0].value.as_str()))
        .collect();

    assert_eq!(
        environments,
        [
            ("Production", "https://eu.store.example.com/api"),
            ("Staging", "https://staging.store.example.com/api"),
        ]
    );

    // every environment holds every variable, secrets included, so each
    // server keeps its own token and ids
    for (name, variables) in &store.bundle.environments {
        let names: Vec<&str> = variables.iter().map(|v| v.name.as_str()).collect();

        for wanted in ["base_url", "x_api_key", "orderid", "token"] {
            assert!(
                names.contains(&wanted),
                "{wanted} is not in {name}: {names:?}"
            );
        }

        assert!(
            variables
                .iter()
                .filter(|v| v.secret)
                .all(|v| v.value.is_empty())
        );
    }

    // the scheme's header, never the example a parameter of that name gives
    let list = request(&store, "GET /orders");

    assert_eq!(list.headers, [KeyValue::new("X-Api-Key", "{{x_api_key}}")]);
    assert!(!format!("{:?}", store.bundle).contains("DO_NOT_COPY"));

    // a referenced parameter, and a token in an ordinary example, which is
    // flagged and left out
    assert_eq!(list.params[0].name, "page");
    assert_eq!(
        list.params[1],
        KeyValue {
            name: "access_token".into(),
            value: "{{access_token}}".into(),
            enabled: false
        }
    );
    assert!(!format!("{:?}", store.bundle).contains("eyJhbGci"));
    assert_eq!(store.credentials.len(), 1, "{:?}", store.credentials);
    assert!(store.credentials[0].starts_with("List orders: access_token"));

    // the operation's own security wins, and a recursive schema ends
    let create = request(&store, "POST /orders");

    assert_eq!(
        create.auth,
        Auth::Bearer {
            token: "{{token}}".into()
        }
    );
    assert_eq!(create.headers, []);

    let body = json(&create.body);

    assert_eq!(body["id"], "00000000-0000-0000-0000-000000000000");
    assert_eq!(body["category"]["parent"], serde_json::Value::Null);
    assert_eq!(body["tags"], serde_json::json!(["string"]));

    // no security at all, and a request without a tag stays at the top
    let health = request(&store, "GET /health");

    assert_eq!((health.auth, health.headers), (Auth::None, vec![]));
    assert!(matches!(
        store.bundle.items.last(),
        Some(BundleItem::Request { name, .. }) if name == "GET /health"
    ));
    assert_eq!(
        request(&store, "DELETE /orders/{orderId}").url,
        "{{base_url}}/orders/{{orderid}}"
    );

    let mut secrets = store.secrets.clone();

    secrets.sort();

    assert_eq!(secrets, ["access_token", "token", "x_api_key"]);
}

#[test]
fn openapi_3_1_becomes_a_collection() {
    // a relative server is resolved against where the spec came from
    let notes = converted(NOTES, Some("https://notes.example.com/docs/openapi.yaml"));

    assert_eq!(
        variable(&notes, "base_url").as_deref(),
        Some("https://notes.example.com/api")
    );

    // a server without a description is named by its URL, once resolved
    assert_eq!(
        notes.bundle.environments[0].0,
        "https://notes.example.com/api"
    );

    // an example under `examples`, written with an anchor
    assert_eq!(
        json(&request(&notes, "POST /notes").body),
        serde_json::json!({"text": "buy milk", "pinned": false})
    );

    // a list of types picks the one that is not null
    assert_eq!(
        json(&request(&notes, "PUT /notes").body),
        serde_json::json!([{"text": "string", "size": 0}])
    );
    assert_eq!(
        request(&notes, "GET /notes/search").params,
        [KeyValue::new("q", "milk")]
    );

    // without where it came from, a relative server is pointed out
    let unplaced = converted(NOTES, None);

    assert_eq!(variable(&unplaced, "base_url").as_deref(), Some("/api"));
    assert!(
        unplaced
            .bundle
            .warnings
            .iter()
            .any(|w| w.contains("relative"))
    );
}

fn fingerprints(converted: &Converted) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut push = |item: &BundleItem| {
        if let BundleItem::Request {
            spec: Some(spec), ..
        } = item
        {
            found.push((spec.operation.clone(), spec.fingerprint.clone()));
        }
    };

    for item in &converted.bundle.items {
        match item {
            BundleItem::Folder { items, .. } => items.iter().for_each(&mut push),
            other @ BundleItem::Request { .. } => push(other),
        }
    }

    found.sort();

    found
}

#[test]
fn the_fingerprint_follows_the_spec_not_its_order() {
    let original = converted(STORE, None);

    // the same spec, its keys in another order, written as YAML
    let value = parse(STORE).unwrap();
    let object = value.as_object().unwrap();
    let mut reordered = serde_json::Map::new();

    for key in object.keys().rev() {
        reordered.insert(key.clone(), object[key].clone());
    }

    let yaml = serde_yaml_ng::to_string(&serde_json::Value::Object(reordered)).unwrap();

    assert_eq!(
        fingerprints(&original),
        fingerprints(&converted(&yaml, None))
    );

    // a change in a schema an operation refers to changes its fingerprint,
    // and only its
    let changed = STORE.replace(
        "\"quantity\": { \"type\": \"integer\" }",
        "\"quantity\": { \"type\": \"number\" }",
    );
    let before = fingerprints(&original);
    let after = fingerprints(&converted(&changed, None));
    let differ: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a.1 != b.1)
        .map(|(a, _)| a.0.as_str())
        .collect();

    assert_eq!(differ, ["POST /orders"]);
}

#[test]
fn what_is_not_a_spec_is_refused() {
    assert!(matches!(parse("{ not json"), Err(SpecError::Syntax(_))));
    assert!(matches!(
        convert(&parse("a: 1").unwrap(), None),
        Err(SpecError::NotASpec)
    ));
    assert!(matches!(
        convert(&parse("swagger: '1.2'").unwrap(), None),
        Err(SpecError::Version(_))
    ));
    assert!(matches!(
        convert(&parse("openapi: 4.0.0").unwrap(), None),
        Err(SpecError::Version(_))
    ));

    // an alias bomb and endless nesting stop
    let mut bomb = String::from("openapi: 3.0.0\na0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");

    for i in 1..12 {
        let p = i - 1;
        let list = vec![format!("*a{p}"); 10].join(", ");

        bomb = format!("{bomb}a{i}: &a{i} [{list}]\n");
    }

    assert!(matches!(parse(&bomb), Err(SpecError::Syntax(_))));
    assert!(matches!(
        parse(&format!("a: {}{}", "[".repeat(5000), "]".repeat(5000))),
        Err(SpecError::Syntax(_))
    ));
    assert_eq!(parse(&" ".repeat(MAX_BYTES + 1)), Err(SpecError::TooLarge));

    // a reference to another file or a URL is never followed
    let external = r#"{"openapi": "3.0.0", "paths": {
        "/a": {"$ref": "other.yaml#/a"},
        "/b": {"get": {"parameters": [{"$ref": "https://evil.example/p"}]}}}}"#;
    let result = convert(&parse(external).unwrap(), None).unwrap();

    // no servers: the variables stay in the collection
    assert_eq!(result.requests, 1);
    assert_eq!(result.bundle.environments, []);
    assert_eq!(result.bundle.variables[0].name.as_str(), "base_url");
    assert!(result.bundle.warnings[0].contains("another file"));
    assert_eq!(request(&result, "GET /b").params, []);
}

#[test]
fn big_numbers_and_status_keys_survive_yaml() {
    let value = parse(
        "openapi: 3.0.0\nx-big: 12345678901234567890123\npaths:\n  /a:\n    get:\n      responses:\n        200:\n          description: ok\n",
    )
    .unwrap();

    assert_eq!(value["x-big"].to_string(), "12345678901234567890123");
    assert!(value["paths"]["/a"]["get"]["responses"]["200"].is_object());
}

#[test]
fn an_imported_spec_remembers_where_it_came_from() {
    use crate::model::{Item, Source};
    use crate::store::{CollectionStore, Library, Paths};

    let root = tempfile::tempdir().unwrap();
    let library = Library::new(Paths::under(root.path()));
    let file = root.path().join("store.json");

    std::fs::write(&file, STORE).unwrap();

    let source = Source {
        location: file.display().to_string(),
        ..Source::default()
    };
    let converted = converted(&read_file(&file).unwrap(), None);
    let (dir, _) = library.import_spec(converted, &source).unwrap();
    let collection = dir.load().unwrap().collection;

    assert_eq!(collection.source, Some(source));
    assert_eq!(collection.environments.len(), 2);

    let Some(Item::Folder(orders)) = collection.items.first() else {
        panic!("{:?}", collection.items);
    };
    let Item::Request(first) = &orders.items[0] else {
        panic!("not a request");
    };

    assert_eq!(first.spec.as_ref().unwrap().operation, "GET /orders");

    // a secret is named, never given a value
    let key = collection.environments[0]
        .variables
        .iter()
        .find(|v| v.name.as_str() == "x_api_key")
        .unwrap();

    assert!(key.secret && key.value.is_empty());
}

mod discovery {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// a fake web: each address's body, and every address asked for
    fn web(pages: &[(&str, &str)]) -> (HashMap<String, String>, RefCell<Vec<String>>) {
        let pages = pages
            .iter()
            .map(|(url, body)| ((*url).to_owned(), (*body).to_owned()))
            .collect();

        (pages, RefCell::new(Vec::new()))
    }

    fn found(
        start: &str,
        pages: &HashMap<String, String>,
        asked: &RefCell<Vec<String>>,
    ) -> Result<(serde_json::Value, String), String> {
        discover(start, |url| {
            asked.borrow_mut().push(url.to_owned());

            pages
                .get(url)
                .map(|body| Fetched {
                    url: url.to_owned(),
                    body: body.clone(),
                })
                .ok_or_else(|| "404".to_owned())
        })
    }

    #[test]
    fn a_spec_address_is_used_as_it_is() {
        let (pages, asked) = web(&[("https://h/openapi.json", STORE)]);
        let (spec, at) = found("https://h/openapi.json", &pages, &asked).unwrap();

        assert_eq!(spec["info"]["title"], "Store");
        assert_eq!(at, "https://h/openapi.json");
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn swagger_ui_is_followed_to_its_spec() {
        let (pages, asked) = web(&[
            ("https://h/docs/", include_str!("fixtures/swagger-ui.html")),
            (
                "https://h/docs/swagger-initializer.js",
                include_str!("fixtures/swagger-initializer.js"),
            ),
            ("https://h/v1/openapi.yaml", NOTES),
        ]);
        let (spec, at) = found("https://h/docs/", &pages, &asked).unwrap();

        assert_eq!(spec["info"]["title"], "Notes");
        assert_eq!(at, "https://h/v1/openapi.yaml");

        // the page, its initializer, then the spec: nothing else
        assert_eq!(
            *asked.borrow(),
            [
                "https://h/docs/",
                "https://h/docs/swagger-initializer.js",
                "https://h/v1/openapi.yaml"
            ]
        );
    }

    #[test]
    fn redoc_is_followed_to_its_spec() {
        let (pages, asked) = web(&[
            ("https://h/reference", include_str!("fixtures/redoc.html")),
            ("https://h/specs/store.json", STORE),
        ]);
        let (_, at) = found("https://h/reference", &pages, &asked).unwrap();

        assert_eq!(at, "https://h/specs/store.json");
    }

    #[test]
    fn the_usual_paths_are_tried_when_a_page_names_nothing() {
        let (pages, asked) = web(&[
            ("https://h/", "<html><body>welcome</body></html>"),
            ("https://h/v3/api-docs", STORE),
        ]);
        let (_, at) = found("https://h/", &pages, &asked).unwrap();

        assert_eq!(at, "https://h/v3/api-docs");

        // a site without a spec is given up on within the limit
        let (pages, asked) = web(&[("https://h/", "<html>nothing</html>")]);
        let error = found("https://h/", &pages, &asked).unwrap_err();

        assert!(error.contains("no OpenAPI"), "{error}");
        assert!(asked.borrow().len() <= 12);

        // an address that cannot be fetched says why
        let (pages, asked) = web(&[]);

        assert_eq!(found("https://gone/", &pages, &asked), Err("404".into()));
    }

    #[test]
    fn a_spec_written_into_the_init_script_is_read_from_there() {
        // the shape NestJS serves: the page loads swagger-ui-init.js, which
        // holds the whole spec under `swaggerDoc`
        let (pages, asked) = web(&[
            ("https://h/v2/docs", include_str!("fixtures/nest-docs.html")),
            (
                "https://h/v2/docs/swagger-ui-init.js",
                include_str!("fixtures/nest-init.js"),
            ),
        ]);
        let (spec, at) = found("https://h/v2/docs", &pages, &asked).unwrap();

        assert_eq!(spec["info"]["title"], "Messages");
        assert_eq!(
            spec["info"]["description"],
            "braces in text: { not an object } and a quote: \" {"
        );
        assert_eq!(at, "https://h/v2/docs/swagger-ui-init.js");

        // only the init script is followed, never the UI's own scripts
        assert_eq!(
            *asked.borrow(),
            ["https://h/v2/docs", "https://h/v2/docs/swagger-ui-init.js"]
        );

        // its relative server is resolved against where it was found
        let converted = convert(&spec, Some(&at)).unwrap();

        assert_eq!(converted.bundle.environments[0].1[0].value, "https://h/v2");
    }

    #[test]
    fn the_spec_next_to_a_docs_page_is_tried() {
        // a page that names nothing, its spec served at `<page>-json`
        let (pages, asked) = web(&[
            ("https://h/v2/docs", "<html><body>docs</body></html>"),
            ("https://h/v2/docs-json", STORE),
        ]);

        // what the browser's address bar shows, the fragment included
        let (_, at) = found("https://h/v2/docs#", &pages, &asked).unwrap();

        assert_eq!(at, "https://h/v2/docs-json");
    }

    #[test]
    fn a_folder_shows_its_specs_and_skips_what_git_ignores() {
        let root = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);

            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };

        write("api/openapi.yaml", NOTES);
        write("store.json", STORE);
        write("package.json", "{\"name\": \"not a spec\"}");
        write("build/openapi.json", STORE);
        write(".gitignore", "build/\n");
        write("README.md", "openapi");

        assert_eq!(
            specs_in(root.path()),
            [
                "store.json".to_owned(),
                "api/openapi.yaml".replace('/', std::path::MAIN_SEPARATOR_STR)
            ]
        );
        assert_eq!(specs_in(&root.path().join("missing")), Vec::<String>::new());
    }
}

mod refreshing {
    use super::*;
    use crate::model::{Item, Source};
    use crate::openapi::refresh::{Choices, DiffLine, LoadError, apply, diff, load_source, plan};
    use crate::store::{CollectionDir, CollectionStore, Library, Paths};

    /// the store spec a while later: an endpoint added with a new path
    /// parameter, the order schema changed, the health check gone
    fn newer() -> String {
        STORE
            .replace(
                "\"quantity\": { \"type\": \"integer\" }",
                "\"quantity\": { \"type\": \"number\" }",
            )
            .replace(
                "\"/health\": {\n      \"get\": { \"security\": [], \"responses\": { \"200\": { \"description\": \"up\" } } }\n    }",
                "\"/customers/{customerId}/orders\": {\n      \"get\": { \"tags\": [\"customers\"], \"summary\": \"Orders of a customer\", \"parameters\": [{ \"name\": \"customerId\", \"in\": \"path\", \"required\": true }], \"responses\": { \"200\": { \"description\": \"ok\" } } }\n    }",
            )
    }

    fn imported(root: &std::path::Path) -> (Library, CollectionDir) {
        let library = Library::new(Paths::under(root));
        let (dir, _) = library
            .import_spec(converted(STORE, None), &Source::default())
            .unwrap();

        (library, dir)
    }

    fn find(collection: &Collection, operation: &str) -> Option<(String, Request)> {
        fn walk(items: &[Item], operation: &str) -> Option<(String, Request)> {
            items.iter().find_map(|item| match item {
                Item::Folder(folder) => walk(&folder.items, operation),
                Item::Request(saved) => saved
                    .spec
                    .as_ref()
                    .filter(|s| s.operation == operation)
                    .map(|_| (saved.name.clone(), saved.request.clone())),
            })
        }

        walk(&collection.items, operation)
    }

    use crate::model::Collection;

    #[test]
    fn a_refresh_finds_what_is_new_changed_and_gone_and_applies_what_is_chosen() {
        assert_ne!(newer(), STORE, "the fixture did not change");

        let root = tempfile::tempdir().unwrap();
        let (_library, dir) = imported(root.path());

        // the user edits one request and renames another: neither is a
        // change in the spec
        let list = vec!["orders".to_owned(), "list-orders".to_owned()];
        let create = vec!["orders".to_owned(), "create-order".to_owned()];
        let mut edited = find(&dir.load().unwrap().collection, "GET /orders")
            .unwrap()
            .1;

        edited.params[0].value = "7".into();
        dir.save_request(&list, &edited).unwrap();

        let create = vec![create[0].clone(), dir.rename(&create, "My order").unwrap()];
        let collection = dir.load().unwrap().collection;
        let plan = plan(&collection, &converted(&newer(), None));

        assert_eq!(plan.unchanged, 2, "{plan:#?}");
        assert_eq!(plan.added.len(), 1);
        assert_eq!(
            plan.added[0].spec.operation,
            "GET /customers/{customerId}/orders"
        );
        assert_eq!(plan.added[0].folder.as_deref(), Some("customers"));
        assert_eq!(plan.changed.len(), 1);
        assert_eq!(plan.changed[0].name, "My order");
        assert_eq!(plan.changed[0].path, create);
        assert_eq!(plan.removed.len(), 1);
        assert_eq!(plan.removed[0].operation, "GET /health");
        assert_eq!(
            plan.variables
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>(),
            ["customerid"]
        );

        // the new one and the change are taken, the gone one kept
        apply(
            &dir,
            &collection,
            &plan,
            &Choices {
                add: vec![0],
                replace: vec![0],
                delete: Vec::new(),
            },
        )
        .unwrap();

        let after = dir.load().unwrap().collection;
        let (name, replaced) = find(&after, "POST /orders").unwrap();
        let new_fingerprint = &plan.changed[0].spec.fingerprint;

        assert_eq!(name, "My order");
        assert_eq!(replaced, plan.changed[0].new);
        assert!(find(&after, "GET /customers/{customerId}/orders").is_some());
        assert!(find(&after, "GET /health").is_some());
        assert_eq!(find(&after, "GET /orders").unwrap().1, edited);

        // the new variable is in every environment
        for environment in &after.environments {
            assert!(
                environment
                    .variables
                    .iter()
                    .any(|v| v.name.as_str() == "customerid")
            );
        }

        // a second refresh has only the gone one left
        let again = crate::openapi::refresh::plan(&after, &converted(&newer(), None));

        assert!(
            again.added.is_empty() && again.changed.is_empty(),
            "{again:#?}"
        );
        assert_eq!(again.removed.len(), 1);

        let Some(Item::Folder(orders)) = after.items.first() else {
            panic!("no folder");
        };
        let Some(Item::Request(saved)) = orders.items.iter().find(|i| i.name() == "My order")
        else {
            panic!("no request");
        };

        assert_eq!(&saved.spec.as_ref().unwrap().fingerprint, new_fingerprint);

        // deleting it leaves nothing to do
        apply(
            &dir,
            &after,
            &again,
            &Choices {
                delete: vec![0],
                ..Choices::default()
            },
        )
        .unwrap();

        let finally = crate::openapi::refresh::plan(
            &dir.load().unwrap().collection,
            &converted(&newer(), None),
        );

        assert!(finally.is_empty(), "{finally:#?}");
    }

    #[test]
    fn a_diff_marks_what_changed() {
        let lines =
            |text: &str| -> Vec<String> { text.split(' ').map(ToOwned::to_owned).collect() };

        assert_eq!(
            diff(&lines("a b c"), &lines("a x c d")),
            [
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Added("x".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );

        // a parameter turned off is a change too
        let on = Request {
            params: vec![KeyValue::new("q", "1")],
            ..Request::default()
        };
        let mut off = on.clone();

        off.params[0].enabled = false;

        let lines = crate::openapi::refresh::request_lines;

        assert_ne!(lines(&on), lines(&off));
        assert!(lines(&off)[1].ends_with("(off)"));
    }

    #[test]
    fn a_source_that_moved_is_reported_as_missing() {
        let root = tempfile::tempdir().unwrap();
        let no_fetch = |_: &str| -> Result<Fetched, String> { Err("offline".into()) };
        let source = |location: &std::path::Path, picked: Option<&str>| Source {
            location: location.display().to_string(),
            picked: picked.map(ToOwned::to_owned),
            ..Source::default()
        };

        assert!(matches!(
            load_source(&source(&root.path().join("gone.json"), None), no_fetch),
            Err(LoadError::Missing(_))
        ));

        std::fs::write(root.path().join("store.json"), STORE).unwrap();

        assert!(load_source(&source(root.path(), Some("store.json")), no_fetch).is_ok());
        assert!(matches!(
            load_source(&source(root.path(), Some("old.json")), no_fetch),
            Err(LoadError::Missing(_))
        ));

        std::fs::write(root.path().join("broken.json"), "{").unwrap();

        assert!(matches!(
            load_source(&source(&root.path().join("broken.json"), None), no_fetch),
            Err(LoadError::Failed(_))
        ));

        // a URL is fetched, and a failure is not taken for a moved file
        let url = Source {
            location: "https://h/openapi.json".into(),
            ..Source::default()
        };

        assert_eq!(
            load_source(&url, no_fetch),
            Err(LoadError::Failed("offline".into()))
        );
    }
}
