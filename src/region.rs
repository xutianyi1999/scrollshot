use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect, HBRUSH, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, IDC_CROSS, LoadCursorW, MSG, PostQuitMessage, RegisterClassW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_LONG_PTR_INDEX, WINDOW_STYLE,
    WM_CLOSE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE,
    WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Error as WindowsError, w};

use crate::error::{AppError, AppResult};
use crate::screen_rect::ScreenRect;

const OVERLAY_CLASS: windows::core::PCWSTR = w!("ScrollshotSelectionOverlay");
const OVERLAY_ALPHA: u8 = 48;
const BACKGROUND_COLOR: COLORREF = COLORREF(0x00000000);
const BORDER_COLOR: COLORREF = COLORREF(0x0000FF00);

#[derive(Clone, Copy, Debug)]
pub struct RegionSelection {
    pub rect: ScreenRect,
    pub scroll_point: POINT,
}

pub fn select_capture_region() -> AppResult<RegionSelection> {
    println!("Hold left mouse button to drag a region, then click inside it to start. Press Esc to cancel.");

    let previous_foreground = unsafe { GetForegroundWindow() };
    let module = unsafe { GetModuleHandleW(None)? };
    let instance = HINSTANCE(module.0);
    register_overlay_class(instance)?;

    let mut state = Box::new(OverlayState::new(virtual_screen_rect()));
    let state_ptr = state.as_mut() as *mut OverlayState;

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0),
            OVERLAY_CLASS,
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            state.bounds.left,
            state.bounds.top,
            rect_width(state.bounds),
            rect_height(state.bounds),
            None,
            None,
            Some(instance),
            Some(state_ptr.cast()),
        )?
    };

    unsafe {
        SetLayeredWindowAttributes(
            hwnd,
            COLORREF(0),
            OVERLAY_ALPHA,
            windows::Win32::UI::WindowsAndMessaging::LWA_ALPHA,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }

    message_loop();

    unsafe {
        DestroyWindow(hwnd)?;
        let _ = UnregisterClassW(OVERLAY_CLASS, Some(instance));
        if !previous_foreground.0.is_null() {
            let _ = SetForegroundWindow(previous_foreground);
        }
    }

    let state = *state;
    if state.cancelled {
        return Err(AppError::Message("Region selection was cancelled".to_string()));
    }

    match (state.selection(), state.scroll_point) {
        (Some(rect), Some(scroll_point)) => Ok(RegionSelection { rect, scroll_point }),
        _ => Err(AppError::Message("Selected region is too small".to_string())),
    }
}

fn register_overlay_class(instance: HINSTANCE) -> AppResult<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_CROSS)? };
    let class = WNDCLASSW {
        hCursor: cursor,
        hInstance: instance,
        lpszClassName: OVERLAY_CLASS,
        lpfnWndProc: Some(selection_overlay_proc),
        ..Default::default()
    };

    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        return Err(AppError::Windows(WindowsError::from_thread()));
    }

    Ok(())
}

