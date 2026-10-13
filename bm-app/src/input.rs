//! winit (and gilrs) events -> `bm-ui` [`InputEvent`]s, and the per-frame [`PlayerInput`].

use bm_game::PlayerInput;
use bm_ui::input::{MouseButton as UiMouse, PadAxis, PadButton};
use bm_ui::{Action, InputEvent, InputMap, InputState};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::PhysicalKey;

/// Translates a window event. Returns None for events that carry no input.
pub fn translate(ev: &WindowEvent) -> Option<InputEvent> {
    match ev {
        WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
            let PhysicalKey::Code(code) = event.physical_key else { return None };
            Some(InputEvent::Key { code: format!("{code:?}"), pressed: event.state == ElementState::Pressed })
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let b = match button {
                MouseButton::Left => UiMouse::Left,
                MouseButton::Right => UiMouse::Right,
                MouseButton::Middle => UiMouse::Middle,
                MouseButton::Back => UiMouse::Back,
                MouseButton::Forward => UiMouse::Forward,
                MouseButton::Other(_) => return None,
            };
            Some(InputEvent::Mouse { button: b, pressed: *state == ElementState::Pressed })
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let y = match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32,
            };
            Some(InputEvent::Wheel(y))
        }
        WindowEvent::CursorMoved { position, .. } => Some(InputEvent::CursorMoved { x: position.x as f32, y: position.y as f32 }),
        _ => None,
    }
}

pub struct LookConfig {
    pub sensitivity: f32,
    pub invert_y: bool,
    /// Right stick turn rate in rad/s at full deflection.
    pub pad_rate: f32,
}

/// Builds the simulation input from the action state. `dt` scales the analog stick look.
pub fn player_input(state: &InputState, map: &InputMap, look: &LookConfig, dt: f32) -> PlayerInput {
    let mv = state.move_vector(map);
    let sign = if look.invert_y { 1.0 } else { -1.0 };
    let mut dx = state.mouse_delta[0] * look.sensitivity;
    let mut dy = state.mouse_delta[1] * look.sensitivity * sign;
    // right stick (bound as axes without actions: read through dedicated bindings)
    let rx = state.axis(PadAxis::RightX);
    let ry = state.axis(PadAxis::RightY);
    let dz = map.dead_zone;
    let shape = |v: f32| if v.abs() < dz { 0.0 } else { v.signum() * (v.abs() - dz) / (1.0 - dz) };
    dx += shape(rx) * look.pad_rate * dt;
    dy += shape(ry) * look.pad_rate * dt * sign;
    let cycle = [
        Action::CyclePistols,
        Action::CycleSmgs,
        Action::CycleAssaultRifles,
        Action::CycleSnipers,
        Action::CycleCCs,
        Action::CycleBinoculars,
        Action::CycleCoins,
    ]
    .iter()
    .any(|a| state.pressed(map, *a));
    PlayerInput {
        move_x: mv[0],
        move_y: mv[1],
        look_dx: dx,
        look_dy: dy,
        run: state.down(map, Action::Run),
        crouch: state.down(map, Action::CrouchSneak),
        aim: state.down(map, Action::FireWeapon2),
        fire: state.down(map, Action::FireWeapon),
        interact: state.down(map, Action::UseAction),
        reload: state.down(map, Action::ReloadWeapon),
        draw: state.down(map, Action::Holster),
        next_weapon: cycle || state.pressed(map, Action::CameraZoomIn),
    }
}

#[cfg(feature = "gamepad")]
pub struct Pads {
    gilrs: Option<gilrs::Gilrs>,
}

#[cfg(feature = "gamepad")]
impl Pads {
    pub fn new() -> Pads {
        match gilrs::Gilrs::new() {
            Ok(g) => {
                for (_, p) in g.gamepads() {
                    log::info!("gamepad: {}", p.name());
                }
                Pads { gilrs: Some(g) }
            }
            Err(e) => {
                log::debug!("no gamepad support: {e}");
                Pads { gilrs: None }
            }
        }
    }

    pub fn poll(&mut self, out: &mut Vec<InputEvent>) {
        use gilrs::{Axis, Button, EventType};
        let Some(g) = self.gilrs.as_mut() else { return };
        while let Some(ev) = g.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _) => {
                    let pressed = matches!(ev.event, EventType::ButtonPressed(..));
                    let pb = match b {
                        Button::South => PadButton::South,
                        Button::East => PadButton::East,
                        Button::West => PadButton::West,
                        Button::North => PadButton::North,
                        Button::LeftTrigger => PadButton::LeftShoulder,
                        Button::RightTrigger => PadButton::RightShoulder,
                        Button::Select => PadButton::Select,
                        Button::Start => PadButton::Start,
                        Button::LeftThumb => PadButton::LeftStick,
                        Button::RightThumb => PadButton::RightStick,
                        Button::DPadUp => PadButton::DPadUp,
                        Button::DPadDown => PadButton::DPadDown,
                        Button::DPadLeft => PadButton::DPadLeft,
                        Button::DPadRight => PadButton::DPadRight,
                        Button::LeftTrigger2 => {
                            out.push(InputEvent::PadAxis { axis: PadAxis::LeftTrigger, value: if pressed { 1.0 } else { 0.0 } });
                            continue;
                        }
                        Button::RightTrigger2 => {
                            out.push(InputEvent::PadAxis { axis: PadAxis::RightTrigger, value: if pressed { 1.0 } else { 0.0 } });
                            continue;
                        }
                        _ => continue,
                    };
                    out.push(InputEvent::Pad { button: pb, pressed });
                }
                EventType::AxisChanged(a, v, _) => {
                    // bm-ui axes are screen oriented: positive Y is down
                    let (axis, v) = match a {
                        Axis::LeftStickX => (PadAxis::LeftX, v),
                        Axis::LeftStickY => (PadAxis::LeftY, -v),
                        Axis::RightStickX => (PadAxis::RightX, v),
                        Axis::RightStickY => (PadAxis::RightY, -v),
                        _ => continue,
                    };
                    out.push(InputEvent::PadAxis { axis, value: v });
                }
                _ => {}
            }
        }
    }
}

#[cfg(not(feature = "gamepad"))]
pub struct Pads;

#[cfg(not(feature = "gamepad"))]
impl Pads {
    pub fn new() -> Pads {
        Pads
    }
    pub fn poll(&mut self, _out: &mut Vec<InputEvent>) {}
}
