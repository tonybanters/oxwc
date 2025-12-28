use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::grabs::move_grab::MoveSurfaceGrab;
use crate::grabs::resize_grab::{self, ResizeSurfaceGrab};
use crate::state::{ClientState, ProjectWC};
use smithay::delegate_layer_shell;
use smithay::delegate_primary_selection;
use smithay::desktop::{LayerSurface, PopupManager, Space, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};
use smithay::utils::{Physical, Rectangle, Size};
use smithay::wayland::compositor;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{
        PopupKind, Window, WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorImageStatus, Focus, GrabStartData as PointerGrabStartData},
    },
    reexports::wayland_server::{
        Resource,
        protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

impl CompositorHandler for ProjectWC {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root_surface = surface.clone();
            while let Some(parent) = get_parent(&root_surface) {
                root_surface = parent;
            }

            if let Some(window) = self
                .space
                .elements()
                .find(|window| {
                    window
                        .toplevel()
                        .map(|toplevel| toplevel.wl_surface() == &root_surface)
                        .unwrap_or(false)
                })
                .cloned()
            {
                window.on_commit();
            }
        }

        handle_commit(&mut self.popups, &self.space, surface);
        resize_grab::handle_commit(&mut self.space, surface);

        // TODO: Split into handlers::layer_shell:handle_commit or smth like that
        for output in self.space.outputs() {
            let mut layer_map = layer_map_for_output(output);
            if let Some(layer) = layer_map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
            {
                layer_map.arrange();

                let initial_configure_sent = compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<LayerSurfaceData>()
                        .map(|data| data.lock().unwrap().initial_configure_sent)
                        .unwrap_or(true)
                });
                if !initial_configure_sent {
                    layer.layer_surface().send_configure();
                }
                break;
            }
        }
    }
}
delegate_compositor!(ProjectWC);

impl BufferHandler for ProjectWC {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for ProjectWC {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(ProjectWC);

impl XdgShellHandler for ProjectWC {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface.clone());
        self.space.map_element(window, (0, 0), false);
        self.apply_layout().ok();
        surface.send_configure();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!("error while tracking popup: {err:?}");
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    // TODO: Test this when you implement resize request
    // as it should be able to trigger this as well.
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == wl_surface)
                .unwrap()
                .clone();

            let initial_window_location = self.space.element_location(&window).unwrap();

            let grab = MoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            };

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat = Seat::from_resource(&seat).unwrap();
        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == wl_surface)
                .unwrap()
                .clone();

            let initial_window_location = self.space.element_location(&window).unwrap();
            let initial_window_size = window.geometry().size;

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });

            surface.send_pending_configure();

            let grab = ResizeSurfaceGrab::start(
                start_data,
                window,
                edges.into(),
                Rectangle::new(initial_window_location, initial_window_size),
            );

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let window = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel == &surface)
                    .unwrap_or(false)
            })
            .cloned();

        if let Some(window) = window {
            self.space.unmap_elem(&window);
        }
    }
}
delegate_xdg_shell!(ProjectWC);

impl SeatHandler for ProjectWC {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}
delegate_seat!(ProjectWC);

impl SelectionHandler for ProjectWC {
    type SelectionUserData = ();
}

impl DataDeviceHandler for ProjectWC {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
delegate_data_device!(ProjectWC);

impl ClientDndGrabHandler for ProjectWC {}
impl ServerDndGrabHandler for ProjectWC {}

impl OutputHandler for ProjectWC {}
delegate_output!(ProjectWC);

impl PrimarySelectionHandler for ProjectWC {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(ProjectWC);

impl WlrLayerShellHandler for ProjectWC {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());

        let Some(output) = output else {
            tracing::warn!(namespace, "no output for new layer surface");
            return;
        };

        let layer_surface = LayerSurface::new(surface, namespace);
        let mut layer_map = layer_map_for_output(&output);
        if let Err(err) = layer_map.map_layer(&layer_surface) {
            tracing::warn!("failed to map layer surface: {err:?}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        if let Some((mut map, layer)) = self.space.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            let layer = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();

            layer.map(|layer| (map, layer))
        }) {
            map.unmap_layer(&layer);
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }
}
delegate_layer_shell!(ProjectWC);

impl ProjectWC {
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };

        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &root)
        else {
            return;
        };

        let output = self.space.outputs().next().unwrap();
        let output_geo = self.space.output_geometry(output).unwrap();
        let window_geo = self.space.element_geometry(window).unwrap();

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

fn check_grab(
    seat: &Seat<ProjectWC>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<ProjectWC>> {
    let pointer = seat.get_pointer()?;

    // Check that this surface has a click grab.
    if !pointer.has_grab(serial) {
        return None;
    }

    let start_data = pointer.grab_start_data()?;

    let (focus, _) = start_data.focus.as_ref()?;
    // If the focus was for a different surface, ignore the request.
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }

    Some(start_data)
}

/// Should be called on `WlSurface::commit`
fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    // Handle toplevel commits.
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()
    {
        let initial_configure_sent = compositor::with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    // Handle popup commits.
    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match &popup {
            PopupKind::Xdg(xdg) => {
                if !xdg.is_initial_configure_sent() {
                    xdg.send_configure().expect("");
                }
            }
            PopupKind::InputMethod(_) => {}
        }
    }
}

