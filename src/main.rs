use projectwc::{state::ProjectWC, CompositorError, Result};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let use_udev = args.iter().any(|a| a == "--tty" || a == "--udev");
    let debug = args.iter().any(|a| a == "--debug");

    if debug {
        unsafe { std::env::set_var("RUST_LOG", "debug") };
    }

    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    let mut event_loop: EventLoop<ProjectWC> =
        EventLoop::try_new().map_err(|e| CompositorError::EventLoop(e.to_string()))?;

    let display = Display::new().map_err(|e| CompositorError::Backend(e.to_string()))?;
    let mut state = ProjectWC::new(display, event_loop.handle(), event_loop.get_signal());

    let use_udev = use_udev
        || std::env::var("PROJECTWC_BACKEND")
            .map(|v| v == "udev")
            .unwrap_or(false)
        || (std::env::var("WAYLAND_DISPLAY").is_err() && std::env::var("DISPLAY").is_err());

    if use_udev {
        tracing::info!("using udev backend");
        projectwc::backend::udev::init_udev(&mut event_loop, &mut state)?;
    } else {
        tracing::info!("using winit backend");
        projectwc::backend::winit::init_winit(&mut event_loop, &mut state)?;
    }

    let spawn_cmd = args.iter().find(|a| !a.starts_with('-') && *a != &args[0]);
    if let Some(cmd) = spawn_cmd {
        std::process::Command::new(cmd).spawn().ok();
    }

    event_loop
        .run(None, &mut state, |_| {})
        .map_err(|e| CompositorError::EventLoop(e.to_string()))?;

    Ok(())
}
