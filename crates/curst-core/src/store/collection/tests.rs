use super::*;
use crate::model::{Auth, Body, KeyValue, Method};
use crate::vars::VarName;

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

fn new_collection() -> (tempfile::TempDir, CollectionDir) {
    let dir = tempfile::tempdir().unwrap();
    let collection = CollectionDir::create(dir.path().join("c"), "Pets").unwrap();

    (dir, collection)
}

/// the slugs of the top level, folders shown as `slug/`
fn top(store: &CollectionDir) -> Vec<String> {
    let loaded = store.load().unwrap();

    assert_eq!(loaded.issues, Vec::new());

    loaded
        .collection
        .items
        .iter()
        .map(|item| match item {
            Item::Folder(f) => format!("{}/", f.slug),
            Item::Request(r) => r.slug.clone(),
        })
        .collect()
}

fn get(url: &str) -> Request {
    Request {
        url: url.into(),
        ..Request::default()
    }
}

#[test]
fn a_new_collection_is_empty() {
    let (_dir, store) = new_collection();
    let loaded = store.load().unwrap();

    assert!(store.exists());
    assert_eq!(loaded.collection.name, "Pets");
    assert_eq!(loaded.collection.items, Vec::new());
    assert_eq!(loaded.collection.environments, Vec::new());
    assert!(matches!(
        CollectionDir::create(store.root(), "again"),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn loading_needs_a_collection_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = CollectionDir::open(dir.path());

    assert!(!store.exists());
    assert!(matches!(store.load(), Err(StoreError::NotFound(_))));

    fs::write(dir.path().join(COLLECTION_FILE), "name = ").unwrap();

    assert!(matches!(store.load(), Err(StoreError::Parse { .. })));

    fs::write(
        dir.path().join(COLLECTION_FILE),
        "name = \"x\"\n[[variables]]\nname = \"bad name\"\n",
    )
    .unwrap();

    assert!(matches!(store.load(), Err(StoreError::Parse { .. })));
}

#[test]
fn the_request_file_is_readable_toml() {
    let (_dir, store) = new_collection();
    let mut off = KeyValue::new("X-Debug", "1");
    off.enabled = false;

    let request = Request {
        method: Method::Post,
        url: "{{base_url}}/pets".into(),
        params: vec![],
        headers: vec![KeyValue::new("Accept", "application/json"), off],
        body: Body::Json("{\n  \"name\": \"Rex\"\n}".into()),
        auth: Auth::Bearer {
            token: "{{token}}".into(),
        },
    };
    let slug = store.create_request(&[], "Create pet", &request).unwrap();
    let text = fs::read_to_string(store.root().join("create-pet.toml")).unwrap();

    assert_eq!(slug, "create-pet");
    assert_eq!(
        text,
        r#"name = "Create pet"
method = "POST"
url = "{{base_url}}/pets"

[[headers]]
name = "Accept"
value = "application/json"

[[headers]]
name = "X-Debug"
value = "1"
enabled = false

[body]
kind = "json"
text = """
{
  "name": "Rex"
}"""

[auth]
kind = "bearer"
token = "{{token}}"
"#
    );

    let loaded = store.load().unwrap().collection;

    assert_eq!(
        loaded.item(&path(&["create-pet"])),
        Some(&Item::Request(SavedRequest {
            slug: "create-pet".into(),
            name: "Create pet".into(),
            request,
            spec: None,
        }))
    );
}

#[test]
fn order_follows_creation_and_survives_reloads() {
    let (_dir, store) = new_collection();

    store.create_request(&[], "Zebra", &get("z")).unwrap();
    store.create_folder(&[], "Users").unwrap();
    store.create_request(&[], "Alpha", &get("a")).unwrap();
    store
        .create_request(&path(&["users"]), "List", &get("l"))
        .unwrap();

    assert_eq!(top(&store), ["zebra", "users/", "alpha"]);
    assert_eq!(top(&store), ["zebra", "users/", "alpha"]);

    let loaded = store.load().unwrap().collection;

    assert_eq!(
        loaded.item(&path(&["users", "list"])).map(Item::name),
        Some("List")
    );
    assert_eq!(
        loaded.item(&path(&["users"])).map(Item::name),
        Some("Users")
    );
}

#[test]
fn hand_made_files_are_appended_and_stale_entries_ignored() {
    let (_dir, store) = new_collection();

    store.create_request(&[], "B", &get("b")).unwrap();
    store.create_request(&[], "A", &get("a")).unwrap();
    fs::write(store.root().join("d.toml"), "name = \"D\"").unwrap();
    fs::write(store.root().join("c.toml"), "name = \"\"").unwrap();
    fs::create_dir(store.root().join("plain")).unwrap();
    fs::write(store.root().join("plain/x.toml"), "name = \"X\"").unwrap();

    // a stale entry, a temporary file and files that are not requests
    store
        .write_order(&[], path(&["gone.toml", "b.toml", "a.toml"]))
        .unwrap();
    fs::write(store.root().join(".curst-tmp123"), "junk").unwrap();
    fs::write(store.root().join("notes.md"), "junk").unwrap();

    assert_eq!(top(&store), ["b", "a", "c", "d", "plain/"]);

    let loaded = store.load().unwrap().collection;

    assert_eq!(loaded.item(&path(&["c"])).map(Item::name), Some("c"));
    assert_eq!(
        loaded.item(&path(&["plain"])).map(Item::name),
        Some("plain")
    );
    assert_eq!(
        loaded.item(&path(&["plain", "x"])).map(Item::name),
        Some("X")
    );

    store
        .create_request(&path(&["plain"]), "Y", &get("y"))
        .unwrap();

    assert!(store.root().join("plain/folder.toml").is_file());
}

#[test]
fn broken_files_are_reported_and_skipped() {
    let (_dir, store) = new_collection();

    store.create_request(&[], "Good", &get("g")).unwrap();
    store.create_folder(&[], "Dir").unwrap();
    fs::write(store.root().join("broken.toml"), "name = ").unwrap();
    fs::write(
        store.root().join("method.toml"),
        "name = \"m\"\nmethod = \"BREW\"",
    )
    .unwrap();
    fs::write(store.root().join("dir/folder.toml"), "order = 1").unwrap();

    let loaded = store.load().unwrap();
    let mut broken: Vec<_> = loaded
        .issues
        .iter()
        .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    broken.sort();

    assert_eq!(broken, ["broken.toml", "folder.toml", "method.toml"]);
    assert_eq!(loaded.collection.items.len(), 2);
    assert_eq!(loaded.collection.items[1].name(), "dir");
}

#[test]
fn slugs_avoid_collisions_and_curst_files() {
    let (_dir, store) = new_collection();
    let request = get("x");

    assert_eq!(
        store.create_request(&[], "Get Pet", &request).unwrap(),
        "get-pet"
    );
    assert_eq!(
        store.create_request(&[], "get pet!", &request).unwrap(),
        "get-pet-2"
    );
    assert_eq!(store.create_folder(&[], "Get Pet").unwrap(), "get-pet-3");
    assert_eq!(
        store.create_request(&[], "Environments", &request).unwrap(),
        "environments-2"
    );
    assert_eq!(
        store.create_request(&[], "Collection", &request).unwrap(),
        "collection-2"
    );
    assert_eq!(store.create_folder(&[], "Folder").unwrap(), "folder-2");
    assert_eq!(
        store.create_request(&[], "日本", &request).unwrap(),
        "untitled"
    );
    assert_eq!(store.create_request(&[], "NUL", &request).unwrap(), "nul-1");

    let inner = path(&["folder-2"]);

    assert_eq!(
        store
            .create_request(&inner, "Environments", &request)
            .unwrap(),
        "environments"
    );
    assert_eq!(
        store.create_request(&inner, "Folder", &request).unwrap(),
        "folder-2"
    );
}

#[test]
fn saving_keeps_the_name_and_checks_the_target() {
    let (_dir, store) = new_collection();

    store.create_request(&[], "Pet", &get("one")).unwrap();
    store.create_folder(&[], "Dir").unwrap();
    store.save_request(&path(&["pet"]), &get("two")).unwrap();

    let loaded = store.load().unwrap().collection;
    let Some(Item::Request(saved)) = loaded.item(&path(&["pet"])) else {
        panic!("no request");
    };

    assert_eq!(
        (saved.name.as_str(), saved.request.url.as_str()),
        ("Pet", "two")
    );
    assert!(matches!(
        store.save_request(&path(&["dir"]), &get("x")),
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.save_request(&path(&["nope"]), &get("x")),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.create_request(&path(&["nope"]), "x", &get("x")),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.create_request(&path(&["environments"]), "x", &get("x")),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(store.delete(&[]), Err(StoreError::NotFound(_))));
}

#[test]
fn renaming_keeps_the_place_and_moves_the_file() {
    let (_dir, store) = new_collection();

    store.create_request(&[], "One", &get("1")).unwrap();
    store.create_folder(&[], "Two").unwrap();
    store.create_request(&[], "Three", &get("3")).unwrap();
    store
        .create_request(&path(&["two"]), "Inner", &get("i"))
        .unwrap();

    assert_eq!(store.rename(&path(&["one"]), "Uno").unwrap(), "uno");
    assert_eq!(store.rename(&path(&["two"]), "Dos").unwrap(), "dos");
    assert_eq!(top(&store), ["uno", "dos/", "three"]);
    assert!(!store.root().join("one.toml").exists());

    // the same slug keeps the file, a taken one gets a number
    assert_eq!(store.rename(&path(&["uno"]), "UNO!").unwrap(), "uno");
    assert_eq!(store.rename(&path(&["three"]), "Uno").unwrap(), "uno-2");

    let loaded = store.load().unwrap().collection;

    assert_eq!(loaded.item(&path(&["uno"])).map(Item::name), Some("UNO!"));
    assert_eq!(loaded.item(&path(&["dos"])).map(Item::name), Some("Dos"));
    assert_eq!(
        loaded.item(&path(&["dos", "inner"])).map(Item::name),
        Some("Inner")
    );
}

#[test]
fn moving_between_folders() {
    let (_dir, store) = new_collection();

    store.create_folder(&[], "A").unwrap();
    store.create_folder(&path(&["a"]), "B").unwrap();
    store.create_request(&[], "Pet", &get("1")).unwrap();
    store
        .create_request(&path(&["a"]), "Pet", &get("2"))
        .unwrap();

    assert_eq!(
        store.move_item(&path(&["pet"]), &path(&["a"])).unwrap(),
        "pet-2"
    );
    assert_eq!(top(&store), ["a/"]);
    assert_eq!(
        store
            .move_item(&path(&["a", "pet"]), &path(&["a"]))
            .unwrap(),
        "pet"
    );
    assert_eq!(store.move_item(&path(&["a", "b"]), &[]).unwrap(), "b");
    assert_eq!(top(&store), ["a/", "b/"]);

    let loaded = store.load().unwrap().collection;
    let Some(Item::Folder(a)) = loaded.item(&path(&["a"])) else {
        panic!("no folder");
    };

    assert_eq!(
        a.items.iter().map(Item::slug).collect::<Vec<_>>(),
        ["pet", "pet-2"]
    );
    assert!(matches!(
        store.move_item(&path(&["a"]), &path(&["a"])),
        Err(StoreError::Invalid(_))
    ));

    store.create_folder(&path(&["a"]), "Deep").unwrap();

    assert!(matches!(
        store.move_item(&path(&["a"]), &path(&["a", "deep"])),
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.move_item(&path(&["b"]), &path(&["nope"])),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
fn reordering_and_deleting() {
    let (_dir, store) = new_collection();

    for name in ["A", "B", "C"] {
        store.create_request(&[], name, &get(name)).unwrap();
    }

    store.create_folder(&[], "D").unwrap();
    store
        .create_request(&path(&["d"]), "Inner", &get("i"))
        .unwrap();
    store.reorder(&path(&["c"]), 0).unwrap();

    assert_eq!(top(&store), ["c", "a", "b", "d/"]);

    store.reorder(&path(&["c"]), 99).unwrap();

    assert_eq!(top(&store), ["a", "b", "d/", "c"]);

    store.delete(&path(&["b"])).unwrap();
    store.delete(&path(&["d"])).unwrap();

    assert_eq!(top(&store), ["a", "c"]);
    assert!(!store.root().join("d").exists());
    assert!(matches!(
        store.delete(&path(&["b"])),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
fn settings_keep_the_order() {
    let (_dir, store) = new_collection();
    let variables = vec![Variable::new(VarName::new("base").unwrap(), "https://x")];

    store.create_request(&[], "B", &get("b")).unwrap();
    store.create_request(&[], "A", &get("a")).unwrap();
    store.save_settings("Renamed", &variables).unwrap();

    let loaded = store.load().unwrap().collection;

    assert_eq!(loaded.name, "Renamed");
    assert_eq!(loaded.variables, variables);
    assert_eq!(top(&store), ["b", "a"]);
}

#[test]
fn environments_keep_their_id_through_renames() {
    let (_dir, store) = new_collection();
    let mut dev = store.create_environment("Dev").unwrap();
    let prod = store.create_environment("Prod").unwrap();
    let dev_too = store.create_environment("dev").unwrap();

    assert_eq!((dev.slug.as_str(), dev_too.slug.as_str()), ("dev", "dev-2"));

    dev.variables = vec![Variable::new(VarName::new("host").unwrap(), "localhost")];
    store.save_environment(&dev).unwrap();

    assert_eq!(
        store.rename_environment(&dev.id, "Development").unwrap(),
        "development"
    );
    assert_eq!(store.rename_environment(&prod.id, "PROD").unwrap(), "prod");

    store.delete_environment(&dev_too.id).unwrap();

    let loaded = store.load().unwrap().collection;
    let names: Vec<_> = loaded
        .environments
        .iter()
        .map(|e| e.name.as_str())
        .collect();

    assert_eq!(names, ["Development", "PROD"]);
    assert_eq!(
        loaded.environment(&dev.id).unwrap().variables,
        dev.variables
    );
    assert_eq!(loaded.environment(&dev.id).unwrap().slug, "development");
    assert!(matches!(
        store.delete_environment(&dev_too.id),
        Err(StoreError::NotFound(_))
    ));

    // environments never show up as a folder of requests
    assert_eq!(top(&store), Vec::<String>::new());
}

#[test]
fn broken_and_duplicate_environments_are_reported() {
    let (_dir, store) = new_collection();
    let dev = store.create_environment("Dev").unwrap();
    let dir = store.root().join(ENVIRONMENTS);

    fs::copy(dir.join("dev.toml"), dir.join("copy.toml")).unwrap();
    fs::write(dir.join("broken.toml"), "id = ").unwrap();
    fs::write(dir.join("badid.toml"), "id = \"UP\"\nname = \"x\"").unwrap();
    fs::write(
        dir.join("badvar.toml"),
        "id = \"v1\"\nname = \"x\"\n[[variables]]\nname = \"a b\"",
    )
    .unwrap();
    fs::write(dir.join(".curst-tmp"), "junk").unwrap();

    let loaded = store.load().unwrap();
    let mut broken: Vec<_> = loaded
        .issues
        .iter()
        .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    broken.sort();

    assert_eq!(
        broken,
        ["badid.toml", "badvar.toml", "broken.toml", "dev.toml"]
    );
    assert_eq!(loaded.collection.environments.len(), 1);
    assert_eq!(loaded.collection.environments[0].id, dev.id);
}

#[test]
fn secret_values_written_by_hand_are_reported() {
    let (_dir, store) = new_collection();

    fs::write(
        store.root().join(COLLECTION_FILE),
        "name = \"Pets\"\n\n[[variables]]\nname = \"key\"\nvalue = \"oops\"\nsecret = true\n",
    )
    .unwrap();
    fs::create_dir(store.root().join(ENVIRONMENTS)).unwrap();
    fs::write(
        store.root().join("environments/dev.toml"),
        "id = \"e1\"\nname = \"Dev\"\n\n[[variables]]\nname = \"token\"\nvalue = \"oops\"\nsecret = true\n",
    )
    .unwrap();

    let loaded = store.load().unwrap();
    let mut files: Vec<_> = loaded
        .issues
        .iter()
        .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    files.sort();

    assert_eq!(files, ["collection.toml", "dev.toml"]);
    assert!(
        loaded
            .issues
            .iter()
            .all(|i| i.message.contains("is a secret"))
    );
    assert_eq!(loaded.collection.variables[0].value, "");
    assert_eq!(loaded.collection.environments[0].variables[0].value, "");
}

#[cfg(unix)]
#[test]
fn links_are_reported_and_never_followed() {
    use std::os::unix::fs::symlink;

    let (dir, store) = new_collection();
    let outside = dir.path().join("outside.toml");

    fs::write(&outside, "name = \"Outside\"\nurl = \"leak\"").unwrap();
    store.create_request(&[], "Real", &get("r")).unwrap();
    store.create_folder(&[], "Sub").unwrap();

    // a loop back to the parent, a file outside the collection, and a
    // linked environment file
    symlink(store.root(), store.root().join("sub/loop")).unwrap();
    symlink(&outside, store.root().join("stolen.toml")).unwrap();
    fs::create_dir(store.root().join(ENVIRONMENTS)).unwrap();
    symlink(&outside, store.root().join("environments/env.toml")).unwrap();

    let loaded = store.load().unwrap();
    let mut refused: Vec<_> = loaded
        .issues
        .iter()
        .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    refused.sort();

    assert_eq!(refused, ["env.toml", "stolen.toml"]);
    assert!(
        loaded
            .issues
            .iter()
            .all(|i| i.message.contains("links are not followed"))
    );

    // the folder loads, without the loop inside it
    let names: Vec<_> = loaded.collection.items.iter().map(Item::slug).collect();

    assert_eq!(names, ["real", "sub"]);

    let Some(Item::Folder(sub)) = loaded.collection.item(&path(&["sub"])) else {
        panic!("no folder");
    };

    assert_eq!(sub.items, Vec::new());

    // a linked folder is not a place to write to either
    symlink(dir.path(), store.root().join("away")).unwrap();

    assert!(matches!(
        store.create_request(&path(&["away"]), "x", &get("x")),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.delete(&path(&["stolen"])),
        Err(StoreError::NotFound(_))
    ));
}

#[cfg(unix)]
#[test]
fn a_linked_collection_root_is_not_a_collection() {
    let (dir, store) = new_collection();
    let link = dir.path().join("linked");

    std::os::unix::fs::symlink(store.root(), &link).unwrap();

    assert!(store.exists());
    assert!(!CollectionDir::open(&link).exists());
}

#[test]
fn a_request_keeps_its_spec_origin_through_every_change() {
    let (_dir, store) = new_collection();
    let spec = SpecOrigin {
        operation: "GET /pets/{petId}".into(),
        fingerprint: "a1b2".into(),
    };
    let folder = store.create_folder(&[], "Pets").unwrap();
    let slug = store
        .create_from_spec(&[], "Get pet", &Request::default(), Some(&spec))
        .unwrap();
    let origin = |path: &[String]| match store.load().unwrap().collection.item(path) {
        Some(Item::Request(saved)) => saved.spec.clone(),
        other => panic!("{other:?}"),
    };

    assert_eq!(origin(std::slice::from_ref(&slug)), Some(spec.clone()));

    // a save, a rename and a move each keep it
    let edited = Request {
        url: "{{base_url}}/pets/7".into(),
        ..Request::default()
    };

    store
        .save_request(std::slice::from_ref(&slug), &edited)
        .unwrap();

    let slug = store.rename(&[slug], "Pet by id").unwrap();
    let slug = store
        .move_item(&[slug], std::slice::from_ref(&folder))
        .unwrap();
    let path = vec![folder, slug];

    assert_eq!(origin(&path), Some(spec.clone()));

    // a refresh replaces the request and its fingerprint, not its name
    let newer = SpecOrigin {
        fingerprint: "c3d4".into(),
        ..spec
    };

    store
        .replace_from_spec(&path, &Request::default(), &newer)
        .unwrap();

    let collection = store.load().unwrap().collection;

    assert_eq!(collection.item(&path).map(Item::name), Some("Pet by id"));
    assert_eq!(origin(&path), Some(newer));
}

#[test]
fn the_source_of_a_collection_survives_its_settings() {
    let (_dir, store) = new_collection();
    let source = Source {
        location: "https://api.example.com/openapi.json".into(),
        headers: vec![KeyValue::new("Authorization", "Bearer {{spec_token}}")],
        picked: None,
    };

    store.save_source(Some(&source)).unwrap();
    store.save_settings("Renamed", &[]).unwrap();

    let collection = store.load().unwrap().collection;

    assert_eq!(collection.name, "Renamed");
    assert_eq!(collection.source, Some(source));

    store.save_source(None).unwrap();

    assert_eq!(store.load().unwrap().collection.source, None);
}

#[test]
fn folders_and_the_collection_keep_the_auth_they_pass_on() {
    let (_dir, store) = new_collection();
    let users = store.create_folder(&[], "Users").unwrap();
    let bearer = Auth::Bearer {
        token: "{{token}}".into(),
    };

    store
        .create_request(&[], "Plain", &get("http://h/"))
        .unwrap();
    store
        .create_request(
            &[],
            "Open",
            &Request {
                auth: Auth::None,
                ..get("http://h/")
            },
        )
        .unwrap();
    store.save_auth(&[], &bearer).unwrap();
    store
        .save_auth(std::slice::from_ref(&users), &Auth::None)
        .unwrap();

    let collection = store.load().unwrap().collection;
    let auth = |slug: &str| match collection.item(&path(&[slug])) {
        Some(Item::Request(r)) => r.request.auth.clone(),
        _ => panic!("no {slug}"),
    };

    // a request saying nothing inherits; one that says none keeps none
    assert_eq!(auth("plain"), Auth::Inherit);
    assert_eq!(auth("open"), Auth::None);
    assert_eq!(collection.auth, bearer);
    assert_eq!(
        collection.inherited_auth(&[]),
        (bearer.clone(), "Pets".into())
    );
    assert_eq!(
        collection.inherited_auth(&path(&["users"])),
        (Auth::None, "Users".into())
    );

    // a folder set back to inheriting passes on what is above it
    store
        .save_auth(std::slice::from_ref(&users), &Auth::Inherit)
        .unwrap();

    let collection = store.load().unwrap().collection;

    assert_eq!(
        collection.inherited_auth(&path(&["users"])),
        (bearer, "Pets".into())
    );

    let text = std::fs::read_to_string(store.root().join("open.toml")).unwrap();

    assert!(text.contains("kind = \"none\""), "{text}");
}

#[test]
fn workflow_steps_follow_their_requests_through_renames_and_moves() {
    use crate::model::{Capture, Extract, Step, Workflow};

    let (_dir, store) = new_collection();
    let auth = store.create_folder(&[], "Auth").unwrap();
    let login = store
        .create_request(std::slice::from_ref(&auth), "Login", &get("http://h/login"))
        .unwrap();
    let workflow = Workflow {
        name: "Log in".into(),
        steps: vec![Step {
            request: vec![auth.clone(), login],
            keep: vec![
                Capture {
                    variable: VarName::new("token").unwrap(),
                    from: Extract::Json("$.access_token".into()),
                },
                Capture {
                    variable: VarName::new("session").unwrap(),
                    from: Extract::Header("X-Session".into()),
                },
            ],
        }],
    };

    store
        .save_workflows(std::slice::from_ref(&workflow))
        .unwrap();

    assert_eq!(
        store.load().unwrap().collection.workflows,
        std::slice::from_ref(&workflow)
    );

    let step = || {
        store.load().unwrap().collection.workflows[0].steps[0]
            .request
            .clone()
    };

    // a renamed request, then a renamed folder, then a move to the top
    let renamed = store.rename(&path(&["auth", "login"]), "Sign in").unwrap();

    assert_eq!(step(), [auth.clone(), renamed.clone()]);

    let folder = store.rename(&path(&["auth"]), "Session").unwrap();

    assert_eq!(step(), [folder.clone(), renamed.clone()]);

    let moved = store.move_item(&[folder, renamed], &[]).unwrap();

    assert_eq!(step(), [moved]);
}

#[test]
fn an_api_key_keeps_its_name_and_place() {
    use crate::model::KeyPlace;

    let (_dir, store) = new_collection();
    let header = Auth::ApiKey {
        name: "X-Api-Key".into(),
        value: "{{api_key}}".into(),
        place: KeyPlace::Header,
    };
    let query = Auth::ApiKey {
        name: "key".into(),
        value: "{{api_key}}".into(),
        place: KeyPlace::Query,
    };

    store.save_auth(&[], &header).unwrap();
    store
        .create_request(
            &[],
            "Search",
            &Request {
                auth: query.clone(),
                ..get("http://h/")
            },
        )
        .unwrap();

    let collection = store.load().unwrap().collection;

    assert_eq!(collection.auth, header);
    assert!(matches!(
        collection.item(&path(&["search"])),
        Some(Item::Request(r)) if r.request.auth == query
    ));

    let text = std::fs::read_to_string(store.root().join("search.toml")).unwrap();

    assert!(text.contains("kind = \"api_key\""), "{text}");
    assert!(text.contains("in = \"query\""), "{text}");
}