const SCREENCOPY_VERSION: u32 = 3;

#[derive(Default)]
pub struct ScreencopyManagerState;

pub struct ScreencopyManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl ScreencopyManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
        D: Dispatch<ZwlrScreencopyManagerV1, ()>,
        D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
        D: ScreencopyHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = ScreencopyManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ZwlrScreencopyManagerV1, _>(SCREENCOPY_VERSION, global_data);

        Self
    }
}

impl<D> GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData, D>
    for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
    D: Dispatch<ZwlrScreencopyManagerV1, ()>,
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrScreencopyManagerV1>,
        _manager_state: &ScreencopyManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }

    fn can_view(client: Client, global_data: &ScreencopyManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwlrScreencopyManagerV1, (), D> for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
    D: Dispatch<ZwlrScreencopyManagerV1, ()>,
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (frame, output) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, output, .. } => {
                let Some(output) = Output::from_resource(&output) else {
                    tracing::trace!("screencopy: client requested non-existent output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };
                (frame, output)
            }
            // TODO: implement region capture (currently captures full output)
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion { frame, output, .. } => {
                let Some(output) = Output::from_resource(&output) else {
                    tracing::trace!("screencopy: client requested non-existent output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };
                (frame, output)
            }
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        let Some(mode) = output.current_mode() else {
            tracing::trace!("screencopy: output has no current mode");
            let frame = data_init.init(frame, ScreencopyFrameState::Failed);
            frame.failed();
            return;
        };

        let buffer_size = mode.size;
        let info = ScreencopyFrameInfo {
            output,
            buffer_size,
        };
        let frame = data_init.init(
            frame,
            ScreencopyFrameState::Pending {
                info,
                copied: Arc::new(AtomicBool::new(false)),
            },
        );

        frame.buffer(
            Format::Xrgb8888,
            buffer_size.w as u32,
            buffer_size.h as u32,
            buffer_size.w as u32 * 4,
        );

        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

#[derive(Clone)]
pub struct ScreencopyFrameInfo {
    output: Output,
    buffer_size: Size<i32, Physical>,
}

pub enum ScreencopyFrameState {
    Failed,
    Pending {
        info: ScreencopyFrameInfo,
        copied: Arc<AtomicBool>,
    },
}

impl<D> Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState, D> for ScreencopyManagerState
where
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameState,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        if matches!(request, zwlr_screencopy_frame_v1::Request::Destroy) {
            return;
        }

        let ScreencopyFrameState::Pending { info, copied } = data else {
            return;
        };

        if copied.load(Ordering::SeqCst) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "copy was already requested",
            );
            return;
        }

        let buffer = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => buffer,
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => buffer,
            _ => unreachable!(),
        };

        let size = info.buffer_size;

        let valid = shm::with_buffer_contents(&buffer, |_, shm_len, buffer_data| {
            buffer_data.format == Format::Xrgb8888
                && buffer_data.width == size.w
                && buffer_data.height == size.h
                && buffer_data.stride == size.w * 4
                && shm_len == buffer_data.stride as usize * buffer_data.height as usize
        })
        .unwrap_or(false);

        if !valid {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "invalid buffer",
            );
            return;
        }

        copied.store(true, Ordering::SeqCst);

        state.frame(Screencopy {
            buffer,
            frame: frame.clone(),
            info: info.clone(),
            submitted: false,
        });
    }
}

pub struct Screencopy {
    pub buffer: WlBuffer,
    frame: ZwlrScreencopyFrameV1,
    info: ScreencopyFrameInfo,
    submitted: bool,
}

impl Drop for Screencopy {
    fn drop(&mut self) {
        if !self.submitted {
            self.frame.failed();
        }
    }
}

impl Screencopy {
    pub fn output(&self) -> &Output {
        &self.info.output
    }

    pub fn buffer_size(&self) -> Size<i32, Physical> {
        self.info.buffer_size
    }

    pub fn submit(mut self, timestamp: Duration) {
        self.frame.flags(Flags::empty());

        let tv_sec_hi = (timestamp.as_secs() >> 32) as u32;
        let tv_sec_lo = (timestamp.as_secs() & 0xFFFFFFFF) as u32;
        let tv_nsec = timestamp.subsec_nanos();
        self.frame.ready(tv_sec_hi, tv_sec_lo, tv_nsec);

        self.submitted = true;
    }
}

pub trait ScreencopyHandler {
    fn screencopy_state(&mut self) -> &mut ScreencopyManagerState;
    fn frame(&mut self, screencopy: Screencopy);
}

impl ScreencopyHandler for ProjectWC {
    fn screencopy_state(&mut self) -> &mut ScreencopyManagerState {
        &mut self.screencopy_state
    }

    fn frame(&mut self, screencopy: Screencopy) {
        self.pending_screencopy = Some(screencopy);
    }
}

#[macro_export]
macro_rules! delegate_screencopy {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: $crate::shell::ScreencopyManagerGlobalData
        ] => $crate::shell::ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: ()
        ] => $crate::shell::ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1: $crate::shell::ScreencopyFrameState
        ] => $crate::shell::ScreencopyManagerState);
    };
}

delegate_screencopy!(ProjectWC);
