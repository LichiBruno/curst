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