fn message_loop() {
    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status == false || status.0 == -1 {
            break;
        }

        unsafe {
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn selection_overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                create.lpCreateParams as *mut OverlayState as isize,
            );
        }
        return LRESULT(1);
    }

    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(GWLP_USERDATA.0)) }
        as *mut OverlayState;
    if state_ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &mut *state_ptr };

    match message {
        WM_LBUTTONDOWN => {
            let cursor = match cursor_position() {
                Ok(cursor) => cursor,
                Err(_) => return LRESULT(0),
            };

            match state.phase {
                OverlayPhase::Drawing => {
                    state.dragging = true;
                    state.start = Some(cursor);
                    state.current = Some(cursor);
                    unsafe {
                        let _ = SetCapture(hwnd);
                    }
                    let _ = redraw_overlay(hwnd);
                }
                OverlayPhase::AwaitingStartClick => {
                    if state.point_in_selection(cursor) {
                        state.scroll_point = Some(cursor);
                        unsafe { PostQuitMessage(0) };
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if state.dragging && let Ok(cursor) = cursor_position() {
                state.current = Some(cursor);
                let _ = redraw_overlay(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if state.dragging {
                state.dragging = false;
                if let Ok(cursor) = cursor_position() {
                    state.current = Some(cursor);
                }
                if state.selection().is_some() {
                    state.phase = OverlayPhase::AwaitingStartClick;
                }
                unsafe {
                    let _ = ReleaseCapture();
                }
                let _ = redraw_overlay(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0 as usize {
                state.cancelled = true;
                unsafe {
                    let _ = ReleaseCapture();
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CLOSE => {
            state.cancelled = true;
            unsafe {
                let _ = ReleaseCapture();
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let _ = paint_overlay(hwnd, state);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn paint_overlay(hwnd: HWND, state: &OverlayState) -> AppResult<()> {
    let mut paint = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    if hdc.0.is_null() {
        return Err(AppError::Message("BeginPaint returned a null HDC".to_string()));
    }

    let result = (|| {
        let background = Brush::solid(BACKGROUND_COLOR)?;
        let border = Brush::solid(BORDER_COLOR)?;

        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client)? };

        if unsafe { FillRect(hdc, &client, background.handle) } == 0 {
            return Err(AppError::Windows(WindowsError::from_thread()));
        }

        if let Some(rect) = state.client_selection_rect()
            && unsafe { FrameRect(hdc, &rect, border.handle) } == 0
        {
            return Err(AppError::Windows(WindowsError::from_thread()));
        }

        Ok(())
    })();

    unsafe {
        let _ = EndPaint(hwnd, &paint);
    }

    result
}

fn redraw_overlay(hwnd: HWND) -> AppResult<()> {
    if unsafe { InvalidateRect(Some(hwnd), None, false) } == false {
        return Err(AppError::Windows(WindowsError::from_thread()));
    }
    if unsafe { UpdateWindow(hwnd) } == false {
        return Err(AppError::Windows(WindowsError::from_thread()));
    }
    Ok(())
}

fn cursor_position() -> AppResult<POINT> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point)? };
    Ok(point)
}

fn virtual_screen_rect() -> RECT {
    RECT {
        left: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
        top: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
        right: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) },
        bottom: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN) },
    }
}

struct OverlayState {
    bounds: RECT,
    start: Option<POINT>,
    current: Option<POINT>,
    scroll_point: Option<POINT>,
    cancelled: bool,
    dragging: bool,
    phase: OverlayPhase,
}

impl OverlayState {
    fn new(bounds: RECT) -> Self {
        Self {
            bounds,
            start: None,
            current: None,
            scroll_point: None,
            cancelled: false,
            dragging: false,
            phase: OverlayPhase::Drawing,
        }
    }

    fn selection(&self) -> Option<ScreenRect> {
        ScreenRect::from_points(self.start?, self.current?)
    }

    fn client_selection_rect(&self) -> Option<RECT> {
        let rect = self.selection()?;
        Some(RECT {
            left: rect.left - self.bounds.left,
            top: rect.top - self.bounds.top,
            right: rect.right() - self.bounds.left,
            bottom: rect.bottom() - self.bounds.top,
        })
    }

    fn point_in_selection(&self, point: POINT) -> bool {
        self.selection()
            .map(|rect| {
                point.x >= rect.left
                    && point.x < rect.right()
                    && point.y >= rect.top
                    && point.y < rect.bottom()
            })
            .unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayPhase {
    Drawing,
    AwaitingStartClick,
}

struct Brush {
    handle: HBRUSH,
}

impl Brush {
    fn solid(color: COLORREF) -> AppResult<Self> {
        let handle = unsafe { CreateSolidBrush(color) };
        if handle.0.is_null() {
            return Err(AppError::Windows(WindowsError::from_thread()));
        }

        Ok(Self { handle })
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.handle.0));
        }
    }
}

fn rect_width(rect: RECT) -> i32 {
    rect.right - rect.left
}

fn rect_height(rect: RECT) -> i32 {
    rect.bottom - rect.top
}
