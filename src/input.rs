use crate::state::{MoveGrab, ProjectWC};
use smithay::reexports::wayland_server::Resource;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::layer_map_for_output,
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::SERIAL_COUNTER,
    wayland::{
        compositor,
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer, LayerSurfaceCachedState},
    },
};

impl ProjectWC {
    pub fn handle_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event } => self.handle_keyboard_event::<B>(event),
            InputEvent::PointerMotion { event } => self.handle_pointer_motion::<B>(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_motion_absolute::<B>(event)
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis::<B>(event),
            _ => {}
        }
    }

    fn handle_keyboard_event<B: InputBackend>(&mut self, event: B::KeyboardKeyEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time_msec = Event::time_msec(&event);
        let key_code = event.key_code();
        let key_state = event.state();

        let keyboard = self.seat.get_keyboard().expect("keyboard not initialized");

        for layer in self.layer_shell_state.layer_surfaces().rev() {
            let exclusive = compositor::with_states(layer.wl_surface(), |states| {
                let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
                let data = guard.current();

                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            });

            if exclusive {
                let surface = self.space.outputs().find_map(|output| {
                    let map = layer_map_for_output(output);
                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                });

                if let Some(surface) = surface {
                    keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
                    keyboard.input::<(), _>(
                        self,
                        key_code,
                        key_state,
                        serial,
                        time_msec,
                        |_, _, _| FilterResult::Forward,
                    );
                    return;
                }
            }
        }

        keyboard.input::<(), _>(
            self,
            key_code,
            key_state,
            serial,
            time_msec,
            |state, modifiers, keysym_handle| {
                if key_state == KeyState::Pressed {
                    let keysym = keysym_handle.modified_sym();
                    if handle_keybinding(state, modifiers, keysym) {
                        return FilterResult::Intercept(());
                    }
                }
                FilterResult::Forward
            },
        );
    }

    fn handle_pointer_motion<B: InputBackend>(&mut self, event: B::PointerMotionEvent) {
        tracing::debug!(
            "pointer motion: pos=({:.0}, {:.0})",
            self.pointer_location.x,
            self.pointer_location.y
        );
        let serial = SERIAL_COUNTER.next_serial();
        let delta = (event.delta_x(), event.delta_y()).into();

        self.pointer_location += delta;
        self.clamp_pointer_location();

        if let Some(grab) = &self.move_grab {
            let delta_x = self.pointer_location.x - grab.initial_pointer_location.x;
            let delta_y = self.pointer_location.y - grab.initial_pointer_location.y;
            let new_location = (
                grab.initial_window_location.x + delta_x as i32,
                grab.initial_window_location.y + delta_y as i32,
            );
            let window = grab.window.clone();
            self.space.map_element(window, new_location, true);
            return;
        }

        let pointer = self.pointer();
        let under = self.surface_under_pointer();

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: self.pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn handle_pointer_motion_absolute<B: InputBackend>(
        &mut self,
        event: B::PointerMotionAbsoluteEvent,
    ) {
        let output_geo = self
            .space
            .outputs()
            .next()
            .map(|output| self.space.output_geometry(output).unwrap());

        let Some(output_geo) = output_geo else { return };

        self.pointer_location = (
            event.x_transformed(output_geo.size.w),
            event.y_transformed(output_geo.size.h),
        )
            .into();

        if let Some(grab) = &self.move_grab {
            let delta_x = self.pointer_location.x - grab.initial_pointer_location.x;
            let delta_y = self.pointer_location.y - grab.initial_pointer_location.y;
            let new_location = (
                grab.initial_window_location.x + delta_x as i32,
                grab.initial_window_location.y + delta_y as i32,
            );
            let window = grab.window.clone();
            self.space.map_element(window, new_location, true);
            return;
        }

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.pointer();
        let under = self.surface_under_pointer();

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: self.pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn handle_pointer_button<B: InputBackend>(&mut self, event: B::PointerButtonEvent) {
        tracing::info!(
            "pointer button: code={} state={:?} pos=({:.0}, {:.0})",
            event.button_code(),
            event.state(),
            self.pointer_location.x,
            self.pointer_location.y
        );
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let button_state = event.state();
        let left_button = 0x110;

        let keyboard = self.seat.get_keyboard().expect("keyboard not initialized");
        let alt_held = keyboard.modifier_state().alt;

        for window in self.space.elements() {
            if let Some(geo) = self.space.element_geometry(window) {
                tracing::info!(
                    "window in space: {:?} at ({}, {}) size {}x{}",
                    window.toplevel().map(|t| t.wl_surface().id()),
                    geo.loc.x,
                    geo.loc.y,
                    geo.size.w,
                    geo.size.h
                );
            }
        }

        let element_under = self
            .space
            .element_under(self.pointer_location)
            .map(|(w, l)| (w.clone(), l));
        tracing::info!(
            "element_under: {:?}",
            element_under
                .as_ref()
                .map(|(w, l)| (w.toplevel().map(|t| t.wl_surface().id()), l))
        );

        if ButtonState::Pressed == button_state
            && button == left_button
            && alt_held
            && let Some((window, _)) = element_under.clone()
        {
            let window_location = self
                .space
                .element_geometry(&window)
                .map(|geo| geo.loc)
                .unwrap_or_default();
            self.move_grab = Some(MoveGrab {
                window: window.clone(),
                initial_window_location: window_location,
                initial_pointer_location: self.pointer_location,
            });
            self.space.raise_element(&window, true);
            return;
        }

        if ButtonState::Released == button_state
            && button == left_button
            && self.move_grab.is_some()
        {
            self.move_grab = None;
            return;
        }

        if ButtonState::Pressed == button_state {
            if let Some((window, _)) = element_under {
                self.space.raise_element(&window, true);

                keyboard.set_focus(
                    self,
                    Some(window.toplevel().expect("toplevel").wl_surface().clone()),
                    serial,
                );

                self.space.elements().for_each(|window| {
                    window
                        .toplevel()
                        .map(|toplevel| toplevel.send_pending_configure());
                });
            }
        }

        let pointer = self.pointer();
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn handle_pointer_axis<B: InputBackend>(&mut self, event: B::PointerAxisEvent) {
        let horizontal_amount = event
            .amount(Axis::Horizontal)
            .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 3.0 / 120.0);
        let vertical_amount = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 3.0 / 120.0);
        let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
        let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

        let mut axis_frame = AxisFrame::new(event.time_msec()).source(event.source());

        if horizontal_amount != 0.0 {
            axis_frame = axis_frame.value(Axis::Horizontal, horizontal_amount);
            if let Some(discrete) = horizontal_amount_discrete {
                axis_frame = axis_frame.v120(Axis::Horizontal, discrete as i32);
            }
        }

        if vertical_amount != 0.0 {
            axis_frame = axis_frame.value(Axis::Vertical, vertical_amount);
            if let Some(discrete) = vertical_amount_discrete {
                axis_frame = axis_frame.v120(Axis::Vertical, discrete as i32);
            }
        }

        if event.source() == AxisSource::Finger {
            if event.amount(Axis::Horizontal) == Some(0.0) {
                axis_frame = axis_frame.stop(Axis::Horizontal);
            }
            if event.amount(Axis::Vertical) == Some(0.0) {
                axis_frame = axis_frame.stop(Axis::Vertical);
            }
        }

        let pointer = self.pointer();
        pointer.axis(self, axis_frame);
        pointer.frame(self);
    }

    fn clamp_pointer_location(&mut self) {
        let output_geo = self
            .space
            .outputs()
            .next()
            .map(|output| self.space.output_geometry(output).unwrap());

        if let Some(output_geo) = output_geo {
            self.pointer_location.x = self
                .pointer_location
                .x
                .clamp(0.0, output_geo.size.w as f64 - 1.0);
            self.pointer_location.y = self
                .pointer_location
                .y
                .clamp(0.0, output_geo.size.h as f64 - 1.0);
        }
    }
}

