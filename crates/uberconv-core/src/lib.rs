//! Core of uberconv: formats, the `Converter` trait and the conversion graph.
//!
//! Every converter only declares *edges* (`from -> to`, with a cost). The
//! [`Registry`] finds the cheapest chain of edges between any two formats, so
//! adding one converter automatically connects its formats to everything that
//! is already reachable (e.g. YAML -> CSV works through JSON without anyone
//! writing a YAML -> CSV converter).

mod context;
mod converter;
mod error;
mod format;
mod registry;

pub use context::{CancelToken, Context, Options};
pub use converter::{Converter, Cost, Edge, Job, LOSSY_PENALTY};
pub use error::{Error, Result};
pub use format::{Family, Format};
pub use registry::{BatchEvent, BatchItem, Plan, Registry, Step};
