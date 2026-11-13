//! everything curst keeps on disk: paths, collections, the registry of
//! known collections and global variables
//!
//! every file is written atomically, through a temporary file and a rename;
//! the registry is also guarded by a lock file, because more than one curst
//! process may change it at once

mod collection;
mod format;
mod fs;
mod ids;
mod library;
mod paths;
mod registry;
mod slug;

use std::io;
use std::path::{Path, PathBuf};

pub use collection::{CollectionDir, CollectionStore, LoadIssue, Loaded};
pub use library::Library;
pub use paths::Paths;
pub use registry::{Entry, Registry};

/// why a store operation failed
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// reading or writing a file failed
    #[error("{}: {source}", path.display())]
    Io {
        /// the file or folder
        path: PathBuf,

        /// the underlying error
        source: io::Error,
    },

    /// a file is not valid TOML or does not match its format
    #[error("{}: {message}", path.display())]
    Parse {
        /// the file
        path: PathBuf,

        /// what is wrong
        message: String,
    },

    /// no home directory, so no default location
    #[error("no home directory found; set CURST_CONFIG_DIR and CURST_DATA_DIR")]
    NoHome,

    /// no folder, request or environment at the given address
    #[error("not found: {0}")]
    NotFound(String),

    /// the operation makes no sense, such as moving a folder into itself
    #[error("{0}")]
    Invalid(String),
}

impl StoreError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }

    pub(crate) fn parse(path: &Path, message: impl Into<String>) -> Self {
        Self::Parse {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}
