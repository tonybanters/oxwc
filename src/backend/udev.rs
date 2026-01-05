use std::{collections::HashMap, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Modifier,
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmNode, GbmBufferedSurface,
            NodeType,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::OutputDamageTracker,
            element::{
                solid::{SolidColorBuffer, SolidColorRenderElement},
                Kind,
            },
            gles::GlesRenderer,
            Bind,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{UdevBackend, UdevEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, RegistrationToken,
        },
        drm::control::{self, connector, crtc, Device as ControlDevice, ModeTypeFlags},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Physical, Point, Rectangle, Scale, Transform},
};

use crate::{CompositorError, ProjectWC, Result};

mod render_element_types {
    use smithay::{
        backend::renderer::{
            element::{solid::SolidColorRenderElement, surface::WaylandSurfaceRenderElement},
            ImportAll, Renderer,
        },
        render_elements,
    };

    render_elements! {
        pub OutputRenderElements<R> where R: ImportAll;
        Surface=WaylandSurfaceRenderElement<R>,
        Cursor=SolidColorRenderElement,
    }

    impl<R: Renderer> std::fmt::Debug for OutputRenderElements<R> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Surface(e) => f.debug_tuple("Surface").field(e).finish(),
                Self::Cursor(e) => f.debug_tuple("Cursor").field(e).finish(),
                _ => f.write_str("OutputRenderElements"),
            }
        }
    }
}

pub use render_element_types::OutputRenderElements;

pub struct UdevData {
    pub session: LibSeatSession,
    pub primary_gpu: DrmNode,
    pub devices: HashMap<DrmNode, Device>,
}

pub struct Device {
    pub drm: DrmDevice,
    pub drm_node: DrmNode,
    pub gbm: GbmDevice<DrmDeviceFd>,
    pub gles: GlesRenderer,
    pub surfaces: HashMap<crtc::Handle, Surface>,
    pub registration_token: RegistrationToken,
}

pub struct Surface {
    pub output: Output,
    pub gbm_surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    pub damage_tracker: OutputDamageTracker,
}

pub fn init_udev(
    event_loop: &mut smithay::reexports::calloop::EventLoop<ProjectWC>,
    state: &mut ProjectWC,
) -> Result<()> {
    let (mut session, notifier) = LibSeatSession::new()
        .map_err(|e| CompositorError::Backend(format!("failed to create session: {e:?}")))?;

    let seat_name = session.seat();
    tracing::info!("session created on seat {seat_name}");

    let udev_backend = UdevBackend::new(&seat_name)
        .map_err(|e| CompositorError::Backend(format!("failed to create udev backend: {e:?}")))?;

    let primary_gpu = udev_backend
        .device_list()
        .filter_map(|(node, _)| {
            DrmNode::from_dev_id(node.into())
                .ok()?
                .node_with_type(NodeType::Render)?
                .ok()
        })
        .next()
        .ok_or_else(|| CompositorError::Backend("no suitable GPU found".into()))?;

    tracing::info!("using primary GPU: {primary_gpu}");

    let udev_data = UdevData {
        session: session.clone(),
        primary_gpu,
        devices: HashMap::new(),
    };

    state.udev = Some(udev_data);

    for (device_id, path) in udev_backend.device_list() {
        if let Err(e) = add_device(state, &mut session, device_id.into(), &path) {
            tracing::warn!("failed to add device {path:?}: {e:?}");
        }
    }

    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());

    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|_| CompositorError::Backend("failed to assign seat to libinput".into()))?;

    tracing::info!("libinput initialized on seat {seat_name}");

    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| {
            state.handle_input_event(event);
        })
        .map_err(|e| CompositorError::Backend(format!("failed to insert libinput: {e:?}")))?;

    event_loop
        .handle()
        .insert_source(notifier, |event, _, state| match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused");
                if let Some(udev) = &mut state.udev {
                    for device in udev.devices.values_mut() {
                        device.drm.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session activated");
                if let Some(udev) = &mut state.udev {
                    for device in udev.devices.values_mut() {
                        device.drm.activate(false).ok();
                        for surface in device.surfaces.values_mut() {
                            surface.gbm_surface.reset_buffers();
                        }
                    }
                }
            }
        })
        .map_err(|e| {
            CompositorError::Backend(format!("failed to insert session notifier: {e:?}"))
        })?;

    event_loop
        .handle()
        .insert_source(udev_backend, |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                if let Some(udev) = &mut state.udev {
                    let mut session = udev.session.clone();
                    if let Err(e) = add_device(state, &mut session, device_id.into(), &path) {
                        tracing::warn!("failed to add device {path:?}: {e:?}");
                    }
                }
            }
            UdevEvent::Changed { device_id } => {
                tracing::info!("device changed: {device_id}");
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id.into()) {
                    remove_device(state, node);
                }
            }
        })
        .map_err(|e| CompositorError::Backend(format!("failed to insert udev: {e:?}")))?;

    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    schedule_initial_render(&state.loop_handle);

    Ok(())
}

