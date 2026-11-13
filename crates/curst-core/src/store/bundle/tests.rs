use super::*;
use crate::model::{Auth, Body, KeyValue, Method};
use crate::store::{CollectionStore, Paths};
use crate::vars::VarName;

fn name(text: &str) -> VarName {
    VarName::new(text).unwrap()
}

fn library() -> (tempfile::TempDir, Library) {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(Paths::under(root.path().join("home")));

    (root, library)
}

/// a collection with a bit of everything: nested folders, each body and
/// auth kind, plain and secret variables, and an environment
fn shop(library: &Library) -> Collection {
    let dir = library.create_collection("Shop").unwrap();
    let orders = dir.create_folder(&[], "Orders").unwrap();
    let archived = dir
        .create_folder(std::slice::from_ref(&orders), "Archived")
        .unwrap();

    dir.create_request(
        std::slice::from_ref(&orders),
        "Create order",
        &Request {
            method: Method::Post,
            url: "{{base_url}}/orders".into(),
            headers: vec![KeyValue::new("Accept", "application/json")],
            body: Body::Json("{\"qty\": 1}".into()),
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            ..Request::default()
        },
    )
    .unwrap();
    dir.create_request(
        &[orders, archived],
        "Old",
        &Request {
            url: "{{base_url}}/old".into(),
            body: Body::Form(vec![KeyValue::new("a", "1")]),
            ..Request::default()
        },
    )
    .unwrap();
    dir.create_request(&[], "Health", &Request::default())
        .unwrap();

    let mut token = Variable::secret(name("token"));

    // a value in memory, as the editor holds it, must never be written
    token.value = "PLANTED-in-memory".into();
    dir.save_settings(
        "Shop",
        &[
            Variable::new(name("base_url"), "https://shop.example"),
            token,
        ],
    )
    .unwrap();

    let mut staging = dir.create_environment("Staging").unwrap();

    staging.variables = vec![
        Variable::new(name("base_url"), "https://staging.shop.example"),
        Variable::secret(name("password")),
    ];
    dir.save_environment(&staging).unwrap();

    dir.load().unwrap().collection
}

#[test]
fn a_collection_comes_back_from_its_bundle_as_a_new_one() {
    let (root, library) = library();
    let original = shop(&library);
    let path = root.path().join("shop.curst.toml");

    export(&original, &path).unwrap();

    let text = fs::read_to_string(&path).unwrap();

    assert!(text.starts_with("curst_bundle = 1\n"), "{text}");
    assert!(!text.contains("PLANTED"), "{text}");

    let (dir, bundle) = library.import_bundle(&path).unwrap();
    let imported = dir.load().unwrap().collection;
    let names =
        |c: &Collection| -> Vec<String> { c.items.iter().map(|i| i.name().to_owned()).collect() };

    // a second collection, with its own folder and ids
    assert_eq!(library.collections().unwrap().len(), 2);
    assert_ne!(imported.environments[0].id, original.environments[0].id);
    assert_eq!(bundle.warnings, Vec::<String>::new());

    assert_eq!(imported.name, "Shop");
    assert_eq!(names(&imported), names(&original));
    assert_eq!(imported.items, original.items);
    assert_eq!(imported.environments[0].name, "Staging");
    assert_eq!(
        imported.environments[0].variables,
        original.environments[0].variables
    );

    // secrets keep their names and lose any value
    let token = imported
        .variables
        .iter()
        .find(|v| v.name.as_str() == "token")
        .unwrap();

    assert!(token.secret);
    assert_eq!(token.value, "");
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let (root, library) = library();
    let original = shop(&library);
    let path = root.path().join("taken.curst.toml");

    fs::write(&path, "mine").unwrap();

    assert!(export(&original, &path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
}

#[test]
fn a_bundle_from_elsewhere_is_checked_before_anything_is_made() {
    let (root, library) = library();
    let path = root.path().join("in.curst.toml");
    let refused = |text: &str| {
        fs::write(&path, text).unwrap();

        let result = library.import_bundle(&path);

        assert!(result.is_err(), "{text}");
        assert_eq!(library.collections().unwrap().len(), 0, "{text}");
    };

    refused("curst_bundle = 2\nname = \"Future\"\n");
    refused("name = \"No version\"\n");
    refused("not toml at all [");
    refused(
        "curst_bundle = 1\nname = \"Bad\"\n\n[[items]]\ntype = \"request\"\nname = \"x\"\nmethod = \"BREW\"\n",
    );
    refused("curst_bundle = 1\nname = \"Bad\"\n\n[[variables]]\nname = \"no spaces\"\n");

    let deep = (0..40).fold(String::new(), |text, level| {
        let table = std::iter::repeat_n("items", level + 1)
            .collect::<Vec<_>>()
            .join(".");

        format!("{text}\n[[{table}]]\ntype = \"folder\"\nname = \"f\"\n")
    });

    refused(&format!("curst_bundle = 1\nname = \"Deep\"\n{deep}"));

    let big = fs::File::create(&path).unwrap();

    big.set_len(MAX_BYTES + 1).unwrap();

    assert!(matches!(read(&path), Err(StoreError::Invalid(_))));
}

#[test]
fn a_secret_value_written_into_a_bundle_is_not_used() {
    let (root, library) = library();
    let path = root.path().join("in.curst.toml");

    fs::write(
        &path,
        "curst_bundle = 1\nname = \"Leaky\"\n\n[[variables]]\nname = \"token\"\nvalue = \"oops\"\nsecret = true\n",
    )
    .unwrap();

    let (dir, bundle) = library.import_bundle(&path).unwrap();

    assert_eq!(bundle.warnings.len(), 1, "{:?}", bundle.warnings);
    assert_eq!(dir.load().unwrap().collection.variables[0].value, "");
}

#[test]
fn plain_credentials_are_found_before_export() {
    let (_root, library) = library();
    let mut collection = shop(&library);

    assert_eq!(plain_credentials(&collection), Vec::<String>::new());

    collection
        .variables
        .push(Variable::new(name("api_key"), "abc123"));

    if let Item::Request(saved) = &mut collection.items[1] {
        saved.request.headers = vec![KeyValue::new("Authorization", "Bearer real")];
    }

    let found = plain_credentials(&collection);

    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("the collection variable api_key"));
    assert!(found[1].starts_with("Health"));
}

#[test]
fn a_failure_partway_leaves_no_collection_behind() {
    let (root, library) = library();
    let original = shop(&library);
    let path = root.path().join("shop.curst.toml");

    export(&original, &path).unwrap();

    let before = library.collections().unwrap().len();
    let bundle = read(&path).unwrap();
    let mut made = None;

    // the requests and environments are written, then something fails
    let result = library.import_with(bundle, |dir, bundle| {
        fill(dir, bundle)?;
        made = Some(dir.root().to_owned());

        Err(StoreError::Invalid("the disk is full".into()))
    });

    let made = made.unwrap();

    assert!(matches!(result, Err(StoreError::Invalid(_))));
    assert!(!made.exists(), "{} is still there", made.display());
    assert_eq!(library.collections().unwrap().len(), before);
    assert!(
        library.registry().find(&made).unwrap().is_none(),
        "the registry still names it"
    );
}
