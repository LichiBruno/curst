//! one collection on disk: a folder with `collection.toml`, a tree of
//! folders and request files, and an `environments` folder
//!
//! items are addressed by their path of slugs from the top. a request
//! `<slug>.toml` and a folder `<slug>` never share a slug in one folder,
//! because a new slug is chosen against both

use std::fs;
use std::path::{Path, PathBuf};

use super::format::{
    CollectionFile, EnvironmentFile, FolderFile, RequestFile, to_toml, variables_from_file,
    variables_to_file,
};
use super::fs::{Node, node, not_plain, read_regular, write_atomic};
use super::{StoreError, ids, slug};
use crate::model::{Collection, EnvId, Environment, Folder, Item, Request, SavedRequest, Variable};

const COLLECTION_FILE: &str = "collection.toml";
const FOLDER_FILE: &str = "folder.toml";
const ENVIRONMENTS: &str = "environments";
const EXTENSION: &str = ".toml";

/// a folder's children by file name, and the names refused because they are
/// links or special files
type Children = (Vec<(String, Kind)>, Vec<String>);

/// environment files with their content, or why it could not be read
type EnvironmentFiles = Vec<(PathBuf, Result<EnvironmentFile, StoreError>)>;

/// a problem found in a file while loading: a file that could not be read,
/// or a secret whose value was written into it; the rest still loads
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadIssue {
    /// the file
    pub path: PathBuf,

    /// what is wrong with it
    pub message: String,
}

/// a loaded collection and the files that were skipped
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// the collection
    pub collection: Collection,

    /// files that could not be read
    pub issues: Vec<LoadIssue>,
}

/// reads and changes one collection; paths are lists of slugs from the top
pub trait CollectionStore: Send + Sync {
    /// the whole collection
    ///
    /// # Errors
    /// when `collection.toml` is missing or broken; other broken files are
    /// skipped and listed in [`Loaded::issues`]
    fn load(&self) -> Result<Loaded, StoreError>;

    /// saves a new request in the folder at `parent` and returns its slug
    ///
    /// # Errors
    /// when the folder does not exist or a file cannot be written
    fn create_request(
        &self,
        parent: &[String],
        name: &str,
        request: &Request,
    ) -> Result<String, StoreError>;

    /// overwrites the request at `path`, keeping its name
    ///
    /// # Errors
    /// when there is no request at `path` or it cannot be written
    fn save_request(&self, path: &[String], request: &Request) -> Result<(), StoreError>;

    /// creates a folder in the folder at `parent` and returns its slug
    ///
    /// # Errors
    /// when the parent does not exist or the folder cannot be created
    fn create_folder(&self, parent: &[String], name: &str) -> Result<String, StoreError>;

    /// renames a folder or request, its file included, and returns the new
    /// slug; the item keeps its place
    ///
    /// # Errors
    /// when there is nothing at `path` or a file cannot be changed
    fn rename(&self, path: &[String], name: &str) -> Result<String, StoreError>;

    /// moves a folder or request to the end of the folder at `parent` and
    /// returns its slug there, which changes on a collision
    ///
    /// # Errors
    /// when either end does not exist, or a folder would move into itself
    fn move_item(&self, path: &[String], parent: &[String]) -> Result<String, StoreError>;

    /// moves an item to position `index` among its siblings
    ///
    /// # Errors
    /// when there is nothing at `path` or the order cannot be written
    fn reorder(&self, path: &[String], index: usize) -> Result<(), StoreError>;

    /// deletes a request, or a folder with everything in it
    ///
    /// # Errors
    /// when there is nothing at `path` or it cannot be removed
    fn delete(&self, path: &[String]) -> Result<(), StoreError>;

    /// saves the collection's name and variables
    ///
    /// # Errors
    /// when `collection.toml` cannot be read or written
    fn save_settings(&self, name: &str, variables: &[Variable]) -> Result<(), StoreError>;

    /// creates an empty environment with a new id
    ///
    /// # Errors
    /// when its file cannot be written
    fn create_environment(&self, name: &str) -> Result<Environment, StoreError>;

