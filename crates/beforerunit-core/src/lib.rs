//! BeforeRunIt core: shell lexer, Tier 1 rule engine, AI merge rules and the
//! ghost-line renderer. No network I/O lives here.

pub mod config;
pub mod engine;
pub mod ghost;
pub mod lexer;
pub mod merge;
pub mod render;
pub mod rules;
pub mod schema;
#[cfg(feature = "io")]
pub mod painter;
#[cfg(feature = "io")]
pub mod proto;
#[cfg(feature = "io")]
pub mod store;

pub use engine::{analyze, Analysis, Engine};
pub use schema::Level;

/// Applies a compiled cache: installs user rules and returns the config.
#[cfg(feature = "io")]
pub fn apply(c: &store::Compiled) -> &config::Config {
    rules::install_user_rules(&c.cmds);
    &c.config
}
