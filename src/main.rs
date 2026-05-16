mod capture;
mod cli;
mod error;
mod region;
mod screen_rect;
mod scroll;
mod stitch;

use std::thread;
use std::time::Duration;

use clap::Parser;

use crate::capture::{CaptureBackend, ScreenCapture};
use crate::cli::Cli;
use crate::error::{AppError, AppResult};
use crate::region::select_capture_region;
use crate::scroll::ScrollController;
use crate::stitch::{detect_vertical_overlap, frames_are_similar, stitch_vertical};

use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

const MAX_STAGNANT_SCROLLS: usize = 3;
const MAX_OVERLAP_MISSES: usize = 2;
const ESC_POLL_INTERVAL_MS: u64 = 25;
const OVERLAP_MISS_RETRY_MS: u64 = 80;
const MAX_SAME_POSITION_RECOVERIES: usize = 2;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    // Prevent DPI virtualization from distorting client coordinates and capture sizes.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let cli = Cli::parse();
    let selection = select_capture_region()?;

    let capture = ScreenCapture::new(selection.rect)?;
    let scroller = ScrollController::new(cli.settle_ms, cli.wheel_notches);
    scroller.focus_target(selection.scroll_point)?;

    let mut frames = Vec::with_capacity(cli.max_scrolls.saturating_add(1));
    let mut overlaps = Vec::with_capacity(cli.max_scrolls);
    let mut stagnant_scrolls = 0usize;
    let mut overlap_misses = 0usize;
    let mut retry_same_position = false;
    let mut same_position_recoveries = 0usize;

    let first = capture.capture()?;
    frames.push(first);

    for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            eprintln!("stopped early by Esc; saving the captured portion");
            break;
        }

        if retry_same_position {
            if wait_for_scroll_settle_or_escape(OVERLAP_MISS_RETRY_MS) {
                eprintln!("stopped early by Esc; saving the captured portion");
                break;
            }
        } else {
            scroller.scroll_down_once(selection.scroll_point)?;
            if wait_for_scroll_settle_or_escape(scroller.settle_ms()) {
                eprintln!("stopped early by Esc; saving the captured portion");
                break;
            }
        }
        let mut next = capture.capture()?;
        let previous = frames.last().expect("at least one frame exists");

        validate_frame_dimensions(previous, &next)?;

        if frames_are_similar(previous, &next) {
            retry_same_position = false;
            same_position_recoveries = 0;
            stagnant_scrolls += 1;
            if stagnant_scrolls >= MAX_STAGNANT_SCROLLS {
                break;
            }
            continue;
        }
        stagnant_scrolls = 0;

        let mut overlap = detect_vertical_overlap(previous, &next);
        if overlap.is_none() {
            if wait_for_scroll_settle_or_escape(OVERLAP_MISS_RETRY_MS) {
                eprintln!("stopped early by Esc; saving the captured portion");
                break;
            }

            let retry = capture.capture()?;
            validate_frame_dimensions(previous, &retry)?;
            if !frames_are_similar(previous, &retry) {
                overlap = detect_vertical_overlap(previous, &retry);
                if overlap.is_some() {
                    next = retry;
                } else if !frames_are_similar(&next, &retry)
                    && same_position_recoveries < MAX_SAME_POSITION_RECOVERIES
                {
                    retry_same_position = true;
                    same_position_recoveries += 1;
                    continue;
                }
            }
        }

        if let Some(overlap) = overlap {
            retry_same_position = false;
            same_position_recoveries = 0;
            overlap_misses = 0;
            overlaps.push(overlap);
            frames.push(next);
        } else {
            retry_same_position = false;
            same_position_recoveries = 0;
            overlap_misses += 1;
            if overlap_misses < MAX_OVERLAP_MISSES {
                continue;
            }

            if frames.len() == 1 {
                return Err(AppError::OverlapNotFound);
            }

            eprintln!(
                "warning: overlap detection became unreliable after {} frame(s); saving the captured portion",
                frames.len()
            );
            break;
        }
    }

    let stitched = stitch_vertical(&frames, &overlaps)?;
    stitched
        .save(&cli.output)
        .map_err(|source| AppError::SaveImage {
            path: cli.output.clone(),
            source,
        })?;

    println!(
        "saved {} frame(s) into {}",
        frames.len(),
        cli.output.display()
    );

    Ok(())
}

fn capture_cancelled_by_escape() -> bool {
    unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0 }
}

fn wait_for_scroll_settle_or_escape(settle_ms: u64) -> bool {
    let deadline = Duration::from_millis(settle_ms);
    let mut waited = Duration::ZERO;
    while waited < deadline {
        if capture_cancelled_by_escape() {
            return true;
        }

        let sleep_for = (deadline - waited).min(Duration::from_millis(ESC_POLL_INTERVAL_MS));
        thread::sleep(sleep_for);
        waited += sleep_for;
    }

    false
}

fn validate_frame_dimensions(previous: &image::RgbaImage, next: &image::RgbaImage) -> AppResult<()> {
    if previous.dimensions() != next.dimensions() {
        return Err(AppError::FrameSizeChanged {
            expected_width: previous.width(),
            expected_height: previous.height(),
            actual_width: next.width(),
            actual_height: next.height(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_OVERLAP_MISSES, MAX_SAME_POSITION_RECOVERIES, MAX_STAGNANT_SCROLLS};

    #[test]
    fn stop_thresholds_allow_short_retries() {
        assert!(MAX_STAGNANT_SCROLLS > 1);
        assert!(MAX_OVERLAP_MISSES > 1);
        assert!(MAX_SAME_POSITION_RECOVERIES > 1);
    }
}