    /// saves an environment's variables, found by its id
    ///
    /// # Errors
    /// when no environment has that id or its file cannot be written
    fn save_environment(&self, environment: &Environment) -> Result<(), StoreError>;

    /// renames an environment, its file included, and returns the new slug;
    /// the id stays the same
    ///
    /// # Errors
    /// when no environment has that id or its file cannot be changed
    fn rename_environment(&self, id: &EnvId, name: &str) -> Result<String, StoreError>;

    /// deletes an environment
    ///
    /// # Errors
    /// when no environment has that id or its file cannot be removed
    fn delete_environment(&self, id: &EnvId) -> Result<(), StoreError>;
}

/// a collection stored as a folder of TOML files
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionDir {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    Request,
}

/// an item found on disk
struct Located {
    parent: PathBuf,
    slug: String,
    kind: Kind,
}

impl Located {
    fn entry(&self) -> String {
        entry(self.kind, &self.slug)
    }

    fn path(&self) -> PathBuf {
        self.parent.join(self.entry())
    }
}

fn entry(kind: Kind, slug: &str) -> String {
    match kind {
        Kind::Folder => slug.to_owned(),
        Kind::Request => format!("{slug}{EXTENSION}"),
    }
}

fn not_found(path: &[String]) -> StoreError {
    StoreError::NotFound(path.join("/"))
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T, StoreError> {
    toml::from_str(text).map_err(|e| StoreError::parse(path, e.message()))
}

fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let text =
        read_regular(path)?.ok_or_else(|| StoreError::NotFound(path.display().to_string()))?;

    parse(path, &text)
}

impl CollectionDir {
    /// the collection in `root`, without touching the disk
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// makes `root` a new, empty collection
    ///
    /// # Errors
    /// when `root` already holds a collection or cannot be written
    pub fn create(root: impl Into<PathBuf>, name: &str) -> Result<Self, StoreError> {
        let dir = Self::open(root);
        let file = dir.root.join(COLLECTION_FILE);

        if file.exists() {
            return Err(StoreError::Invalid(format!(
                "{} is already a collection",
                dir.root.display()
            )));
        }

        let content = CollectionFile {
            name: name.to_owned(),
            order: Vec::new(),
            variables: Vec::new(),
        };

        write_atomic(&file, to_toml(&content).as_bytes())?;

        Ok(dir)
    }

    /// the folder holding the collection
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// whether `root` holds a collection
    #[must_use]
    pub fn exists(&self) -> bool {
        node(&self.root) == Node::Dir && node(&self.root.join(COLLECTION_FILE)) == Node::File
    }

    /// the folder at `parent`, every step of it a real folder
    fn folder_dir(&self, parent: &[String]) -> Result<PathBuf, StoreError> {
        if reserved(parent) {
            return Err(not_found(parent));
        }

        let mut dir = self.root.clone();

        if node(&dir) != Node::Dir {
            return Err(not_found(parent));
        }

        for slug in parent {
            dir.push(slug);

            if node(&dir) != Node::Dir {
                return Err(not_found(parent));
            }
        }

        Ok(dir)
    }

    fn locate(&self, path: &[String]) -> Result<Located, StoreError> {
        let (slug, parent) = path.split_last().ok_or_else(|| not_found(path))?;
        let parent = self.folder_dir(parent)?;
        let kind = if node(&parent.join(entry(Kind::Request, slug))) == Node::File {
            Kind::Request
        } else if node(&parent.join(slug)) == Node::Dir
            && !(path.len() == 1 && slug == ENVIRONMENTS)
        {
            Kind::Folder
        } else {
            return Err(not_found(path));
        };

        Ok(Located {
            parent,
            slug: slug.clone(),
            kind,
        })
    }

    /// whether `slug` is used in `dir`, by an item or by curst's own files
    fn taken(dir: &Path, top: bool, slug: &str) -> bool {
        slug == "folder"
            || slug == "collection"
            || (top && slug == ENVIRONMENTS)
            || dir.join(slug).exists()
            || dir.join(entry(Kind::Request, slug)).exists()
    }

