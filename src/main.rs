use oxwc::{
    CompositorError, Result,
    state::{Oxwc, init_wayland_listener},
};
use smithay::{
    backend::{
        renderer::{
            Color32F, Frame, Renderer as _,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, RenderElement, surface::WaylandSurfaceRenderElement,
            },
            glow::GlowRenderer,
        },
        winit::{self, WinitEvent},
    },
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
    },
    utils::{Physical, Rectangle, Scale},
};
use std::time::Duration;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let spawn_cmd: Option<String> = std::env::args().nth(1);

    let mut event_loop: EventLoop<Oxwc> =
        EventLoop::try_new().map_err(|e| CompositorError::EventLoop(e.to_string()))?;

    let display: Display<Oxwc> =
        Display::new().map_err(|e| CompositorError::Backend(e.to_string()))?;

    let (mut state, mut display) = Oxwc::new(display, event_loop.handle());

    let socket_name = init_wayland_listener(&event_loop.handle());

    let (mut winit_backend, mut winit_event_loop) =
        winit::init::<GlowRenderer>().map_err(|e| CompositorError::Backend(format!("{:?}", e)))?;

    let window_size = winit_backend.window_size();
    let winit_output = state.add_output("winit".to_string(), window_size.w, window_size.h);

    let _damage_tracker = OutputDamageTracker::from_output(&winit_output);

    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };

    if let Some(cmd) = spawn_cmd {
        std::process::Command::new(&cmd).spawn().ok();
    }

    state
        .seat
        .add_keyboard(Default::default(), 200, 25)
        .expect("failed to add keyboard");
    state.seat.add_pointer();

    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, state| {
            let mut should_close = false;

            winit_event_loop.dispatch_new_events(|event| match event {
                WinitEvent::Resized { size, .. } => {
                    winit_output.change_current_state(
                        Some(smithay::output::Mode {
                            size,
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                }
                WinitEvent::Input(event) => state.handle_input_event(event),
                WinitEvent::Focus(_) => {}
                WinitEvent::Redraw => {}
                WinitEvent::CloseRequested => should_close = true,
            });

            if should_close {
                state.running = false;
                return TimeoutAction::Drop;
            }

            let size = winit_backend.window_size();
            let damage: Rectangle<i32, Physical> = Rectangle::new((0, 0).into(), size);

            let (renderer, mut target) = winit_backend.bind().expect("bind failed");
            let clear_color = Color32F::new(0.1, 0.1, 0.1, 1.0);
            let scale = Scale::from(1.0);

            let mut render_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = Vec::new();
            for window in state.space.elements() {
                let window_location = state
                    .space
                    .element_geometry(window)
                    .map(|geo| geo.loc)
                    .unwrap_or_default();

                let elements: Vec<_> = window
                    .render_elements::<WaylandSurfaceRenderElement<GlowRenderer>>(
                        renderer,
                        window_location.to_physical_precise_round(1),
                        scale,
                        1.0,
                    );
                render_elements.extend(elements);
            }

            let mut frame = renderer
                .render(&mut target, size, smithay::utils::Transform::Flipped180)
                .expect("render failed");

            frame.clear(clear_color, &[damage]).expect("clear failed");

            for element in &render_elements {
                let src = element.src();
                let dst = element.geometry(scale);
                element
                    .draw(&mut frame, src, dst, &[damage], &[damage])
                    .ok();
            }

            let _ = frame.finish();

            drop(target);
            winit_backend
                .submit(Some(&[damage]))
                .expect("submit failed");

            state.space.elements().for_each(|window| {
                window.send_frame(
                    &winit_output,
                    Duration::ZERO,
                    Some(Duration::ZERO),
                    |_, _| Some(winit_output.clone()),
                );
            });

            state.space.refresh();

            TimeoutAction::ToDuration(Duration::from_millis(16))
        })
        .expect("failed to insert timer");

    while state.running {
        display.dispatch_clients(&mut state).unwrap();
        display.flush_clients().unwrap();

        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .map_err(|e| CompositorError::EventLoop(e.to_string()))?;
    }

    Ok(())
}
