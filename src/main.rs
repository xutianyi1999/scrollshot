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
use crate::stitch::{
    detect_vertical_overlap, frames_are_similar, stitch_vertical,
};

use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

const MAX_STAGNANT_SCROLLS: usize = 3;
const ESC_POLL_INTERVAL_MS: u64 = 25;

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
    let mut measured_overlaps = Vec::with_capacity(cli.max_scrolls);
    let mut stagnant_scrolls = 0usize;

    let first = capture.capture()?;
    frames.push(first);

    for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            eprintln!("stopped early by Esc; saving the captured portion");
            break;
        }

        scroller.scroll_down_once(selection.scroll_point, cli.wheel_notches)?;
        if wait_for_scroll_settle_or_escape(cli.settle_ms) {
            eprintln!("stopped early by Esc; saving the captured portion");
            break;
        }
        let next = capture.capture()?;
        let previous = frames.last().expect("at least one frame exists");

        validate_frame_dimensions(previous, &next)?;

        if frames_are_similar(previous, &next) {
            stagnant_scrolls += 1;
            if stagnant_scrolls >= MAX_STAGNANT_SCROLLS {
                break;
            }
            continue;
        }
        stagnant_scrolls = 0;

        if let Some(overlap) = detect_vertical_overlap(previous, &next) {
            overlaps.push(overlap);
            measured_overlaps.push(overlap);
            frames.push(next);
        } else {

            if let Some(overlap) = estimate_overlap_from_history(&measured_overlaps, next.height()) {
                eprintln!(
                    "info: continuing with estimated overlap {} from history ({} prior frames)",
                    overlap,
                    measured_overlaps.len()
                );
                overlaps.push(overlap);
                frames.push(next);
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

    let estimate_count = overlaps.len() - measured_overlaps.len();
    if !measured_overlaps.is_empty() {
        let recent: Vec<u32> = measured_overlaps.iter().rev().take(10).copied().rev().collect();
        let avg = measured_overlaps.iter().copied().sum::<u32>() as f64 / measured_overlaps.len() as f64;
        eprintln!(
            "info: {} overlaps ({} estimated), last 10 measured: {:?}, avg {:.1} px",
            overlaps.len(),
            estimate_count,
            recent,
            avg
        );
    } else {
        eprintln!("info: {} overlaps (all estimated)", overlaps.len());
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

fn estimate_overlap_from_history(overlaps: &[u32], frame_height: u32) -> Option<u32> {
    let recent: Vec<u32> = overlaps.iter().rev().take(10).copied().collect();
    if recent.len() < 3 {
        return None;
    }
    let mut sorted = recent;
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    let min_allowed = (frame_height as f32 * 0.01).max(4.0) as u32;
    let max_allowed = frame_height.saturating_sub(2);
    if min_allowed > max_allowed {
        return None;
    }
    Some(median.clamp(min_allowed, max_allowed))
}

#[cfg(test)]
mod tests {
    use super::MAX_STAGNANT_SCROLLS;

    #[test]
    fn stop_threshold_prevents_immediate_break() {
        assert!(MAX_STAGNANT_SCROLLS > 1);
    }
}
