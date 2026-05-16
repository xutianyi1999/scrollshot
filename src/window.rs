use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::Error as WindowsError;

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    pub fn from_points(a: POINT, b: POINT) -> Option<Self> {
        let left = a.x.min(b.x);
        let top = a.y.min(b.y);
        let right = a.x.max(b.x);
        let bottom = a.y.max(b.y);
        let width = right - left;
        let height = bottom - top;

        (width > 0 && height > 0).then_some(Self {
            left,
            top,
            width,
            height,
        })
    }

    pub fn center(&self) -> POINT {
        POINT {
            x: self.left + (self.width / 2),
            y: self.top + (self.height / 2),
        }
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }
}

#[derive(Clone, Debug)]
pub struct TargetWindow {
    pub hwnd: HWND,
    pub title: String,
}

struct SearchState {
    needle: String,
    result: Option<TargetWindow>,
}

impl TargetWindow {
    pub fn resolve(title_filter: Option<&str>) -> AppResult<Self> {
        match title_filter {
            Some(filter) => find_by_title(filter),
            None => from_foreground_window(),
        }
    }

    pub fn activate(&self) -> AppResult<()> {
        unsafe {
            if IsIconic(self.hwnd) != false {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }

            let _ = SetForegroundWindow(self.hwnd);
        }

        thread::sleep(Duration::from_millis(150));
        let active = unsafe { GetForegroundWindow() };
        if active != self.hwnd {
            return Err(AppError::Message(format!(
                "Failed to bring '{}' to the foreground",
                self.title
            )));
        }

        Ok(())
    }

    pub fn client_rect(&self) -> AppResult<ScreenRect> {
        client_rect_in_screen_space(self.hwnd)
    }
}

fn from_foreground_window() -> AppResult<TargetWindow> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Err(AppError::NoForegroundWindow);
    }

    Ok(TargetWindow {
        hwnd,
        title: window_title(hwnd),
    })
}

fn find_by_title(filter: &str) -> AppResult<TargetWindow> {
    let mut state = SearchState {
        needle: filter.to_lowercase(),
        result: None,
    };

    unsafe {
        EnumWindows(
            Some(enum_windows_proc),
            LPARAM((&mut state as *mut SearchState).cast::<()>() as isize),
        )?;
    }

    state
        .result
        .ok_or_else(|| AppError::WindowNotFound(filter.to_string()))
}

unsafe extern "system" fn enum_windows_proc(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::core::BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut SearchState) };
    if unsafe { IsWindowVisible(hwnd) } == false {
        return true.into();
    }

    let title = window_title(hwnd);
    if title.is_empty() {
        return true.into();
    }

    if title.to_lowercase().contains(&state.needle) {
        state.result = Some(TargetWindow { hwnd, title });
        return false.into();
    }

    true.into()
}

pub fn client_rect_in_screen_space(hwnd: HWND) -> AppResult<ScreenRect> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client)? };

    let mut top_left = POINT {
        x: client.left,
        y: client.top,
    };
    let mut bottom_right = POINT {
        x: client.right,
        y: client.bottom,
    };

    unsafe {
        if ClientToScreen(hwnd, &mut top_left) == false {
            return Err(AppError::Windows(WindowsError::from_thread()));
        }
        if ClientToScreen(hwnd, &mut bottom_right) == false {
            return Err(AppError::Windows(WindowsError::from_thread()));
        }
    }

    let width = bottom_right.x - top_left.x;
    let height = bottom_right.y - top_left.y;
    if width <= 0 || height <= 0 {
        return Err(AppError::Message(
            "Target window has an empty client area".to_string(),
        ));
    }

    Ok(ScreenRect {
        left: top_left.x,
        top: top_left.y,
        width,
        height,
    })
}

fn window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }

    let mut buf = vec![0u16; len as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..written as usize])
}
