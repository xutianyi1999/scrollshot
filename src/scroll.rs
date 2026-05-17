use std::thread;
use std::time::Duration;

use enigo::{
    Axis, Button, Coordinate, Direction::Click, Enigo, Mouse, Settings,
};

use crate::error::{AppError, AppResult};

const FOCUS_SETTLE_MS: u64 = 80;

pub struct ScrollController;

impl ScrollController {
    pub fn new() -> Self {
        Self
    }

    pub fn focus_target(&self, point: (i32, i32)) -> AppResult<()> {
        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| AppError::Message(format!("enigo init failed: {e}")))?;
        enigo
            .move_mouse(point.0, point.1, Coordinate::Abs)
            .map_err(|e| AppError::Message(format!("move_mouse failed: {e}")))?;
        enigo
            .button(Button::Left, Click)
            .map_err(|e| AppError::Message(format!("button click failed: {e}")))?;

        thread::sleep(Duration::from_millis(FOCUS_SETTLE_MS));
        Ok(())
    }

    pub fn scroll_down_once(&self, point: (i32, i32), notches: i32) -> AppResult<()> {
        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| AppError::Message(format!("enigo init failed: {e}")))?;
        enigo
            .move_mouse(point.0, point.1, Coordinate::Abs)
            .map_err(|e| AppError::Message(format!("move_mouse failed: {e}")))?;
        enigo
            .scroll(notches, Axis::Vertical)
            .map_err(|e| AppError::Message(format!("scroll failed: {e}")))?;
        Ok(())
    }

}
