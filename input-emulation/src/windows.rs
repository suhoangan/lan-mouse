use super::error::{EmulationError, WindowsEmulationCreationError};
use input_event::{
    BTN_BACK, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Event, KeyboardEvent, PointerEvent,
    scancode,
};

use async_trait::async_trait;
use std::ops::BitOrAssign;
use std::time::Duration;
use tokio::task::AbortHandle;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT_0, KEYEVENTF_EXTENDEDKEY, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

use super::{Emulation, EmulationHandle};

const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(500);
const DEFAULT_REPEAT_INTERVAL: Duration = Duration::from_millis(32);

pub(crate) struct WindowsEmulation {
    repeat_task: Option<AbortHandle>,
    fractional_dx: f64,
    fractional_dy: f64,
}

impl WindowsEmulation {
    pub(crate) fn new() -> Result<Self, WindowsEmulationCreationError> {
        Ok(Self {
            repeat_task: None,
            fractional_dx: 0.0,
            fractional_dy: 0.0,
        })
    }
}

#[async_trait]
impl Emulation for WindowsEmulation {
    async fn consume(&mut self, event: Event, _: EmulationHandle) -> Result<(), EmulationError> {
        match event {
            Event::Pointer(pointer_event) => match pointer_event {
                PointerEvent::Motion { time: _, dx, dy } => {
                    let (int_dx, int_dy) = self.accumulate_motion(dx, dy);
                    if int_dx != 0 || int_dy != 0 {
                        rel_mouse(int_dx, int_dy);
                    }
                }
                PointerEvent::Button {
                    time: _,
                    button,
                    state,
                } => mouse_button(button, state),
                PointerEvent::Axis {
                    time: _,
                    axis,
                    value,
                } => scroll(axis, value as i32),
                PointerEvent::AxisDiscrete120 { axis, value } => scroll(axis, value),
            },
            Event::Keyboard(keyboard_event) => match keyboard_event {
                KeyboardEvent::Key {
                    time: _,
                    key,
                    state,
                } => {
                    match state {
                        // pressed
                        0 => self.kill_repeat_task(),
                        1 => self.spawn_repeat_task(key).await,
                        _ => {}
                    }
                    key_event(key, state)
                }
                KeyboardEvent::Modifiers { .. } => {}
            },
        }
        // FIXME
        Ok(())
    }

    async fn create(&mut self, _handle: EmulationHandle) {}

    async fn destroy(&mut self, _handle: EmulationHandle) {
        self.fractional_dx = 0.0;
        self.fractional_dy = 0.0;
    }

    async fn terminate(&mut self) {
        self.kill_repeat_task();
        self.fractional_dx = 0.0;
        self.fractional_dy = 0.0;
    }
}

impl WindowsEmulation {
    pub(crate) fn accumulate_motion(&mut self, dx: f64, dy: f64) -> (i32, i32) {
        let total_dx = dx + self.fractional_dx;
        let total_dy = dy + self.fractional_dy;
        let int_dx = total_dx.trunc() as i32;
        let int_dy = total_dy.trunc() as i32;
        self.fractional_dx = total_dx - (int_dx as f64);
        self.fractional_dy = total_dy - (int_dy as f64);
        (int_dx, int_dy)
    }

    async fn spawn_repeat_task(&mut self, key: u32) {
        // there can only be one repeating key and it's
        // always the last to be pressed
        self.kill_repeat_task();
        let repeat_task = tokio::task::spawn_local(async move {
            tokio::time::sleep(DEFAULT_REPEAT_DELAY).await;
            loop {
                key_event(key, 1);
                tokio::time::sleep(DEFAULT_REPEAT_INTERVAL).await;
            }
        });
        self.repeat_task = Some(repeat_task.abort_handle());
    }
    fn kill_repeat_task(&mut self) {
        if let Some(task) = self.repeat_task.take() {
            task.abort();
        }
    }
}

fn send_input_safe(input: INPUT) {
    unsafe {
        loop {
            /* retval = number of successfully submitted events */
            if SendInput(&[input], std::mem::size_of::<INPUT>() as i32) > 0 {
                break;
            }
        }
    }
}

fn send_mouse_input(mi: MOUSEINPUT) {
    send_input_safe(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    });
}

fn send_keyboard_input(ki: KEYBDINPUT) {
    send_input_safe(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    });
}
fn rel_mouse(dx: i32, dy: i32) {
    let mi = MOUSEINPUT {
        dx,
        dy,
        mouseData: 0,
        dwFlags: MOUSEEVENTF_MOVE,
        time: 0,
        dwExtraInfo: 0,
    };
    send_mouse_input(mi);
}

