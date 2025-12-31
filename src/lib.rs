pub mod backend;
pub mod errors;
pub mod grabs;
pub mod input;
pub mod layout;
pub mod shell;
pub mod state;
pub mod protocols;

pub use errors::{CompositorError, Result};
pub use state::ProjectWC;
