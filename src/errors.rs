use thiserror::Error;

#[derive(Error, Debug)]
pub enum CompositorError {
    #[error("backend initialization failed: {0}")]
    Backend(String),

    #[error("renderer creation failed: {0}")]
    Renderer(String),

    #[error("wayland socket creation failed: {0}")]
    Socket(#[from] std::io::Error),

    #[error("event loop error: {0}")]
    EventLoop(String),
}

pub type Result<T> = std::result::Result<T, CompositorError>;
