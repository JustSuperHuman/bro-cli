//! Durable, provider-neutral coordination. All I/O is blocking; call off the UI/runtime thread.
//! Terminal activity is an observation, never evidence of task completion.
mod context;
mod evidence;
mod model;
mod scheduler;
mod store;
mod transition;

pub use evidence::{ContentEntry, digest_bytes, digest_manifest};
pub use model::*;
pub use store::Engine;

#[cfg(test)]
mod tests;
