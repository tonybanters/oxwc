use smithay::{
    desktop::{Space, Window},
    input::{Seat, SeatState, pointer::PointerHandle},
    reexports::{
        calloop::LoopHandle,
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};
use std::{ffi::OsString, sync::Arc};

use crate::{
    CompositorError,
    layout::{GapConfig, LayoutBox, LayoutType},
};

pub struct Oxwc {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Oxwc>,
    pub running: bool,

    pub space: Space<Window>,
    pub seat: Seat<Self>,
    pub seat_state: SeatState<Self>,
    pub layout: LayoutBox,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub data_device_state: DataDeviceState,

    pub pointer_location: Point<f64, Logical>,
    pub move_grab: Option<MoveGrab>,
}

pub struct MoveGrab {
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
    pub initial_pointer_location: Point<f64, Logical>,
}

impl Oxwc {
    pub fn new(
        display: Display<Self>,
        loop_handle: LoopHandle<'static, Oxwc>,
    ) -> (Self, Display<Self>) {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        // TODO: Get a brain
        let layout = LayoutType::from_str("tiling").unwrap().new();

        let mut seat_state = SeatState::new();
        let seat_name = "seat0".to_string();
        let seat = seat_state.new_wl_seat(&display_handle, seat_name);
        let space = Space::default();

        (
            Self {
                display_handle,
                loop_handle,
                running: true,

                space,
                layout,
                seat,
                seat_state,

                compositor_state,
                xdg_shell_state,
                shm_state,
                output_manager_state,
                data_device_state,

                pointer_location: Point::from((0.0, 0.0)),
                move_grab: None,
            },
            display,
        )
    }

    pub fn apply_layout(&mut self) -> Result<(), CompositorError> {
        let windows: Vec<smithay::desktop::Window> = self.space.elements().cloned().collect();
        if windows.is_empty() {
            return Ok(());
        }

        let output = self
            .space
            .outputs()
            .next()
            .cloned()
            .ok_or_else(|| CompositorError::Backend("no output".into()))?;

        let out_geo = self
            .space
            .output_geometry(&output)
            .ok_or_else(|| CompositorError::Backend("no output geometry".into()))?;

        let gaps = GapConfig {
            outer_horizontal: 20,
            outer_vertical: 20,
            inner_horizontal: 10,
            inner_vertical: 10,
        };

        let master_factor: f32 = 0.55;
        let num_master: i32 = 1;
        let smartgaps_enabled: bool = true;

        let geometries = self.layout.arrange(
            &windows,
            out_geo.size.w as u32,
            out_geo.size.h as u32,
            &gaps,
            master_factor,
            num_master,
            smartgaps_enabled,
        );

        for (window, geom) in windows.into_iter().zip(geometries.into_iter()) {
            let loc = Point::<i32, Logical>::from((
                out_geo.loc.x + geom.x_coordinate,
                out_geo.loc.y + geom.y_coordinate,
            ));
            self.space.map_element(window, loc, false);
        }

        Ok(())
    }

    pub fn surface_under_pointer(&self) -> Option<(Window, Point<i32, Logical>)> {
        let position = self.pointer_location;
        self.space
            .element_under(position)
            .map(|(window, location)| (window.clone(), location))
    }

    pub fn pointer(&self) -> PointerHandle<Self> {
        self.seat.get_pointer().expect("pointer not initialized")
    }
}

pub fn init_wayland_listener(loop_handle: &LoopHandle<'static, Oxwc>) -> OsString {
    let listening_socket = ListeningSocketSource::new_auto().expect("failed to create socket");
    let socket_name = listening_socket.socket_name().to_os_string();

    loop_handle
        .insert_source(listening_socket, move |client_stream, _, state| {
            state
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
                .expect("failed to insert client");
        })
        .expect("failed to init wayland listener");

    socket_name
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
