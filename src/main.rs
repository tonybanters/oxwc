use oxwc::{CompositorError, Result, state::Oxwc};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut event_loop: EventLoop<Oxwc> =
        EventLoop::try_new().map_err(|e| CompositorError::EventLoop(e.to_string()))?;

    let display = Display::new().map_err(|e| CompositorError::Backend(e.to_string()))?;
    let mut state = Oxwc::new(display, event_loop.handle(), event_loop.get_signal());

    oxwc::backend::winit::init_winit(&mut event_loop, &mut state)?;

    let spawn_cmd: Option<String> = std::env::args().nth(1);
    if let Some(cmd) = spawn_cmd {
        std::process::Command::new(&cmd).spawn().ok();
    }

    event_loop
        .run(None, &mut state, |_| {})
        .map_err(|e| CompositorError::EventLoop(e.to_string()))?;

    Ok(())
}
