//! core library of curst, a terminal API client
//!
//! it holds everything that does not need a terminal: the request model,
//! variable resolution, storage, secrets, the HTTP executor and the
//! importers. the `curst` binary builds its TUI and CLI on top of it

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::panic))]

pub mod http;
pub mod model;
pub mod secrets;
pub mod store;
pub mod url;
pub mod vars;

#[cfg(test)]
mod security_tests;
