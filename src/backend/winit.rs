use std::time::Duration;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ExportMem,
            damage::OutputDamageTracker,
            element::surface::WaylandSurfaceRenderElement,
            gles::{GlesRenderer, GlesTarget},
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    reexports::wayland_server::protocol::wl_shm::Format,
    utils::{Physical, Rectangle, Size, Transform},
    wayland::shm,
};

use crate::{CompositorError, ProjectWC, Result, protocols::wlr_screencopy::Screencopy};

pub fn init_winit(event_loop: &mut EventLoop<ProjectWC>, state: &mut ProjectWC) -> Result<()> {
    let (mut winit_backend, winit) =
        winit::init::<GlesRenderer>().map_err(|e| CompositorError::Backend(format!("{:?}", e)))?;

    let physical_properties = PhysicalProperties {
        size: (0, 0).into(),
        subpixel: Subpixel::Unknown,
        make: "projectwc".into(),
        model: "winit".into(),
        serial_number: "Unknown".into(),
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
                state.apply_layout().ok();
            }
            WinitEvent::Input(event) => state.handle_input_event(event),
            WinitEvent::Redraw => {
                let size = winit_backend.window_size();
                let damage = Rectangle::from_size(size);

                let pending_screencopy = state.pending_screencopy.take();

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

                if let Some(screencopy) = pending_screencopy
                    && screencopy.output() == &output
                {
                    let (renderer, framebuffer) =
                        winit_backend.bind().expect("failed to bind for screencopy");
                    if let Err(err) = render_screencopy(
                        renderer,
                        &framebuffer,
                        &output,
                        screencopy,
                        state.start_time,
                    ) {
                        tracing::warn!("screencopy failed: {err:?}");
                    }
                }

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

fn render_screencopy(
    renderer: &mut GlesRenderer,
    target: &GlesTarget<'_>,
    _output: &Output,
    screencopy: Screencopy,
    start_time: std::time::Instant,
) -> Result<()> {
    let size = screencopy.buffer_size();
    let buffer_size = Size::<i32, Physical>::from((size.w, size.h))
        .to_logical(1)
        .to_buffer(1, Transform::Normal);
    let rect = Rectangle::from_size(buffer_size);

    let mapping = renderer
        .copy_framebuffer(target, rect, Fourcc::Xrgb8888)
        .map_err(|e| CompositorError::Screencopy(format!("copy_framebuffer: {e:?}")))?;
    let bytes = renderer
        .map_texture(&mapping)
        .map_err(|e| CompositorError::Screencopy(format!("map_texture: {e:?}")))?;

    shm::with_buffer_contents_mut(&screencopy.buffer, |shm_buffer, shm_len, buffer_data| {
        if buffer_data.format != Format::Xrgb8888
            || buffer_data.width != size.w
            || buffer_data.height != size.h
            || buffer_data.stride != size.w * 4
            || shm_len != buffer_data.stride as usize * buffer_data.height as usize
        {
            tracing::warn!(
                "buffer validation failed: format={:?} size={}x{} stride={} len={}",
                buffer_data.format,
                buffer_data.width,
                buffer_data.height,
                buffer_data.stride,
                shm_len
            );
            return;
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(shm_buffer.cast::<u8>(), shm_len) };
        dst.copy_from_slice(&bytes[..shm_len]);
    })
    .map_err(|e| CompositorError::Screencopy(format!("shm buffer: {e:?}")))?;

    screencopy.submit(start_time.elapsed());

    Ok(())
}
