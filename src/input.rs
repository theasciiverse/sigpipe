//! Controls: keyboard (arrows, WASD or hjkl to move, Space or Enter to lay, F for
//! fast flow, P to pause, Esc to go back), the mouse (the cursor follows it, a
//! click lays) and every gamepad (d-pad or stick to move with repeat, A to lay, X,
//! Y or a shoulder for fast flow, Start to pause, B to go back).
//!
//! Every raw event is logged at debug level before it turns into an action.
//!
//! gilrs counts any device with buttons and two axes as a gamepad, so only devices
//! that matched SDL's controller database are used; `SIGPIPE_ANY_PAD=1` takes all.

use crate::engine::Dir;
use log::debug;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
#[cfg(feature = "gamepad")]
use log::info;
#[cfg(feature = "gamepad")]
use std::time::Duration;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Move(Dir),
    /// Lay a piece, or confirm in a menu.
    Ok,
    Fast,
    Pause,
    Back,
    Quit,
    /// The mouse over a screen cell, and whether it clicked there.
    Point { x: u16, y: u16, click: bool },
}

/// A held pad direction repeats after this long, then every `REPEAT_EVERY`.
#[cfg(feature = "gamepad")]
const REPEAT_AFTER: Duration = Duration::from_millis(260);
#[cfg(feature = "gamepad")]
const REPEAT_EVERY: Duration = Duration::from_millis(85);
#[cfg(feature = "gamepad")]
const STICK_DEADZONE: f32 = 0.5;

pub struct Input {
    pub acts: Vec<Act>,
    #[cfg(feature = "gamepad")]
    gilrs: Option<gilrs::Gilrs>,
    /// The pad direction held (d-pad or stick) and when it next repeats.
    #[cfg(feature = "gamepad")]
    held: Option<(Dir, Instant)>,
    #[cfg(feature = "gamepad")]
    dpad: Option<Dir>,
    #[cfg(feature = "gamepad")]
    stick: Option<Dir>,
}

#[cfg(feature = "gamepad")]
fn usable_pad(p: &gilrs::Gamepad) -> bool {
    p.mapping_source() == gilrs::MappingSource::SdlMappings || std::env::var("SIGPIPE_ANY_PAD").is_ok_and(|v| v == "1")
}

