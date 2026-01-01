pub mod backend;
pub mod errors;
pub mod grabs;
mod handlers;
pub mod input;
pub mod layout;
pub mod protocols;
pub mod state;

pub use errors::{CompositorError, Result};
pub use state::ProjectWC;
