use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum CompositorError {
    Backend(String),
    Renderer(String),
    Socket(std::io::Error),
    EventLoop(String),
    Screencopy(String),
    InvalidAction,
}

impl fmt::Display for CompositorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(msg) => write!(f, "backend initialization failed: {msg}"),
            Self::Renderer(msg) => write!(f, "renderer creation failed: {msg}"),
            Self::Socket(err) => write!(f, "wayland socket creation failed: {err}"),
            Self::EventLoop(msg) => write!(f, "event loop error: {msg}"),
            Self::Screencopy(msg) => write!(f, "screencopy failed: {msg}"),
            Self::InvalidAction => write!(f, "invalid action"),
        }
    }
}

impl Error for CompositorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Socket(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CompositorError {
    fn from(err: std::io::Error) -> Self {
        Self::Socket(err)
    }
}

pub type Result<T> = std::result::Result<T, CompositorError>;
