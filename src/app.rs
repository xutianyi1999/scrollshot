use std::io::Write;
use std::thread;
use std::time::Duration;

use clap::Parser;

use crate::capture::{CaptureBackend, ScreenCapture};
use crate::capture_progress::{CaptureDecision, CaptureProgress};
use crate::cli::Cli;
use crate::error::AppResult;
use crate::region::select_capture_region;
use crate::scroll::ScrollController;
use crate::stitch::{
    detect_vertical_overlap, frames_near_stagnant, scroll_progress_evident, stitch_vertical,
};

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};

const ESC_POLL_INTERVAL_MS: u64 = 25;
const CAPTURE_ATTEMPTS_PER_SCROLL: usize = 2;
const MIN_RETRY_SETTLE_MS: u64 = 100;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "{}", record.args()))
        .init();

    if let Err(error) = capture_scrollshot() {
        log::error!("{error}");
        std::process::exit(1);
    }
}

fn capture_scrollshot() -> AppResult<()> {
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
    let mut progress = CaptureProgress::default();

    let first = capture.capture()?;
    frames.push(first);

    'capture: for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }

        let recovering = progress.is_recovering();
        let wheel_notches = if recovering { 1 } else { cli.wheel_notches };
        let settle_ms = if recovering {
            cli.settle_ms.saturating_mul(2)
        } else {
            cli.settle_ms
        };
        if recovering {
            log::debug!(
                "overlap recovery: scrolling 1 notch and waiting {settle_ms}ms before recapturing"
            );
        }
        scroller.scroll_down_once(
            (selection.scroll_point.x, selection.scroll_point.y),
            wheel_notches,
        )?;
        if wait_for_scroll_settle_or_escape(settle_ms) {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }
        let mut next = capture.capture()?;
        for attempt in 0..CAPTURE_ATTEMPTS_PER_SCROLL {
            let previous = frames.last().expect("at least one frame exists");

            validate_frame_dimensions(previous, &next)?;

            let mut unmatched = false;
            let decision = if frames_near_stagnant(previous, &next) {
                // Sparse scrolled content (tall blank table cells) can satisfy
                // the axis heuristic while the document still moves; yield to
                // it only when a verified shifted match cannot prove progress.
                match detect_vertical_overlap(previous, &next, None) {
                    Some(overlap) if scroll_progress_evident(previous, &next, overlap) => {
                        progress.record_measured_with_height(overlap, Some(next.height()))
                    }
                    _ => progress.record_stagnant(),
                }
            } else if let Some(overlap) = detect_vertical_overlap(previous, &next, None) {
                progress.record_measured_with_height(overlap, Some(next.height()))
            } else if attempt + 1 < CAPTURE_ATTEMPTS_PER_SCROLL {
                let retry_settle_ms = (cli.settle_ms / 2).max(MIN_RETRY_SETTLE_MS);
                log::debug!(
                    "overlap detection missed; waiting {retry_settle_ms}ms before recapturing the same position"
                );
                if wait_for_scroll_settle_or_escape(retry_settle_ms) {
                    log::warn!("stopped early by Esc; saving the captured portion");
                    break 'capture;
                }
                next = capture.capture()?;
                continue;
            } else {
                unmatched = true;
                progress.record_unmatched()
            };

            match decision {
                CaptureDecision::AppendMeasured(overlap) => {
                    overlaps.push(overlap);
                    frames.push(next);
                    break;
                }
                CaptureDecision::Retry if unmatched => {
                    let attempts = progress.recovery_attempts();
                    if attempts == 1 {
                        log::warn!(
                            "overlap detection missed; entering cautious recovery instead of guessing a seam"
                        );
                    } else {
                        log::debug!(
                            "overlap detection missed; discarding frame during cautious recovery attempt {attempts}"
                        );
                    }
                    break;
                }
                CaptureDecision::Retry => {
                    log::debug!(
                        "capture produced no progress evidence; retrying after another scroll"
                    );
                    break;
                }
                CaptureDecision::ReachedBottom => {
                    log::info!("reached page bottom after two stagnant captures");
                    break 'capture;
                }
                CaptureDecision::StopUnreliable => {
                    if frames.len() == 1 {
                        return Err(crate::error::AppError::OverlapNotFound);
                    }
                    log::warn!(
                        "overlap detection remained unreliable for too many captures; saving the captured portion"
                    );
                    break 'capture;
                }
            }
        }
    }

    let measured_overlaps = progress.measured_overlaps();
    if !measured_overlaps.is_empty() {
        let recent: Vec<u32> = measured_overlaps
            .iter()
            .rev()
            .take(10)
            .copied()
            .rev()
            .collect();
        let avg =
            measured_overlaps.iter().copied().sum::<u32>() as f64 / measured_overlaps.len() as f64;
        log::info!(
            "{} measured overlaps, last 10: {:?}, avg {:.1} px",
            overlaps.len(),
            recent,
            avg
        );
    } else {
        log::info!("no reliable overlaps were captured");
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

fn validate_frame_dimensions(
    previous: &image::RgbaImage,
    next: &image::RgbaImage,
) -> AppResult<()> {
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