fn handle_keybinding(state: &mut ProjectWC, modifiers: &ModifiersState, keysym: Keysym) -> bool {
    if !modifiers.alt {
        return false;
    }

    if modifiers.shift && (keysym == Keysym::q || keysym == Keysym::Q) {
        tracing::info!("quitting compositor");
        state.loop_signal.stop();
        return true;
    }

    match keysym {
        Keysym::Escape => {
            tracing::info!("quitting compositor");
            state.loop_signal.stop();
            true
        }
        Keysym::Return => {
            tracing::debug!("Spawning weston-terminal");
            std::process::Command::new("weston-terminal").spawn().ok();
            true
        }
        Keysym::q => {
            let keyboard = state.seat.get_keyboard().unwrap();
            if let Some(focused_surface) = keyboard.current_focus() {
                for window in state.space.elements() {
                    if window
                        .toplevel()
                        .map(|t| t.wl_surface() == &focused_surface)
                        .unwrap_or(false)
                    {
                        tracing::info!("Closing focused window");
                        window.toplevel().unwrap().send_close();
                        break;
                    }
                }
            }
            true
        }
        Keysym::d => {
            tracing::debug!("Spawning rofi menu");
            let _ = std::process::Command::new("rofi")
                .arg("-show")
                .arg("drun")
                .spawn();
            true
        }
        _ => false,
    }
}