fn add_device(
    state: &mut ProjectWC,
    session: &mut LibSeatSession,
    device_id: u64,
    path: &Path,
) -> Result<()> {
    let node = DrmNode::from_dev_id(device_id)
        .map_err(|e| CompositorError::Backend(format!("failed to get DRM node: {e:?}")))?;

    let fd = session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC)
        .map_err(|e| CompositorError::Backend(format!("failed to open device: {e:?}")))?;

    let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, drm_notifier) = DrmDevice::new(device_fd.clone(), true)
        .map_err(|e| CompositorError::Backend(format!("failed to create DRM device: {e:?}")))?;

    let gbm = GbmDevice::new(device_fd.clone())
        .map_err(|e| CompositorError::Backend(format!("failed to create GBM device: {e:?}")))?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone()) }
        .map_err(|e| CompositorError::Backend(format!("failed to create EGL display: {e:?}")))?;

    let egl_context = EGLContext::new(&egl_display)
        .map_err(|e| CompositorError::Backend(format!("failed to create EGL context: {e:?}")))?;

    let gles = unsafe { GlesRenderer::new(egl_context) }
        .map_err(|e| CompositorError::Backend(format!("failed to create GLES renderer: {e:?}")))?;

    let registration_token = state
        .loop_handle
        .insert_source(drm_notifier, move |event, metadata, state| {
            handle_drm_event(state, node, event, metadata);
        })
        .map_err(|e| CompositorError::Backend(format!("failed to insert DRM notifier: {e:?}")))?;

    let device = Device {
        drm,
        drm_node: node,
        gbm,
        gles,
        surfaces: HashMap::new(),
        registration_token,
    };

    let udev = state
        .udev
        .as_mut()
        .ok_or_else(|| CompositorError::Backend("udev data not initialized".into()))?;

    udev.devices.insert(node, device);

    scan_connectors(state, node)?;

    Ok(())
}

fn remove_device(state: &mut ProjectWC, node: DrmNode) {
    let Some(udev) = &mut state.udev else { return };

    if let Some(device) = udev.devices.remove(&node) {
        for (_, surface) in device.surfaces {
            state.space.unmap_output(&surface.output);
        }
        state.loop_handle.remove(device.registration_token);
    }
}

