use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    desktop::{PopupKind, PopupManager, Space, Window},
    reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            self, CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        shell::xdg::XdgToplevelSurfaceData,
        shm::{ShmHandler, ShmState},
    },
};

use crate::{ProjectWC, grabs::resize_grab, handlers::layer_shell, state::ClientState};

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
        layer_shell::handle_commit(&mut self.space, surface);
    }
}

impl BufferHandler for ProjectWC {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for ProjectWC {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(ProjectWC);
delegate_compositor!(ProjectWC);

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
