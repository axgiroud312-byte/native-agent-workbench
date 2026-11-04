//! Shared business objects owned by the workbench, independent of UI, protocol and storage.
//!
//! This crate defines identity and lifecycle facts. Command/event routing, mutation rules,
//! persistence, and Agent I/O belong to subsequent tasks. See `docs/architecture/domain-model.md`.

mod agent;
mod configuration;
mod content;
mod contract;
pub mod examples;
mod identity;
mod input;
mod interaction;
mod lifecycle;
mod port;
mod preferences;
mod reducer;
mod session;
mod snapshot;
mod state;
mod tool;
mod transport;

pub use agent::*;
pub use configuration::*;
pub use content::*;
pub use contract::*;
pub use identity::*;
pub use input::*;
pub use interaction::*;
pub use lifecycle::*;
pub use port::*;
pub use preferences::*;
pub use reducer::*;
pub use session::*;
pub use snapshot::*;
pub use state::WorkbenchState;
pub use tool::*;
pub use transport::*;
