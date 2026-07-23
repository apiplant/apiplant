//! # apiplant-core
//!
//! Configuration, error handling, and the declarative resource model that the
//! rest of apiplant is built on. This crate has no async, no database and no
//! HTTP — it only knows how to turn an *app directory* on disk into typed
//! [`App`] data other crates act on.

pub mod app;
pub mod config;
pub mod defaults;
pub mod error;
pub mod schema;

pub use app::{App, TlsPaths};
pub use config::Config;
pub use error::{Error, Result};
pub use schema::{
    relation_name, Access, Field, FieldType, OnDelete, Permissions, Reference, Resource, Scope,
};
