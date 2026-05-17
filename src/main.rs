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
use std::io::Write;

use crate::capture::{CaptureBackend, ScreenCapture};
use crate::cli::Cli;
use crate::error::AppResult;
use crate::region::select_capture_region;
use crate::scroll::ScrollController;
use crate::stitch::{
    detect_vertical_overlap, frames_near_stagnant, stitch_vertical,
};

use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

const ESC_POLL_INTERVAL_MS: u64 = 25;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format(|buf, record| writeln!(buf, "{}", record.args()))
    .init();

    if let Err(error) = run() {
        log::error!("{error}");
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
    scroller.focus_target((selection.scroll_point.x, selection.scroll_point.y))?;

    let mut frames = Vec::with_capacity(cli.max_scrolls.saturating_add(1));
    let mut overlaps = Vec::with_capacity(cli.max_scrolls);
    let mut measured_overlaps = Vec::with_capacity(cli.max_scrolls);

    let first = capture.capture()?;
    frames.push(first);

    for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }

        scroller.scroll_down_once((selection.scroll_point.x, selection.scroll_point.y), cli.wheel_notches)?;
        if wait_for_scroll_settle_or_escape(cli.settle_ms) {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }
        let next = capture.capture()?;
        let previous = frames.last().expect("at least one frame exists");

        validate_frame_dimensions(previous, &next)?;

        if frames_near_stagnant(previous, &next) {
            log::info!("reached page bottom");
            break;
        }

        let avg_overlap = (!measured_overlaps.is_empty()).then(|| {
            measured_overlaps.iter().copied().sum::<u32>() as f32 / measured_overlaps.len() as f32
        });
        if let Some(overlap) = detect_vertical_overlap(previous, &next, avg_overlap) {
            let smoothed = smooth_overlap(overlap, &measured_overlaps);
            overlaps.push(overlap);
            measured_overlaps.push(smoothed);
            frames.push(next);

            if measured_overlaps.len() >= 4 {
                let count = measured_overlaps.len() - 1;
                let recent: Vec<u32> = measured_overlaps[..count].iter().rev().take(5).copied().collect();
                let avg = recent.iter().copied().sum::<u32>() as f64 / recent.len() as f64;
                let deviation = (overlap as f64 - avg).abs() / avg.max(1.0);
                if deviation > 0.01 {
                    log::info!("overlap {overlap}px deviates {:.1}% from recent pattern ({avg:.0}px); page bottom likely reached", deviation * 100.0);
                    break;
                }
            }
        } else if let Some(overlap) = estimate_overlap_from_history(&measured_overlaps, next.height()) {
            log::info!(
                "continuing with estimated overlap {} from history ({} prior frames)",
                overlap,
                measured_overlaps.len()
            );
            overlaps.push(overlap);
            frames.push(next);
            continue;
        } else if frames.len() == 1 {
            return Err(crate::error::AppError::OverlapNotFound);
        } else {
            log::warn!(
                "overlap detection became unreliable after {} frame(s); saving the captured portion",
                frames.len()
            );
            log::info!("[break: unreliable]");
            break;
        }
    }

    let estimate_count = overlaps.len() - measured_overlaps.len();
    if !measured_overlaps.is_empty() {
        let recent: Vec<u32> = measured_overlaps.iter().rev().take(10).copied().rev().collect();
        let avg = measured_overlaps.iter().copied().sum::<u32>() as f64 / measured_overlaps.len() as f64;
        log::info!(
            "{} overlaps ({} estimated), last 10 measured: {:?}, avg {:.1} px",
            overlaps.len(),
            estimate_count,
            recent,
            avg
        );
    } else {
        log::info!("{} overlaps (all estimated)", overlaps.len());
    }

    let stitched = stitch_vertical(&frames, &overlaps)?;
    stitched
        .save(&cli.output)
        .map_err(|source| crate::error::AppError::SaveImage {
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
        return Err(crate::error::AppError::FrameSizeChanged {
            expected_width: previous.width(),
            expected_height: previous.height(),
            actual_width: next.width(),
            actual_height: next.height(),
        });
    }

    Ok(())
}

fn smooth_overlap(current: u32, measured: &[u32]) -> u32 {
    const SMOOTHING_WINDOW: usize = 3;
    if measured.len() < SMOOTHING_WINDOW {
        return current;
    }
    let recent = &measured[measured.len().saturating_sub(SMOOTHING_WINDOW - 1)..];
    let mut sorted = recent.to_vec();
    sorted.push(current);
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    if current.abs_diff(median) > 3 {
        median
    } else {
        current
    }
}

fn estimate_overlap_from_history(overlaps: &[u32], frame_height: u32) -> Option<u32> {
    let recent: Vec<u32> = overlaps.iter().rev().take(10).copied().collect();
    if recent.len() < 2 {
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
    use super::smooth_overlap;

    #[test]
    fn smooth_overlap_passes_through_normal_values() {
        assert_eq!(smooth_overlap(100, &[98, 99, 101]), 100);
    }

    #[test]
    fn smooth_overlap_clamps_single_frame_outlier() {
        assert_eq!(smooth_overlap(200, &[98, 99, 101]), 101);
    }

    #[test]
    fn smooth_overlap_requires_no_history() {
        assert_eq!(smooth_overlap(100, &[]), 100);
        assert_eq!(smooth_overlap(100, &[90]), 100);
    }
}
