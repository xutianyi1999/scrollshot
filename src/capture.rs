use std::ffi::c_void;
use std::mem::size_of;

use image::RgbaImage;
use xcap::Monitor;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC, HGDIOBJ, RGBQUAD,
    ReleaseDC, SRCCOPY, SelectObject,
};

use crate::error::{AppError, AppResult};
use crate::screen_rect::ScreenRect;

pub trait CaptureBackend {
    fn capture(&self) -> AppResult<RgbaImage>;
}

pub(crate) enum ScreenCapture {
    Xcap(XcapScreenCapture),
    Gdi(GdiScreenCapture),
}

impl ScreenCapture {
    pub fn new(rect: ScreenRect) -> AppResult<Self> {
        Ok(match XcapScreenCapture::try_new(rect)? {
            Some(capture) => Self::Xcap(capture),
            None => Self::Gdi(GdiScreenCapture::new(rect)),
        })
    }
}

impl CaptureBackend for ScreenCapture {
    fn capture(&self) -> AppResult<RgbaImage> {
        match self {
            Self::Xcap(capture) => capture.capture(),
            Self::Gdi(capture) => capture.capture(),
        }
    }
}

pub(crate) struct XcapScreenCapture {
    monitor: Monitor,
    offset_x: u32,
    offset_y: u32,
    width: u32,
    height: u32,
}

impl XcapScreenCapture {
    fn try_new(rect: ScreenRect) -> AppResult<Option<Self>> {
        let monitor = Monitor::from_point(rect.left, rect.top)?;
        let monitor_left = monitor.x()?;
        let monitor_top = monitor.y()?;
        let monitor_right = monitor_left + monitor.width()? as i32;
        let monitor_bottom = monitor_top + monitor.height()? as i32;

        if rect.right() > monitor_right || rect.bottom() > monitor_bottom {
            return Ok(None);
        }

        Ok(Some(Self {
            monitor,
            offset_x: (rect.left - monitor_left) as u32,
            offset_y: (rect.top - monitor_top) as u32,
            width: rect.width as u32,
            height: rect.height as u32,
        }))
    }
}

impl CaptureBackend for XcapScreenCapture {
    fn capture(&self) -> AppResult<RgbaImage> {
        let image = self
            .monitor
            .capture_region(self.offset_x, self.offset_y, self.width, self.height)?;
        if image.width() == 0 || image.height() == 0 {
            return Err(AppError::EmptyCapture);
        }

        Ok(image)
    }
}

pub struct GdiScreenCapture {
    rect: ScreenRect,
}

impl GdiScreenCapture {
    fn new(rect: ScreenRect) -> Self {
        Self { rect }
    }
}

impl CaptureBackend for GdiScreenCapture {
    fn capture(&self) -> AppResult<RgbaImage> {
        let rect = self.rect;
        let width = rect.width;
        let height = rect.height;

        let screen_dc = ScreenDc::acquire()?;
        let memory_dc = MemoryDc::create(screen_dc.handle)?;
        let bitmap = OwnedBitmap::create(screen_dc.handle, width, height)?;
        let _selected = SelectedBitmap::select(memory_dc.handle, bitmap.handle)?;

        unsafe {
            BitBlt(
                memory_dc.handle,
                0,
                0,
                width,
                height,
                Some(screen_dc.handle),
                rect.left,
                rect.top,
                SRCCOPY,
            )?;
        }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };

        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let copied = unsafe {
            GetDIBits(
                memory_dc.handle,
                bitmap.handle,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast::<c_void>()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };

        if copied == 0 {
            return Err(AppError::Message("GetDIBits returned no pixels".to_string()));
        }

        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }

        RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or(AppError::EmptyCapture)
    }
}

struct ScreenDc {
    handle: HDC,
}

impl ScreenDc {
    fn acquire() -> AppResult<Self> {
        let handle = unsafe { GetDC(None) };
        if handle.0.is_null() {
            return Err(AppError::Message("GetDC returned a null HDC".to_string()));
        }

        Ok(Self { handle })
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseDC(None, self.handle);
        }
    }
}

struct MemoryDc {
    handle: HDC,
}

impl MemoryDc {
    fn create(reference: HDC) -> AppResult<Self> {
        let handle = unsafe { CreateCompatibleDC(Some(reference)) };
        if handle.0.is_null() {
            return Err(AppError::Message(
                "CreateCompatibleDC returned a null HDC".to_string(),
            ));
        }

        Ok(Self { handle })
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.handle);
        }
    }
}

struct OwnedBitmap {
    handle: HBITMAP,
}

impl OwnedBitmap {
    fn create(reference: HDC, width: i32, height: i32) -> AppResult<Self> {
        let handle = unsafe { CreateCompatibleBitmap(reference, width, height) };
        if handle.0.is_null() {
            return Err(AppError::Message(
                "CreateCompatibleBitmap returned a null bitmap".to_string(),
            ));
        }

        Ok(Self { handle })
    }
}

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.handle.0));
        }
    }
}

struct SelectedBitmap {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectedBitmap {
    fn select(dc: HDC, bitmap: HBITMAP) -> AppResult<Self> {
        let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        if previous.0.is_null() {
            return Err(AppError::Message(
                "SelectObject failed to attach the bitmap".to_string(),
            ));
        }

        Ok(Self { dc, previous })
    }
}

impl Drop for SelectedBitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
        }
    }
}
