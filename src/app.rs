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
use crate::stitch::{detect_vertical_overlap, frames_near_stagnant, stitch_vertical};

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};

const ESC_POLL_INTERVAL_MS: u64 = 25;

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

    for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }

        scroller.scroll_down_once(
            (selection.scroll_point.x, selection.scroll_point.y),
            cli.wheel_notches,
        )?;
        if wait_for_scroll_settle_or_escape(cli.settle_ms) {
            log::warn!("stopped early by Esc; saving the captured portion");
            break;
        }
        let next = capture.capture()?;
        let previous = frames.last().expect("at least one frame exists");

        validate_frame_dimensions(previous, &next)?;

        let decision = if frames_near_stagnant(previous, &next) {
            progress.record_stagnant()
        } else if let Some(overlap) =
            detect_vertical_overlap(previous, &next, progress.expected_overlap())
        {
            progress.record_measured_with_height(overlap, Some(next.height()))
        } else {
            progress.record_unmatched(next.height())
        };

        match decision {
            CaptureDecision::AppendMeasured(overlap) => {
                overlaps.push(overlap);
                frames.push(next);
            }
            CaptureDecision::AppendEstimated(overlap) => {
                log::warn!(
                    "overlap detection missed; using bounded history estimate {}px",
                    overlap
                );
                overlaps.push(overlap);
                frames.push(next);
            }
            CaptureDecision::Retry => {
                log::debug!("capture produced no progress evidence; retrying after another scroll");
            }
            CaptureDecision::ReachedBottom => {
                log::info!("reached page bottom after two stagnant captures");
                break;
            }
            CaptureDecision::StopUnreliable => {
                if frames.len() == 1 {
                    return Err(crate::error::AppError::OverlapNotFound);
                }
                log::warn!(
                    "overlap detection remained unreliable for too many captures; saving the captured portion"
                );
                break;
            }
        }
    }

    let measured_overlaps = progress.measured_overlaps();
    let estimate_count = overlaps.len().saturating_sub(measured_overlaps.len());
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
