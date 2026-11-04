//! Shared business objects owned by the workbench, independent of UI, protocol and storage.
//!
//! This crate defines identity and lifecycle facts. Command/event routing, mutation rules,
//! persistence, and Agent I/O belong to subsequent tasks. See `docs/architecture/domain-model.md`.

mod agent;
mod configuration;
mod content;
mod identity;
mod input;
mod interaction;
mod lifecycle;
mod session;
mod tool;

pub use agent::*;
pub use configuration::*;
pub use content::*;
pub use identity::*;
pub use input::*;
pub use interaction::*;
pub use lifecycle::*;
pub use session::*;
pub use tool::*;