fn mouse_button(button: u32, state: u32) {
    let dw_flags = match state {
        0 => match button {
            BTN_LEFT => MOUSEEVENTF_LEFTUP,
            BTN_RIGHT => MOUSEEVENTF_RIGHTUP,
            BTN_MIDDLE => MOUSEEVENTF_MIDDLEUP,
            BTN_BACK => MOUSEEVENTF_XUP,
            BTN_FORWARD => MOUSEEVENTF_XUP,
            _ => return,
        },
        1 => match button {
            BTN_LEFT => MOUSEEVENTF_LEFTDOWN,
            BTN_RIGHT => MOUSEEVENTF_RIGHTDOWN,
            BTN_MIDDLE => MOUSEEVENTF_MIDDLEDOWN,
            BTN_BACK => MOUSEEVENTF_XDOWN,
            BTN_FORWARD => MOUSEEVENTF_XDOWN,
            _ => return,
        },
        _ => return,
    };
    let mouse_data = match button {
        BTN_BACK => XBUTTON1 as u32,
        BTN_FORWARD => XBUTTON2 as u32,
        _ => 0,
    };
    let mi = MOUSEINPUT {
        dx: 0,
        dy: 0, // no movement
        mouseData: mouse_data,
        dwFlags: dw_flags,
        time: 0,
        dwExtraInfo: 0,
    };
    send_mouse_input(mi);
}

fn scroll(axis: u8, value: i32) {
    // WHEEL is positive up but HWHEEL positive right, like lan-mouse's horizontal axis
    let (event_type, value) = match axis {
        0 => (MOUSEEVENTF_WHEEL, value.saturating_neg()),
        1 => (MOUSEEVENTF_HWHEEL, value),
        _ => return,
    };
    let mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: value as u32,
        dwFlags: event_type,
        time: 0,
        dwExtraInfo: 0,
    };
    send_mouse_input(mi);
}

fn key_event(key: u32, state: u8) {
    let scancode = match linux_keycode_to_windows_scancode(key) {
        Some(code) => code,
        None => return,
    };
    let extended = scancode > 0xff;
    let scancode = scancode & 0xff;
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags.bitor_assign(KEYEVENTF_EXTENDEDKEY);
    }
    if state == 0 {
        flags.bitor_assign(KEYEVENTF_KEYUP);
    }
    let ki = KEYBDINPUT {
        wVk: Default::default(),
        wScan: scancode,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    send_keyboard_input(ki);
}

fn linux_keycode_to_windows_scancode(linux_keycode: u32) -> Option<u16> {
    let linux_scancode = match scancode::Linux::try_from(linux_keycode) {
        Ok(s) => s,
        Err(_) => {
            log::warn!("unknown keycode: {linux_keycode}");
            return None;
        }
    };
    log::trace!("linux code: {linux_scancode:?}");
    let windows_scancode = match scancode::Windows::try_from(linux_scancode) {
        Ok(s) => s,
        Err(_) => {
            log::warn!("failed to translate linux code into windows scancode: {linux_scancode:?}");
            return None;
        }
    };
    log::trace!("windows code: {windows_scancode:?}");
    Some(windows_scancode as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fractional_motion_accumulation() {
        let mut emu = WindowsEmulation::new().unwrap();

        // Sub-pixel moves at 1000 Hz: dx = 0.4 each
        assert_eq!(emu.accumulate_motion(0.4, 0.0), (0, 0));
        assert_eq!(emu.accumulate_motion(0.4, 0.0), (0, 0));
        // 3rd event: total = 1.2 -> outputs 1, remainder 0.2
        assert_eq!(emu.accumulate_motion(0.4, 0.0), (1, 0));
        // 4th event: total = 0.6 -> outputs 0, remainder 0.6
        assert_eq!(emu.accumulate_motion(0.4, 0.0), (0, 0));
        // 5th event: total = 1.0 -> outputs 1, remainder 0.0
        assert_eq!(emu.accumulate_motion(0.4, 0.0), (1, 0));
        assert!((emu.fractional_dx.abs()) < 1e-6);
    }

    #[test]
    fn test_negative_fractional_motion_accumulation() {
        let mut emu = WindowsEmulation::new().unwrap();

        assert_eq!(emu.accumulate_motion(-0.6, -0.4), (0, 0));
        // total dx = -1.2 -> int_dx = -1, remainder -0.2
        // total dy = -0.8 -> int_dy = 0, remainder -0.8
        assert_eq!(emu.accumulate_motion(-0.6, -0.4), (-1, 0));
        // total dy = -1.2 -> int_dy = -1, remainder -0.2
        assert_eq!(emu.accumulate_motion(0.0, -0.4), (0, -1));
    }

    #[tokio::test]
    async fn test_destroy_resets_fractional_deltas() {
        let mut emu = WindowsEmulation::new().unwrap();
        assert_eq!(emu.accumulate_motion(0.7, -0.7), (0, 0));
        assert_ne!(emu.fractional_dx, 0.0);
        assert_ne!(emu.fractional_dy, 0.0);

        emu.destroy(0).await;
        assert_eq!(emu.fractional_dx, 0.0);
        assert_eq!(emu.fractional_dy, 0.0);
    }
}
