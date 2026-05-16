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
const NOTCH_REDUCTION_ON_MISS: i32 = 1;
const SETTLE_INCREMENT_MS: u64 = 50;
const MAX_SETTLE_MS: u64 = 1000;
const SUCCESSFUL_STEPS_BEFORE_RESTORE: usize = 3;
const MAX_CONSECUTIVE_MISSES_AT_MIN_NOTCH: usize = 5;
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
    let scroller = ScrollController::new();
    scroller.focus_target(selection.scroll_point)?;

    let mut frames = Vec::with_capacity(cli.max_scrolls.saturating_add(1));
    let mut overlaps = Vec::with_capacity(cli.max_scrolls);
    let mut stagnant_scrolls = 0usize;
    let mut current_notches = cli.wheel_notches;
    let mut current_settle_ms = cli.settle_ms;
    let mut consecutive_misses_at_min = 0usize;
    let mut successful_steps = 0usize;
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
            scroller.scroll_down_once(selection.scroll_point, current_notches)?;
            if wait_for_scroll_settle_or_escape(current_settle_ms) {
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
            consecutive_misses_at_min = 0;
            successful_steps += 1;
            if successful_steps >= SUCCESSFUL_STEPS_BEFORE_RESTORE {
                let restored = current_notches < cli.wheel_notches;
                let settled = current_settle_ms > cli.settle_ms;
                if restored {
                    current_notches =
                        (current_notches + NOTCH_REDUCTION_ON_MISS).min(cli.wheel_notches);
                }
                if settled {
                    current_settle_ms = current_settle_ms
                        .saturating_sub(SETTLE_INCREMENT_MS)
                        .max(cli.settle_ms);
                }
                if restored || settled {
                    successful_steps = 0;
                }
            }
            overlaps.push(overlap);
            frames.push(next);
        } else {
            retry_same_position = false;
            same_position_recoveries = 0;
            successful_steps = 0;

            if current_notches > 1 {
                current_notches = (current_notches - NOTCH_REDUCTION_ON_MISS).max(1);
                current_settle_ms = (current_settle_ms + SETTLE_INCREMENT_MS).min(MAX_SETTLE_MS);
                eprintln!(
                    "info: reduced scroll step to {} notch(es), settle {} ms",
                    current_notches, current_settle_ms
                );
                continue;
            }

            consecutive_misses_at_min += 1;
            current_settle_ms = (current_settle_ms + SETTLE_INCREMENT_MS).min(MAX_SETTLE_MS);
            if consecutive_misses_at_min < MAX_CONSECUTIVE_MISSES_AT_MIN_NOTCH {
                eprintln!(
                    "info: increased settle to {} ms for better overlap",
                    current_settle_ms
                );
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
    use super::{MAX_CONSECUTIVE_MISSES_AT_MIN_NOTCH, MAX_SAME_POSITION_RECOVERIES, MAX_STAGNANT_SCROLLS};

    #[test]
    fn stop_thresholds_allow_short_retries() {
        assert!(MAX_STAGNANT_SCROLLS > 1);
        assert!(MAX_CONSECUTIVE_MISSES_AT_MIN_NOTCH > 1);
        assert!(MAX_SAME_POSITION_RECOVERIES > 1);
    }
}