fn scan_connectors(state: &mut ProjectWC, node: DrmNode) -> Result<()> {
    let udev = state
        .udev
        .as_mut()
        .ok_or_else(|| CompositorError::Backend("udev data not initialized".into()))?;

    let device = udev
        .devices
        .get_mut(&node)
        .ok_or_else(|| CompositorError::Backend("device not found".into()))?;

    let res_handles = device
        .drm
        .resource_handles()
        .map_err(|e| CompositorError::Backend(format!("failed to get resource handles: {e:?}")))?;

    let connectors: Vec<_> = res_handles.connectors().iter().copied().collect();

    for connector in connectors {
        let connector_info = device.drm.get_connector(connector, true).map_err(|e| {
            CompositorError::Backend(format!("failed to get connector info: {e:?}"))
        })?;

        if connector_info.state() != connector::State::Connected {
            continue;
        }

        let modes = connector_info.modes();
        let drm_mode = modes
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| modes.first())
            .copied()
            .ok_or_else(|| CompositorError::Backend("no mode found for connector".into()))?;

        let crtc = find_crtc_for_connector(device, &res_handles, connector)?;

        let physical_properties = PhysicalProperties {
            size: connector_info
                .size()
                .map(|(w, h)| (w as i32, h as i32))
                .unwrap_or((0, 0))
                .into(),
            subpixel: Subpixel::Unknown,
            make: "projectwc".into(),
            model: format!("{:?}", connector_info.interface()),
            serial_number: "Unknown".into(),
        };

        let output = Output::new(
            format!(
                "{}-{}",
                connector_info.interface().as_str(),
                connector_info.interface_id()
            ),
            physical_properties,
        );

        let smithay_mode = Mode {
            size: (drm_mode.size().0 as i32, drm_mode.size().1 as i32).into(),
            refresh: (drm_mode.vrefresh() * 1000) as i32,
        };

        output.create_global::<ProjectWC>(&state.display_handle);
        output.change_current_state(Some(smithay_mode), Some(Transform::Normal), None, None);
        output.set_preferred(smithay_mode);

        let drm_surface = device
            .drm
            .create_surface(crtc, drm_mode, &[connector])
            .map_err(|e| {
                CompositorError::Backend(format!("failed to create DRM surface: {e:?}"))
            })?;

        let allocator = GbmAllocator::new(
            device.gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

        let renderer_formats = device.gles.egl_context().dmabuf_render_formats().clone();

        let filtered_formats: Vec<_> = renderer_formats
            .iter()
            .copied()
            .filter(|format| {
                !matches!(
                    format.modifier,
                    Modifier::I915_y_tiled_ccs
                        | Modifier::I915_y_tiled_gen12_rc_ccs
                        | Modifier::I915_y_tiled_gen12_mc_ccs
                )
            })
            .collect();

        tracing::debug!(
            "filtered {} CCS formats, {} remaining",
            renderer_formats.iter().count() - filtered_formats.len(),
            filtered_formats.len()
        );

        let format_codes: Vec<_> = filtered_formats.iter().map(|f| f.code).collect();
        let filtered_set: FormatSet = filtered_formats.iter().copied().collect();

        let gbm_surface = match GbmBufferedSurface::new(
            drm_surface,
            allocator.clone(),
            &format_codes,
            filtered_set.clone(),
        ) {
            Ok(surface) => surface,
            Err(e) => {
                tracing::warn!(
                    "GBM surface creation failed, retrying with implicit modifier: {e:?}"
                );
                let implicit_formats: Vec<_> = filtered_formats
                    .iter()
                    .copied()
                    .filter(|f| f.modifier == Modifier::Invalid)
                    .collect();
                let implicit_codes: Vec<_> = implicit_formats.iter().map(|f| f.code).collect();
                let implicit_set: FormatSet = implicit_formats.iter().copied().collect();

                let drm_surface2 = device
                    .drm
                    .create_surface(crtc, drm_mode, &[connector])
                    .map_err(|e| {
                        CompositorError::Backend(format!("failed to create DRM surface: {e:?}"))
                    })?;

                GbmBufferedSurface::new(drm_surface2, allocator, &implicit_codes, implicit_set)
                    .map_err(|e| {
                        CompositorError::Backend(format!(
                            "failed to create GBM surface with implicit modifier: {e:?}"
                        ))
                    })?
            }
        };

        let damage_tracker = OutputDamageTracker::from_output(&output);

        let surface = Surface {
            output: output.clone(),
            gbm_surface,
            damage_tracker,
        };

        device.surfaces.insert(crtc, surface);
        state.space.map_output(&output, (0, 0));

        tracing::info!(
            "output {} enabled: {}x{}@{}Hz",
            output.name(),
            drm_mode.size().0,
            drm_mode.size().1,
            drm_mode.vrefresh()
        );
    }

    Ok(())
}

fn find_crtc_for_connector(
    device: &Device,
    res_handles: &control::ResourceHandles,
    connector: connector::Handle,
) -> Result<crtc::Handle> {
    let connector_info = device
        .drm
        .get_connector(connector, false)
        .map_err(|e| CompositorError::Backend(format!("failed to get connector info: {e:?}")))?;

    let encoder = connector_info
        .current_encoder()
        .and_then(|e| device.drm.get_encoder(e).ok())
        .or_else(|| {
            connector_info
                .encoders()
                .iter()
                .filter_map(|e| device.drm.get_encoder(*e).ok())
                .next()
        })
        .ok_or_else(|| CompositorError::Backend("no encoder found".into()))?;

    let crtcs: Vec<_> = res_handles.crtcs().iter().copied().collect();
    let possible = encoder.possible_crtcs();

    for crtc in crtcs {
        if res_handles.filter_crtcs(possible).contains(&crtc)
            && !device.surfaces.contains_key(&crtc)
        {
            return Ok(crtc);
        }
    }

    Err(CompositorError::Backend("no available CRTC found".into()))
}