impl Input {
    pub fn new() -> Input {
        Input {
            acts: Vec::new(),
            #[cfg(feature = "gamepad")]
            gilrs: match gilrs::Gilrs::new() {
                Ok(g) => {
                    let all: Vec<_> = g.gamepads().map(|(_, p)| (p.name().to_string(), usable_pad(&p))).collect();
                    info!("gamepads at start (name, used): {all:?}");
                    Some(g)
                }
                Err(e) => {
                    log::warn!("gamepad support unavailable: {e}");
                    None
                }
            },
            #[cfg(feature = "gamepad")]
            held: None,
            #[cfg(feature = "gamepad")]
            dpad: None,
            #[cfg(feature = "gamepad")]
            stick: None,
        }
    }

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) => {
                debug!("key {:?} {:?} {:?}", k.code, k.kind, k.modifiers);
                if k.kind == KeyEventKind::Release {
                    return;
                }
                if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c' | 'd')) {
                    self.acts.push(Act::Quit);
                    return;
                }
                let act = match k.code {
                    KeyCode::Up | KeyCode::Char('w' | 'W' | 'k' | 'K') => Act::Move(Dir::Up),
                    KeyCode::Down | KeyCode::Char('s' | 'S' | 'j' | 'J') => Act::Move(Dir::Down),
                    KeyCode::Left | KeyCode::Char('a' | 'A' | 'h' | 'H') => Act::Move(Dir::Left),
                    KeyCode::Right | KeyCode::Char('d' | 'D' | 'l' | 'L') => Act::Move(Dir::Right),
                    KeyCode::Enter | KeyCode::Char(' ') => Act::Ok,
                    KeyCode::Char('f' | 'F') => Act::Fast,
                    KeyCode::Char('p' | 'P') => Act::Pause,
                    KeyCode::Esc | KeyCode::Backspace => Act::Back,
                    KeyCode::Char('q' | 'Q') => Act::Quit,
                    _ => return,
                };
                self.acts.push(act);
            }
            Event::Mouse(m) => {
                let click = match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => true,
                    MouseEventKind::Moved | MouseEventKind::Drag(_) => false,
                    _ => return,
                };
                if click {
                    debug!("mouse click {},{}", m.column, m.row);
                }
                self.acts.push(Act::Point { x: m.column, y: m.row, click });
            }
            _ => {}
        }
    }

    /// Drain gamepad events and repeat a held direction (call once per frame).
    pub fn poll_pads(&mut self, now: Instant) {
        #[cfg(feature = "gamepad")]
        {
            use gilrs::{Axis, Button, EventType};
            let Some(g) = &mut self.gilrs else { return };
            while let Some(ev) = g.next_event() {
                let pad = g.gamepad(ev.id);
                if !usable_pad(&pad) {
                    continue;
                }
                match ev.event {
                    EventType::Connected => info!("pad connected: {}", pad.name()),
                    EventType::Disconnected => {
                        info!("pad disconnected: {}", pad.name());
                        (self.dpad, self.stick, self.held) = (None, None, None);
                    }
                    EventType::ButtonPressed(b, _) => {
                        debug!("pad press {b:?}");
                        let act = match b {
                            Button::DPadUp => {
                                self.dpad = Some(Dir::Up);
                                None
                            }
                            Button::DPadDown => {
                                self.dpad = Some(Dir::Down);
                                None
                            }
                            Button::DPadLeft => {
                                self.dpad = Some(Dir::Left);
                                None
                            }
                            Button::DPadRight => {
                                self.dpad = Some(Dir::Right);
                                None
                            }
                            Button::South => Some(Act::Ok),
                            Button::West | Button::North | Button::RightTrigger | Button::RightTrigger2 => Some(Act::Fast),
                            Button::East | Button::Select => Some(Act::Back),
                            Button::Start => Some(Act::Pause),
                            _ => None,
                        };
                        self.acts.extend(act);
                    }
                    EventType::ButtonReleased(b, _) => {
                        debug!("pad release {b:?}");
                        if matches!(b, Button::DPadUp | Button::DPadDown | Button::DPadLeft | Button::DPadRight) {
                            self.dpad = None;
                        }
                    }
                    EventType::AxisChanged(Axis::LeftStickX | Axis::LeftStickY, ..) => {
                        let axis = |a: Axis| pad.axis_data(a).map_or(0.0, |d| d.value());
                        // stick up is positive; screen up is negative
                        let (x, y) = (axis(Axis::LeftStickX), -axis(Axis::LeftStickY));
                        let dir = if x.hypot(y) < STICK_DEADZONE {
                            None
                        } else if x.abs() >= y.abs() {
                            Some(if x < 0.0 { Dir::Left } else { Dir::Right })
                        } else {
                            Some(if y < 0.0 { Dir::Up } else { Dir::Down })
                        };
                        if dir != self.stick {
                            debug!("pad stick {x:.2},{y:.2} -> {dir:?}");
                            self.stick = dir;
                        }
                    }
                    _ => {}
                }
            }
            match (self.dpad.or(self.stick), self.held) {
                (None, _) => self.held = None,
                (Some(d), Some((h, next))) if d == h => {
                    if now >= next {
                        self.acts.push(Act::Move(d));
                        self.held = Some((d, now + REPEAT_EVERY));
                    }
                }
                (Some(d), _) => {
                    self.acts.push(Act::Move(d));
                    self.held = Some((d, now + REPEAT_AFTER));
                }
            }
        }
        #[cfg(not(feature = "gamepad"))]
        let _ = now;
    }
}