    fn free_slug(dir: &Path, top: bool, name: &str, current: Option<&str>) -> String {
        slug::unique(&slug::slugify(name), |s| {
            Some(s) != current && Self::taken(dir, top, s)
        })
    }

    /// the file names of a folder's children, in display order: first as
    /// `order` lists them, then the rest by name; links and other special
    /// files are returned apart, never followed
    fn children(dir: &Path, top: bool, order: &[String]) -> Result<Children, StoreError> {
        let mut children = Vec::new();
        let mut refused = Vec::new();

        for entry in fs::read_dir(dir).map_err(|e| StoreError::io(dir, e))? {
            let entry = entry.map_err(|e| StoreError::io(dir, e))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };

            // dot files include curst's temporary files
            if name.starts_with('.') || name == COLLECTION_FILE || name == FOLDER_FILE {
                continue;
            }

            let kind = entry.file_type().map_err(|e| StoreError::io(dir, e))?;

            if kind.is_dir() {
                if !(top && name == ENVIRONMENTS) {
                    children.push((name, Kind::Folder));
                }
            } else if name.ends_with(EXTENSION) {
                if kind.is_file() {
                    children.push((name, Kind::Request));
                } else {
                    refused.push(name);
                }
            }
        }

        children.sort_by_cached_key(|(name, _)| {
            let listed = order.iter().position(|o| o == name).unwrap_or(usize::MAX);

            (listed, name.clone())
        });

