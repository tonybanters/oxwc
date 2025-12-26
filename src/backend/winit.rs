use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{CompositorError, ProjectWC};

pub fn init_winit(
    event_loop: &mut EventLoop<ProjectWC>,
    state: &mut ProjectWC,
) -> Result<(), CompositorError> {
    let (mut winit_backend, winit) =
        winit::init::<GlesRenderer>().map_err(|e| CompositorError::Backend(format!("{:?}", e)))?;

    let physical_properties = PhysicalProperties {
        size: (0, 0).into(),
        subpixel: Subpixel::Unknown,
        make: "projectwc".to_string(),
        model: "winit".into(),
    };

    let mode = Mode {
        size: winit_backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new("projectwc".into(), physical_properties);
    output.create_global::<ProjectWC>(&state.display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, None);
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Set WAYLAND_DISPLAY for child processes
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
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
            WinitEvent::Redraw => {
                let size = winit_backend.window_size();
                let damage = Rectangle::from_size(size);

                {
                    let (renderer, mut framebuffer) =
                        winit_backend.bind().expect("failed to bind winit window");
                    smithay::desktop::space::render_output::<
                        _,
                        WaylandSurfaceRenderElement<GlesRenderer>,
                        _,
                        _,
                    >(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&state.space],
                        &[],
                        &mut damage_tracker,
                        [0.1, 0.1, 0.1, 1.0],
                    )
                    .unwrap();
                }

                winit_backend
                    .submit(Some(&[damage]))
                    .expect("failed to submit damage");

                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                });

                state.space.refresh();
                state.display_handle.flush_clients().unwrap();

                // Ask for redraw to schedule new frame.
                winit_backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        })
        .map_err(|e| CompositorError::Backend(format!("{:?}", e)))?;

    Ok(())
}