fn handle_drm_event(
    state: &mut ProjectWC,
    node: DrmNode,
    event: DrmEvent,
    _metadata: &mut Option<DrmEventMetadata>,
) {
    match event {
        DrmEvent::VBlank(crtc) => {
            if let Some(udev) = &mut state.udev {
                if let Some(device) = udev.devices.get_mut(&node) {
                    if let Some(surface) = device.surfaces.get_mut(&crtc) {
                        surface.gbm_surface.frame_submitted().ok();
                    }
                }
            }
            schedule_render(&state.loop_handle);
        }
        DrmEvent::Error(e) => {
            tracing::error!("DRM error: {e:?}");
        }
    }
}

fn schedule_initial_render(loop_handle: &LoopHandle<'static, ProjectWC>) {
    let timer = Timer::immediate();
    loop_handle
        .insert_source(timer, |_, _, state| {
            render(state);
            TimeoutAction::Drop
        })
        .ok();
}

fn schedule_render(loop_handle: &LoopHandle<'static, ProjectWC>) {
    let timer = Timer::immediate();
    loop_handle
        .insert_source(timer, |_, _, state| {
            render(state);
            TimeoutAction::Drop
        })
        .ok();
}

fn render(state: &mut ProjectWC) {
    let Some(udev) = &mut state.udev else {
        tracing::warn!("render: no udev data");
        return;
    };

    if udev.devices.is_empty() {
        tracing::warn!("render: no devices");
        return;
    }

    for device in udev.devices.values_mut() {
        if device.surfaces.is_empty() {
            tracing::warn!("render: no surfaces on device");
            continue;
        }

        for (_crtc, surface) in &mut device.surfaces {
            let output = surface.output.clone();

            let (mut dmabuf, _age) = match surface.gbm_surface.next_buffer() {
                Ok(result) => result,
                Err(e) => {
                    tracing::warn!("failed to get next buffer: {e:?}");
                    continue;
                }
            };

            let mut framebuffer = match device.gles.bind(&mut dmabuf) {
                Ok(fb) => fb,
                Err(e) => {
                    tracing::warn!("failed to bind dmabuf: {e:?}");
                    continue;
                }
            };

            let output_size = output
                .current_mode()
                .map(|m| m.size)
                .unwrap_or((1920, 1080).into());

            let damage = Rectangle::from_size(output_size);

            let cursor_size = 16;
            let cursor_buffer =
                SolidColorBuffer::new((cursor_size, cursor_size), [1.0, 1.0, 1.0, 1.0]);
            let cursor_pos: Point<i32, Physical> = (
                state.pointer_location.x as i32,
                state.pointer_location.y as i32,
            )
                .into();
            let cursor_element = SolidColorRenderElement::from_buffer(
                &cursor_buffer,
                cursor_pos,
                Scale::from(1.0),
                1.0,
                Kind::Cursor,
            );

            let custom_elements: Vec<OutputRenderElements<GlesRenderer>> =
                vec![OutputRenderElements::Cursor(cursor_element)];

            let render_result = smithay::desktop::space::render_output::<
                _,
                OutputRenderElements<GlesRenderer>,
                _,
                _,
            >(
                &output,
                &mut device.gles,
                &mut framebuffer,
                1.0,
                0,
                [&state.space],
                &custom_elements,
                &mut surface.damage_tracker,
                [0.1, 0.1, 0.1, 1.0],
            );

            match render_result {
                Ok(_) => {
                    match surface
                        .gbm_surface
                        .queue_buffer(None, Some(vec![damage]), ())
                    {
                        Ok(_) => {
                            tracing::debug!("frame queued for {}", output.name());
                        }
                        Err(e) => {
                            tracing::warn!("failed to queue buffer: {e:?}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("render failed: {e:?}");
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
        }
    }

    state.space.refresh();
    state.display_handle.flush_clients().ok();
}