        Ok((children, refused))
    }

    fn order_of(&self, parent: &[String]) -> Result<Vec<String>, StoreError> {
        let dir = self.folder_dir(parent)?;
        let listed = if parent.is_empty() {
            read_file::<CollectionFile>(&dir.join(COLLECTION_FILE))?.order
        } else {
            Self::folder_file(&dir)?.order
        };
        let (children, _) = Self::children(&dir, parent.is_empty(), &listed)?;

        Ok(children.into_iter().map(|(name, _)| name).collect())
    }

    fn folder_file(dir: &Path) -> Result<FolderFile, StoreError> {
        let path = dir.join(FOLDER_FILE);

        match read_regular(&path)? {
            Some(text) => parse(&path, &text),
            None => Ok(FolderFile {
                name: dir_name(dir),
                order: Vec::new(),
            }),
        }
    }

    fn write_order(&self, parent: &[String], order: Vec<String>) -> Result<(), StoreError> {
        let dir = self.folder_dir(parent)?;

        if parent.is_empty() {
            let path = dir.join(COLLECTION_FILE);
            let mut file: CollectionFile = read_file(&path)?;

            file.order = order;

            write_atomic(&path, to_toml(&file).as_bytes())
        } else {
            let mut file = Self::folder_file(&dir)?;

            file.order = order;

            write_atomic(&dir.join(FOLDER_FILE), to_toml(&file).as_bytes())
        }
    }

    fn load_items(
        dir: &Path,
        top: bool,
        order: &[String],
        issues: &mut Vec<LoadIssue>,
    ) -> Vec<Item> {
        let (children, refused) = match Self::children(dir, top, order) {
            Ok(found) => found,
            Err(err) => {
                issues.push(issue(dir, &err));

                return Vec::new();
            }
        };
        let mut items = Vec::new();

        for name in refused {
            let path = dir.join(name);

            issues.push(issue(&path, &not_plain(&path)));
        }

        for (name, kind) in children {
            let path = dir.join(&name);

            match kind {
                Kind::Folder => items.push(Item::Folder(Self::load_folder(&path, &name, issues))),
                Kind::Request => match load_request(&path, &name) {
                    Ok(request) => items.push(Item::Request(request)),
                    Err(err) => issues.push(issue(&path, &err)),
                },
            }
        }

        items
    }

    fn load_folder(dir: &Path, slug: &str, issues: &mut Vec<LoadIssue>) -> Folder {
        let path = dir.join(FOLDER_FILE);
        let file = match read_regular(&path)
            .and_then(|text| text.map(|t| parse::<FolderFile>(&path, &t)).transpose())
        {
            Ok(file) => file.unwrap_or_default(),
            Err(err) => {
                issues.push(issue(&path, &err));

                FolderFile::default()
            }
        };
        let name = if file.name.is_empty() {
            slug.to_owned()
        } else {
            file.name
        };

        Folder {
            slug: slug.to_owned(),
            name,
            items: Self::load_items(dir, false, &file.order, issues),
        }
    }

    fn environments_dir(&self) -> PathBuf {
        self.root.join(ENVIRONMENTS)
    }

    /// every environment file with its parsed content, broken ones as errors
    fn environment_files(&self) -> Result<EnvironmentFiles, StoreError> {
        let dir = self.environments_dir();

        match node(&dir) {
            Node::Missing => return Ok(Vec::new()),
            Node::Dir => {}
            Node::File | Node::Other => return Err(not_plain(&dir)),
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(&dir).map_err(|e| StoreError::io(&dir, e))? {
            let path = entry.map_err(|e| StoreError::io(&dir, e))?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(".");

            if name.starts_with('.') || !name.ends_with(EXTENSION) {
                continue;
            }

            // a link is listed as a problem rather than skipped silently
            let content = match node(&path) {
                Node::File => read_file::<EnvironmentFile>(&path),
                Node::Dir => continue,
                Node::Other | Node::Missing => Err(not_plain(&path)),
            };

            files.push((path, content));
        }

        files.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(files)
    }

    fn load_environments(&self, issues: &mut Vec<LoadIssue>) -> Vec<Environment> {
        let files = match self.environment_files() {
            Ok(files) => files,
            Err(err) => {
                issues.push(issue(&self.environments_dir(), &err));

                return Vec::new();
            }
        };
        let mut environments: Vec<Environment> = Vec::new();

        for (path, content) in files {
            let environment = content.and_then(|file| {
                let id = file.env_id().map_err(|e| StoreError::parse(&path, e))?;
                let read =
                    variables_from_file(file.variables).map_err(|e| StoreError::parse(&path, e))?;

                if environments.iter().any(|e| e.id == id) {
                    return Err(StoreError::parse(
                        &path,
                        format!("environment id {id} is used twice"),
                    ));
                }

                Ok((
                    Environment {
                        id,
                        slug: stem(&path),
                        name: file.name,
                        variables: read.variables,
                    },
                    read.warnings,
                ))
            });

            match environment {
                Ok((environment, warnings)) => {
                    issues.extend(warnings.into_iter().map(|message| LoadIssue {
                        path: path.clone(),
                        message,
                    }));
                    environments.push(environment);
                }
                Err(err) => issues.push(issue(&path, &err)),
            }
        }

        environments.sort_by_cached_key(|e| (e.name.to_lowercase(), e.slug.clone()));

        environments
    }

    fn find_environment(&self, id: &EnvId) -> Result<(PathBuf, EnvironmentFile), StoreError> {
        self.environment_files()?
            .into_iter()
            .find_map(|(path, file)| file.ok().filter(|f| f.id == id.as_str()).map(|f| (path, f)))
            .ok_or_else(|| StoreError::NotFound(format!("environment {id}")))
    }
}

/// whether `parent` points into the environments folder
fn reserved(parent: &[String]) -> bool {
    parent.first().is_some_and(|first| first == ENVIRONMENTS)
}

fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn issue(path: &Path, err: &StoreError) -> LoadIssue {
    let message = match err {
        StoreError::Io { source, .. } => source.to_string(),
        StoreError::Parse { message, .. } => message.clone(),
        other => other.to_string(),
    };

    LoadIssue {
        path: path.to_owned(),
        message,
    }
}

fn load_request(path: &Path, name: &str) -> Result<SavedRequest, StoreError> {
    let file: RequestFile = read_file(path)?;
    let (display, request) = file
        .into_request()
        .map_err(|e| StoreError::parse(path, e))?;
    let slug = name.strip_suffix(EXTENSION).unwrap_or(name).to_owned();

    Ok(SavedRequest {
        name: if display.is_empty() {
            slug.clone()
        } else {
            display
        },
        slug,
        request,
    })
}

