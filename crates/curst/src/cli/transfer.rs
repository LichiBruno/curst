//! `curst import` and `curst export`: collections in and out of curst, as
//! the UI moves them

use std::fmt::Write as _;
use std::path::Path;

use curst_core::http::{Cancel, HttpExecutor, RedirectRule};
use curst_core::model::Source;
use curst_core::openapi::{self, Converted};
use curst_core::store::{Library, bundle};

use super::{ExportArgs, Fail, ImportArgs};

/// whether the file at `path` is a bundle rather than a spec
fn is_bundle(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines()
            .any(|line| line.trim_start().starts_with("curst_bundle"))
    })
}

/// the spec at `source` converted, and the source to keep for a refresh
fn spec(
    executor: &dyn HttpExecutor,
    source: &str,
    picked: Option<&str>,
) -> Result<(Converted, Source), Fail> {
    let failed = |e: &dyn std::fmt::Display| Fail::setup(e.to_string());

    if crate::app::is_url(source) {
        let cancel = Cancel::new();
        let (value, found_at) = openapi::discover(source, |url| {
            openapi::fetch_with(executor, url, &[], &cancel, &RedirectRule::Any)
        })
        .map_err(|e| failed(&e))?;
        let converted = openapi::convert(&value, Some(&found_at)).map_err(|e| failed(&e))?;

        return Ok((
            converted,
            Source {
                location: source.to_owned(),
                ..Source::default()
            },
        ));
    }

    let path = std::path::absolute(source).map_err(|e| failed(&e))?;

    if !path.exists() {
        return Err(Fail::setup(format!(
            "there is nothing at {}",
            path.display()
        )));
    }

    let (file, picked) = if path.is_dir() {
        let specs = openapi::specs_in(&path);
        let chosen = match (picked, specs.as_slice()) {
            (Some(name), _) if specs.iter().any(|s| s == name) => name.to_owned(),
            (Some(name), _) => {
                return Err(Fail::setup(format!(
                    "{name} is not a spec in {}",
                    path.display()
                )));
            }
            (None, []) => {
                return Err(Fail::setup(format!(
                    "no OpenAPI or Swagger spec is in {} or the folders under it",
                    path.display()
                )));
            }
            (None, [only]) => only.clone(),
            (None, several) => {
                return Err(Fail::setup(format!(
                    "{} holds several specs; choose one with --spec:\n  {}",
                    path.display(),
                    several.join("\n  ")
                )));
            }
        };

        (path.join(&chosen), Some(chosen))
    } else {
        (path.clone(), None)
    };

    let text = openapi::read_file(&file).map_err(|e| failed(&e))?;
    let value = openapi::parse(&text).map_err(|e| failed(&e))?;
    let converted = openapi::convert(&value, None).map_err(|e| failed(&e))?;

    Ok((
        converted,
        Source {
            location: path.display().to_string(),
            picked,
            ..Source::default()
        },
    ))
}

/// puts the first environment of a new collection in use, as the UI does,
/// and says which
fn first_environment(library: &Library, workspace: &crate::app::Workspace) -> Option<String> {
    let first = workspace.collection.environments.first()?;

    library
        .registry()
        .set_active_environment(&workspace.root, Some(&first.id))
        .ok()?;

    Some(first.name.clone())
}

/// imports `args.source` as a new collection and says what came in
pub(crate) fn import(
    library: &Library,
    executor: &dyn HttpExecutor,
    args: &ImportArgs,
) -> Result<String, Fail> {
    let source = args.source.trim();
    let failed = |e: &dyn std::fmt::Display| Fail::setup(e.to_string());
    let mut report = String::new();

    if !crate::app::is_url(source) && Path::new(source).is_file() && is_bundle(Path::new(source)) {
        let (dir, bundle) = library
            .import_bundle(Path::new(source))
            .map_err(|e| failed(&e))?;

        let (workspace, _) = crate::storage::open(library, &dir, true).map_err(|e| failed(&e))?;

        let _ = write!(report, "imported {} from the bundle", bundle.name);

        if let Some(env) = first_environment(library, &workspace) {
            let _ = write!(report, "\n  environment in use: {env}");
        }

        for warning in &bundle.warnings {
            let _ = write!(report, "\n  {warning}");
        }

        return Ok(report);
    }

    let (converted, source) = spec(executor, source, args.spec.as_deref())?;
    let (requests, folders) = (converted.requests, converted.folders);
    let secrets = converted.secrets.clone();
    let credentials = converted.credentials.clone();
    let (dir, bundle) = library
        .import_spec(converted, &source)
        .map_err(|e| failed(&e))?;

    let (workspace, _) = crate::storage::open(library, &dir, true).map_err(|e| failed(&e))?;

    let _ = write!(
        report,
        "imported {}: {requests} requests in {folders} folders",
        bundle.name
    );

    if let Some(env) = first_environment(library, &workspace) {
        let _ = write!(report, "\n  environment in use: {env}");
    }

    for warning in &bundle.warnings {
        let _ = write!(report, "\n  {warning}");
    }

    if !credentials.is_empty() {
        let _ = write!(
            report,
            "\n  {} example values looked like credentials and were left out",
            credentials.len()
        );
    }

    if !secrets.is_empty() {
        let _ = write!(
            report,
            "\n  set these secrets in curst before sending: {}",
            secrets.join(", ")
        );
    }

    Ok(report)
}

/// writes the collection `args` names as a bundle
pub(crate) fn export(library: &Library, args: &ExportArgs) -> Result<String, Fail> {
    let (collections, _) = super::load(library)?;
    let workspace = super::find(&collections, &args.collection)?;
    let plain = bundle::plain_credentials(&workspace.collection);

    if !plain.is_empty() && !args.force {
        return Err(Fail::setup(format!(
            "{} holds what looks like credentials in plain text, which a bundle would carry:\n  {}\nmake them secrets in curst, or pass --force to write it anyway",
            workspace.collection.name,
            plain.join("\n  ")
        )));
    }

    bundle::export(&workspace.collection, &args.file).map_err(|e| Fail::setup(e.to_string()))?;

    Ok(format!(
        "wrote {} to {}",
        workspace.collection.name,
        args.file.display()
    ))
}
