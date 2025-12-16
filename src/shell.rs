use crate::state::{ClientState, Oxwc};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::Window,
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        output::OutputHandler,
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

impl CompositorHandler for Oxwc {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a smithay::reexports::wayland_server::Client) -> &'a CompositorClientState {
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
    }
}

impl BufferHandler for Oxwc {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Oxwc {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Oxwc {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface.clone());
        self.space.map_element(window, (0, 0), false);
        surface.send_configure();
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}

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

impl SeatHandler for Oxwc {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl SelectionHandler for Oxwc {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Oxwc {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Oxwc {}
impl ServerDndGrabHandler for Oxwc {}

impl OutputHandler for Oxwc {}

delegate_compositor!(Oxwc);
delegate_shm!(Oxwc);
delegate_xdg_shell!(Oxwc);
delegate_seat!(Oxwc);
delegate_data_device!(Oxwc);
delegate_output!(Oxwc);