impl CollectionStore for CollectionDir {
    fn load(&self) -> Result<Loaded, StoreError> {
        let path = self.root.join(COLLECTION_FILE);
        let file: CollectionFile = match read_regular(&path)? {
            Some(text) => parse(&path, &text)?,
            None => return Err(StoreError::NotFound(path.display().to_string())),
        };
        let read = variables_from_file(file.variables).map_err(|e| StoreError::parse(&path, e))?;
        let variables = read.variables;
        let mut issues: Vec<LoadIssue> = read
            .warnings
            .into_iter()
            .map(|message| LoadIssue {
                path: path.clone(),
                message,
            })
            .collect();
        let items = Self::load_items(&self.root, true, &file.order, &mut issues);
        let environments = self.load_environments(&mut issues);

        Ok(Loaded {
            collection: Collection {
                name: file.name,
                variables,
                items,
                environments,
            },
            issues,
        })
    }

    fn create_request(
        &self,
        parent: &[String],
        name: &str,
        request: &Request,
    ) -> Result<String, StoreError> {
        let dir = self.folder_dir(parent)?;
        let mut order = self.order_of(parent)?;
        let slug = Self::free_slug(&dir, parent.is_empty(), name, None);
        let content = to_toml(&RequestFile::new(name, request));

        write_atomic(&dir.join(entry(Kind::Request, &slug)), content.as_bytes())?;
        order.push(entry(Kind::Request, &slug));
        self.write_order(parent, order)?;

        Ok(slug)
    }

    fn save_request(&self, path: &[String], request: &Request) -> Result<(), StoreError> {
        let item = self.locate(path)?;

        if item.kind != Kind::Request {
            return Err(StoreError::Invalid(format!(
                "{} is a folder",
                path.join("/")
            )));
        }

        let file = item.path();
        let name = read_file::<RequestFile>(&file).map_or_else(|_| item.slug.clone(), |f| f.name);

        write_atomic(&file, to_toml(&RequestFile::new(&name, request)).as_bytes())
    }

    fn create_folder(&self, parent: &[String], name: &str) -> Result<String, StoreError> {
        let dir = self.folder_dir(parent)?;
        let mut order = self.order_of(parent)?;
        let slug = Self::free_slug(&dir, parent.is_empty(), name, None);
        let folder = dir.join(&slug);
        let content = FolderFile {
            name: name.to_owned(),
            order: Vec::new(),
        };

        fs::create_dir(&folder).map_err(|e| StoreError::io(&folder, e))?;
        write_atomic(&folder.join(FOLDER_FILE), to_toml(&content).as_bytes())?;
        order.push(slug.clone());
        self.write_order(parent, order)?;

        Ok(slug)
    }

    fn rename(&self, path: &[String], name: &str) -> Result<String, StoreError> {
        let item = self.locate(path)?;
        let parent = &path[..path.len() - 1];
        let mut order = self.order_of(parent)?;
        let slug = Self::free_slug(&item.parent, parent.is_empty(), name, Some(&item.slug));
        let target = item.parent.join(entry(item.kind, &slug));

        // the file moves first: a crash before the order is written only
        // loses the item's position
        if slug != item.slug {
            fs::rename(item.path(), &target).map_err(|e| StoreError::io(&target, e))?;
        }

        match item.kind {
            Kind::Request => {
                let mut file: RequestFile = read_file(&target)?;

                name.clone_into(&mut file.name);
                write_atomic(&target, to_toml(&file).as_bytes())?;
            }
            Kind::Folder => {
                let mut file = Self::folder_file(&target)?;

                name.clone_into(&mut file.name);
                write_atomic(&target.join(FOLDER_FILE), to_toml(&file).as_bytes())?;
            }
        }

        let old = item.entry();
        let new = entry(item.kind, &slug);

        for listed in &mut order {
            if *listed == old {
                listed.clone_from(&new);
            }
        }

        self.write_order(parent, order)?;

        Ok(slug)
    }

