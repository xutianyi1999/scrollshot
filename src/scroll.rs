use std::mem::size_of;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

use crate::error::{AppError, AppResult};

const WHEEL_DELTA: i32 = 120;
const FOCUS_SETTLE_MS: u64 = 80;

pub struct ScrollController {
    settle_ms: u64,
    wheel_notches: i32,
}

impl ScrollController {
    pub fn new(settle_ms: u64, wheel_notches: i32) -> Self {
        Self {
            settle_ms,
            wheel_notches,
        }
    }

    pub fn focus_target(&self, point: POINT) -> AppResult<()> {
        unsafe { SetCursorPos(point.x, point.y)? };

        let input = [
            mouse_input(MOUSE_EVENT_FLAGS(MOUSEEVENTF_LEFTDOWN.0), 0),
            mouse_input(MOUSE_EVENT_FLAGS(MOUSEEVENTF_LEFTUP.0), 0),
        ];
        let sent = unsafe { SendInput(&input, size_of::<INPUT>() as i32) };
        if sent != input.len() as u32 {
            return Err(AppError::Message(
                "SendInput failed to click the chosen scroll focus point".to_string(),
            ));
        }

        thread::sleep(Duration::from_millis(FOCUS_SETTLE_MS));
        Ok(())
    }

    pub fn scroll_down_once(&self, point: POINT) -> AppResult<()> {
        unsafe { SetCursorPos(point.x, point.y)? };

        let input = [mouse_input(
            MOUSE_EVENT_FLAGS(MOUSEEVENTF_WHEEL.0),
            (-WHEEL_DELTA * self.wheel_notches) as u32,
        )];
        let sent = unsafe { SendInput(&input, size_of::<INPUT>() as i32) };
        if sent != input.len() as u32 {
            return Err(AppError::Message(
                "SendInput failed to issue the downward mouse wheel event".to_string(),
            ));
        }

        Ok(())
    }

    pub fn settle_ms(&self) -> u64 {
        self.settle_ms
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, mouse_data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
