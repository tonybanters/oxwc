use crate::{grabs::move_grab::MoveGrab, state::ProjectWC};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, MouseButton, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    desktop::{WindowSurfaceType, layer_map_for_output},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GrabStartData as PointerGrabStartData, MotionEvent,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::{
        compositor,
        input_method::InputMethodSeat,
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
        let serial = SERIAL_COUNTER.next_serial();
        let delta = (event.delta_x(), event.delta_y()).into();

        self.pointer_location += delta;
        self.clamp_pointer_location();

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
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button();
        let button_code = event.button_code();
        let button_state = event.state();
        let pointer = self.pointer();

        let keyboard = self.seat.get_keyboard().expect("keyboard not initialized");
        let alt_held = keyboard.modifier_state().alt;

        if ButtonState::Pressed == button_state
            && button == Some(MouseButton::Left)
            && alt_held
            && let Some((window, _)) = self.window_under_pointer()
            && !pointer.is_grabbed()
        {
            let location = self.pointer_location;

            let start_data = PointerGrabStartData {
                focus: None,
                button: button_code,
                location,
            };
            let initial_window_location = self.space.element_location(&window).unwrap();
            let grab = MoveGrab {
                start_data,
                window: window.clone(),
                initial_window_location,
            };
            pointer.set_grab(self, grab, serial, Focus::Clear);
            self.space.raise_element(&window, true);
        }

        if ButtonState::Pressed == button_state {
            self.update_keyboard_focus(self.pointer_location, serial);
            self.space.elements().for_each(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.send_pending_configure());
            });
        }

        pointer.button(
            self,
            &ButtonEvent {
                button: button_code,
                state: button_state,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn update_keyboard_focus(&mut self, location: Point<f64, Logical>, serial: Serial) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let input_method = self.seat.input_method();

        if !self.pointer().is_grabbed()
            && (!keyboard.is_grabbed() || input_method.keyboard_grabbed())
        {
            tracing::debug!("Pointer and keyboard are not grabbed");
            // There's only one output as of now
            let output = self.space.outputs().next().cloned().unwrap();
            let output_geo = self.space.output_geometry(&output).unwrap();

            let layers = layer_map_for_output(&output);

            #[allow(clippy::collapsible_if)]
            if let Some(layer) = layers
                .layer_under(WlrLayer::Overlay, location - output_geo.loc.to_f64())
                .or_else(|| layers.layer_under(WlrLayer::Top, location - output_geo.loc.to_f64()))
            {
                if layer.can_receive_keyboard_focus() {
                    tracing::debug!(
                        namespace = layer.namespace(),
                        "Layer can receive keyboard focus"
                    );

                    if let Some((_, _)) = layer.surface_under(
                        location
                            - output_geo.loc.to_f64()
                            - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::ALL,
                    ) {
                        let namespace = layer.namespace();
                        tracing::debug!(namespace, "Set keyboard focus for layer");
                        keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
                        return;
                    }
                }
            }

            if let Some((window, _)) = self
                .space
                .element_under(location)
                .map(|(w, p)| (w.clone(), p))
            {
                tracing::debug!("Setting focus of surface under pointer");
                self.space.raise_element(&window, true);
                keyboard.set_focus(
                    self,
                    Some(window.toplevel().unwrap().wl_surface().clone()),
                    serial,
                );
                return;
            }

            #[allow(clippy::collapsible_if)]
            if let Some(layer) = layers
                .layer_under(WlrLayer::Bottom, location - output_geo.loc.to_f64())
                .or_else(|| {
                    layers.layer_under(WlrLayer::Background, location - output_geo.loc.to_f64())
                })
            {
                if layer.can_receive_keyboard_focus() {
                    if let Some((_, _)) = layer.surface_under(
                        location
                            - output_geo.loc.to_f64()
                            - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::ALL,
                    ) {
                        keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
                    }
                }
            }
        }
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

    match keysym {
        Keysym::Escape => {
            tracing::debug!("Quitting");
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
            if let Some(focused_surface) = keyboard.current_focus()
                && let Some(window) = state.window_for_surface(&focused_surface)
            {
                tracing::info!("Closing focused window");
                window.toplevel().unwrap().send_close();
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