    fn move_item(&self, path: &[String], parent: &[String]) -> Result<String, StoreError> {
        let item = self.locate(path)?;
        let from = &path[..path.len() - 1];

        if from == parent {
            return Ok(item.slug);
        }

        if item.kind == Kind::Folder && parent.starts_with(path) {
            return Err(StoreError::Invalid(format!(
                "cannot move {} into itself",
                path.join("/")
            )));
        }

        let dir = self.folder_dir(parent)?;
        let mut source_order = self.order_of(from)?;
        let mut target_order = self.order_of(parent)?;
        let slug = slug::unique(&item.slug, |s| Self::taken(&dir, parent.is_empty(), s));
        let target = dir.join(entry(item.kind, &slug));

        fs::rename(item.path(), &target).map_err(|e| StoreError::io(&target, e))?;
        source_order.retain(|listed| *listed != item.entry());
        target_order.push(entry(item.kind, &slug));
        self.write_order(from, source_order)?;
        self.write_order(parent, target_order)?;

        Ok(slug)
    }

    fn reorder(&self, path: &[String], index: usize) -> Result<(), StoreError> {
        let item = self.locate(path)?;
        let parent = &path[..path.len() - 1];
        let mut order = self.order_of(parent)?;
        let name = item.entry();

        order.retain(|listed| *listed != name);
        order.insert(index.min(order.len()), name);

        self.write_order(parent, order)
    }

    fn delete(&self, path: &[String]) -> Result<(), StoreError> {
        let item = self.locate(path)?;
        let parent = &path[..path.len() - 1];
        let mut order = self.order_of(parent)?;
        let target = item.path();

        match item.kind {
            Kind::Request => fs::remove_file(&target),
            Kind::Folder => fs::remove_dir_all(&target),
        }
        .map_err(|e| StoreError::io(&target, e))?;

        order.retain(|listed| *listed != item.entry());

        self.write_order(parent, order)
    }

    fn save_settings(&self, name: &str, variables: &[Variable]) -> Result<(), StoreError> {
        let path = self.root.join(COLLECTION_FILE);
        let mut file: CollectionFile = read_file(&path)?;

        name.clone_into(&mut file.name);
        file.variables = variables_to_file(variables);

        write_atomic(&path, to_toml(&file).as_bytes())
    }

    fn create_environment(&self, name: &str) -> Result<Environment, StoreError> {
        let dir = self.environments_dir();
        let slug = slug::unique(&slug::slugify(name), |s| {
            dir.join(format!("{s}{EXTENSION}")).exists()
        });
        let environment = Environment {
            id: ids::env_id()?,
            slug,
            name: name.to_owned(),
            variables: Vec::new(),
        };
        let file = EnvironmentFile {
            id: environment.id.as_str().to_owned(),
            name: environment.name.clone(),
            variables: Vec::new(),
        };

        write_atomic(
            &dir.join(format!("{}{EXTENSION}", environment.slug)),
            to_toml(&file).as_bytes(),
        )?;

        Ok(environment)
    }

    fn save_environment(&self, environment: &Environment) -> Result<(), StoreError> {
        let (path, mut file) = self.find_environment(&environment.id)?;

        file.variables = variables_to_file(&environment.variables);

        write_atomic(&path, to_toml(&file).as_bytes())
    }

    fn rename_environment(&self, id: &EnvId, name: &str) -> Result<String, StoreError> {
        let (path, mut file) = self.find_environment(id)?;
        let dir = self.environments_dir();
        let current = stem(&path);
        let slug = slug::unique(&slug::slugify(name), |s| {
            s != current && dir.join(format!("{s}{EXTENSION}")).exists()
        });
        let target = dir.join(format!("{slug}{EXTENSION}"));

        if target != path {
            fs::rename(&path, &target).map_err(|e| StoreError::io(&target, e))?;
        }

        name.clone_into(&mut file.name);
        write_atomic(&target, to_toml(&file).as_bytes())?;

        Ok(slug)
    }

    fn delete_environment(&self, id: &EnvId) -> Result<(), StoreError> {
        let (path, _) = self.find_environment(id)?;

        fs::remove_file(&path).map_err(|e| StoreError::io(&path, e))
    }
}

#[cfg(test)]
mod tests;
