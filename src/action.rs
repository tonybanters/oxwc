use crate::ProjectWC;
use smithay::{
    desktop::Window, reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::SERIAL_COUNTER,
};

pub enum Action {
    FocusNext,
    FocusPrevious,
}

enum Direction {
    Next,
    Previous,
}

impl Action {
    pub fn execute(self, project_wc: &mut ProjectWC) {
        let direction = match self {
            Action::FocusNext => Direction::Next,
            Action::FocusPrevious => Direction::Previous,
        };
        change_focus(direction, project_wc);
    }
}

fn change_focus(direction: Direction, project_wc: &mut ProjectWC) {
    let keyboard = project_wc.seat.get_keyboard().unwrap();
    let serial = SERIAL_COUNTER.next_serial();

    let windows: Vec<Window> = project_wc.space.elements().cloned().collect();
    if windows.is_empty() {
        return;
    }

    let current_focus = keyboard.current_focus();
    let current_idx = current_focus.and_then(|surf| {
        project_wc
            .window_for_surface(&surf)
            .and_then(|w| windows.iter().position(|win| win == &w))
    });

    let target_idx = match (direction, current_idx) {
        (Direction::Next, Some(i)) if i + 1 < windows.len() => i + 1,
        (Direction::Next, Some(_)) => return,
        (Direction::Next, None) => 0,
        (Direction::Previous, Some(i)) if i > 0 => i - 1,
        (Direction::Previous, Some(_)) => return,
        (Direction::Previous, None) => windows.len() - 1,
    };

    if let Some(prev_idx) = current_idx {
        if let Some(prev_toplevel) = windows[prev_idx].toplevel() {
            prev_toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Activated);
            });
            prev_toplevel.send_pending_configure();
        }
    }

    let target = &windows[target_idx];

    if let Some(toplevel) = target.toplevel() {
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        toplevel.send_pending_configure();
        keyboard.set_focus(project_wc, Some(toplevel.wl_surface().clone()), serial);
    }
}
