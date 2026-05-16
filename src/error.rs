use std::path::PathBuf;

use thiserror::Error;
use windows::core::Error as WindowsError;
use xcap::XCapError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Windows API error: {0}")]
    Windows(#[from] WindowsError),

    #[error("Screen capture backend error: {0}")]
    XCap(#[from] XCapError),

    #[error("{0}")]
    Message(String),

    #[error("No foreground window is available")]
    NoForegroundWindow,

    #[error("No visible window matched title filter: {0}")]
    WindowNotFound(String),

    #[error("Captured frame is empty")]
    EmptyCapture,

    #[error(
        "Frame dimensions changed during capture: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    FrameSizeChanged {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },

    #[error("Unable to detect a stable overlap between consecutive frames")]
    OverlapNotFound,

    #[error("Frame count and overlap count are inconsistent")]
    InvalidStitchState,

    #[error("Failed to write image to {path}: {source}")]
    SaveImage {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
}
