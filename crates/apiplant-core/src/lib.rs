//! # apiplant-core
//!
//! Configuration, error handling, and the declarative resource model that the
//! rest of apiplant is built on. This crate has no async, no database and no
//! HTTP — it only knows how to turn an *app directory* on disk into typed
//! [`App`] data other crates act on.

pub mod agent;
pub mod app;
pub mod config;
pub mod defaults;
pub mod env;
pub mod error;
pub mod schema;

pub use agent::{Agent, AgentAiOverride, AgentMeta, AgentPermissions, AgentStorage, AgentTool};
pub use app::{App, TlsPaths};
pub use config::{
    AiConfig, CacheConfig, Config, EmailConfig, PaymentsConfig, QueuesConfig, SmtpConfig,
    StorageConfig,
};
pub use env::{expand_document, parse_toml};
pub use error::{Error, Result};
pub use schema::{
    relation_name, Access, AuthEvent, Field, FieldType, HookEvent, Hooks, OnDelete, Permissions,
    Publishes, Reference, Resource, Scope,
};
