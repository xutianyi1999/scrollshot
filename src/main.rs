mod capture;
mod cli;
mod error;
mod region;
mod scroll;
mod stitch;
mod window;

use clap::Parser;

use crate::capture::{CaptureBackend, ScreenCapture};
use crate::cli::Cli;
use crate::error::{AppError, AppResult};
use crate::region::{RegionSelection, select_capture_region};
use crate::scroll::ScrollController;
use crate::stitch::{detect_vertical_overlap, frames_are_similar, stitch_vertical};
use crate::window::TargetWindow;

use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

const MAX_STAGNANT_SCROLLS: usize = 3;
const MAX_OVERLAP_MISSES: usize = 2;

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
    let selection = if cli.select_region {
        if let Some(title) = cli.title.as_deref() {
            TargetWindow::resolve(Some(title))?.activate()?;
        }
        select_capture_region()?
    } else {
        let target = TargetWindow::resolve(cli.title.as_deref())?;
        target.activate()?;
        let rect = target.client_rect()?;
        RegionSelection {
            rect,
            scroll_point: rect.center(),
        }
    };

    let capture = ScreenCapture::new(selection.rect)?;
    let scroller = ScrollController::new(cli.settle_ms, cli.wheel_notches);
    if cli.select_region {
        scroller.focus_target(selection.scroll_point)?;
    }

    let mut frames = Vec::with_capacity(cli.max_scrolls.saturating_add(1));
    let mut overlaps = Vec::with_capacity(cli.max_scrolls);
    let mut stagnant_scrolls = 0usize;
    let mut overlap_misses = 0usize;

    let first = capture.capture()?;
    frames.push(first);

    for _ in 0..cli.max_scrolls {
        if capture_cancelled_by_escape() {
            eprintln!("stopped early by Esc; saving the captured portion");
            break;
        }

        scroller.scroll_down_once(selection.scroll_point)?;
        let next = capture.capture()?;
        let previous = frames.last().expect("at least one frame exists");

        if previous.dimensions() != next.dimensions() {
            return Err(AppError::FrameSizeChanged {
                expected_width: previous.width(),
                expected_height: previous.height(),
                actual_width: next.width(),
                actual_height: next.height(),
            });
        }

        if frames_are_similar(previous, &next) {
            stagnant_scrolls += 1;
            if stagnant_scrolls >= MAX_STAGNANT_SCROLLS {
                break;
            }
            continue;
        }
        stagnant_scrolls = 0;

        if let Some(overlap) = detect_vertical_overlap(previous, &next) {
            overlap_misses = 0;
            overlaps.push(overlap);
            frames.push(next);
        } else {
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

#[cfg(test)]
mod tests {
    use super::{MAX_OVERLAP_MISSES, MAX_STAGNANT_SCROLLS};

    #[test]
    fn stop_thresholds_allow_short_retries() {
        assert!(MAX_STAGNANT_SCROLLS > 1);
        assert!(MAX_OVERLAP_MISSES > 1);
    }
}
