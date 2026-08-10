#![allow(dead_code, unused_imports)]

mod error {
    #[derive(Debug)]
    pub enum AppError {
        Message(String),
        InvalidStitchState,
    }

    pub type AppResult<T> = Result<T, AppError>;
}

#[path = "../src/stitch.rs"]
mod stitch;

#[path = "support/mod.rs"]
mod support;

#[path = "../src/capture_progress.rs"]
mod capture_progress;

use capture_progress::{CaptureDecision, CaptureProgress};
use image::{Rgba, RgbaImage};
use stitch::{detect_vertical_overlap, frames_near_stagnant, stitch_vertical};

#[derive(Debug, PartialEq, Eq)]
enum HeadlessCaptureEnd {
    ReachedBottom,
    Unreliable,
    ExhaustedInput,
}

fn run_headless_capture(input: &[RgbaImage]) -> (Vec<RgbaImage>, Vec<u32>, HeadlessCaptureEnd) {
    let mut frames = vec![input.first().expect("at least one frame").clone()];
    let mut overlaps = Vec::new();
    let mut progress = CaptureProgress::default();

    for next in &input[1..] {
        let previous = frames.last().unwrap();
        let decision = if frames_near_stagnant(previous, next) {
            progress.record_stagnant()
        } else if let Some(overlap) =
            detect_vertical_overlap(previous, next, progress.expected_overlap())
        {
            progress.record_measured_with_height(overlap, Some(next.height()))
        } else {
            progress.record_unmatched(next.height())
        };

        match decision {
            CaptureDecision::AppendMeasured(overlap)
            | CaptureDecision::AppendEstimated(overlap) => {
                overlaps.push(overlap);
                frames.push(next.clone());
            }
            CaptureDecision::Retry => {}
            CaptureDecision::ReachedBottom => {
                return (frames, overlaps, HeadlessCaptureEnd::ReachedBottom);
            }
            CaptureDecision::StopUnreliable => {
                return (frames, overlaps, HeadlessCaptureEnd::Unreliable);
            }
        }
    }

    (frames, overlaps, HeadlessCaptureEnd::ExhaustedInput)
}

#[test]
fn dense_text_detects_real_scroll_offsets() {
    let source = support::dense_text_source(480, 2400);
    let frame_height = 480;
    let offsets = [0, 263, 517, 778];

    for pair in offsets.windows(2) {
        let previous = support::crop(&source, pair[0], frame_height);
        let current = support::crop(&source, pair[1], frame_height);
        let expected = frame_height - (pair[1] - pair[0]);
        let detected = detect_vertical_overlap(&previous, &current, None)
            .expect("dense text overlap should be detected");
        assert!(
            detected.abs_diff(expected) <= 1,
            "expected overlap {expected}, got {detected}"
        );
    }
}

#[test]
fn dense_text_1280x720_detects_real_scroll_offset() {
    let source = support::dense_text_source(1280, 2400);
    let frame_height = 720;
    let previous = support::crop(&source, 0, frame_height);
    let current = support::crop(&source, 400, frame_height);

    let detected = detect_vertical_overlap(&previous, &current, None)
        .expect("high-resolution dense text overlap should be detected");
    assert!(
        detected.abs_diff(320) <= 1,
        "expected overlap 320, got {detected}"
    );
}

#[test]
fn dense_text_stitch_reconstructs_source() {
    let source = support::dense_text_source(320, 1500);
    let frame_height = 420;
    let starts = [0, 211, 453, 688, 931, 1080];
    let frames: Vec<RgbaImage> = starts
        .iter()
        .map(|start| support::crop(&source, *start, frame_height))
        .collect();
    let expected: Vec<u32> = starts
        .windows(2)
        .map(|pair| frame_height - (pair[1] - pair[0]))
        .collect();

    let detected: Vec<u32> = frames
        .windows(2)
        .map(|pair| detect_vertical_overlap(&pair[0], &pair[1], None).unwrap())
        .collect();
    assert!(
        detected
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| actual.abs_diff(*expected) <= 1),
        "detected overlaps: {detected:?}, expected: {expected:?}"
    );

    let stitched = stitch_vertical(&frames, &detected).unwrap();
    assert_eq!(stitched.dimensions(), source.dimensions());
    assert_eq!(stitched, source);
}

#[test]
fn dense_text_stitch_reconstructs_variable_scroll_steps_pixel_for_pixel() {
    let source = support::dense_text_source(640, 2400);
    let frame_height = 520;
    let starts = [0, 173, 421, 612, 903, 1177, 1459, 1772];
    let frames: Vec<RgbaImage> = starts
        .iter()
        .map(|start| support::crop(&source, *start, frame_height))
        .collect();

    let overlaps: Vec<u32> = frames
        .windows(2)
        .map(|pair| {
            detect_vertical_overlap(&pair[0], &pair[1], None)
                .expect("every variable scroll step should have a stable overlap")
        })
        .collect();
    let expected: Vec<u32> = starts
        .windows(2)
        .map(|pair| frame_height - (pair[1] - pair[0]))
        .collect();
    assert!(
        overlaps
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| actual.abs_diff(*expected) <= 1),
        "detected overlaps: {overlaps:?}, expected: {expected:?}"
    );

    let stitched = stitch_vertical(&frames, &overlaps).unwrap();
    assert_eq!(
        stitched,
        support::crop(&source, 0, starts.last().unwrap() + frame_height)
    );
}

#[test]
fn headless_capture_flow_keeps_variable_scrolls_and_stops_at_bottom() {
    let source = support::dense_text_source(640, 2400);
    let frame_height = 520;
    let starts = [0, 173, 421, 612, 903, 1177, 1459, 1772];
    let mut input: Vec<RgbaImage> = starts
        .iter()
        .map(|start| support::crop(&source, *start, frame_height))
        .collect();
    let bottom = input.last().unwrap().clone();
    input.push(support::add_animated_panel(&bottom, 0, 48, 65));
    input.push(support::add_animated_panel(&bottom, 0, 48, 130));

    let (frames, overlaps, end) = run_headless_capture(&input);
    assert_eq!(end, HeadlessCaptureEnd::ReachedBottom);
    assert_eq!(frames.len(), starts.len());

    let stitched = stitch_vertical(&frames, &overlaps).unwrap();
    assert_eq!(
        stitched,
        support::crop(&source, 0, starts.last().unwrap() + frame_height)
    );
}

#[test]
fn dense_text_tolerates_small_render_noise() {
    let source = support::dense_text_source(420, 1200);
    let frame_height = 420;
    let previous = support::crop(&source, 0, frame_height);
    let current = support::add_render_noise(&support::crop(&source, 197, frame_height), 1);
    let expected = frame_height - 197;

    let detected = detect_vertical_overlap(&previous, &current, None)
        .expect("small render noise should not invalidate overlap");
    assert!(detected.abs_diff(expected) <= 1);
}

#[test]
fn dense_text_recovers_when_history_is_wrong() {
    let source = support::dense_text_source(420, 1200);
    let previous = support::crop(&source, 0, 420);
    let current = support::crop(&source, 197, 420);

    let detected = detect_vertical_overlap(&previous, &current, Some(80.0))
        .expect("a wrong history value must fall back to a full search");
    assert!(detected.abs_diff(223) <= 1);
}

#[test]
fn dark_dense_text_detects_real_scroll_offsets() {
    let source = support::dark_text_source(480, 1800);
    let previous = support::crop(&source, 0, 480);
    let current = support::crop(&source, 251, 480);

    let detected = detect_vertical_overlap(&previous, &current, None)
        .expect("dark dense text overlap should be detected");
    assert!(detected.abs_diff(229) <= 1);
}

#[test]
fn dense_text_with_fixed_header_detects_content_overlap() {
    let source = support::dense_text_source(480, 1800);
    let previous = support::fixed_header_frame(&source, 0, 480, 48);
    let current = support::fixed_header_frame(&source, 211, 480, 48);

    let detected = detect_vertical_overlap(&previous, &current, None)
        .expect("fixed header must not hide the text overlap");
    assert!(detected.abs_diff(221) <= 1);
}

#[test]
fn dense_text_with_fixed_header_reconstructs_content_once() {
    let source = support::dense_text_source(320, 1800);
    let frame_height = 420;
    let header_height = 40;
    let starts = [0, 211, 421, 691, 1011, 1370];
    let frames: Vec<RgbaImage> = starts
        .iter()
        .map(|start| support::fixed_header_frame(&source, *start, frame_height, header_height))
        .collect();
    let overlaps: Vec<u32> = frames
        .windows(2)
        .map(|pair| detect_vertical_overlap(&pair[0], &pair[1], None).unwrap())
        .collect();

    let stitched = stitch_vertical(&frames, &overlaps).unwrap();
    let expected = support::crop(
        &source,
        0,
        starts.last().copied().unwrap() + frame_height - header_height,
    );
    assert_eq!(stitched, expected);
}

#[test]
fn uniform_frames_are_rejected_as_ambiguous() {
    let previous = RgbaImage::from_pixel(480, 480, Rgba([245, 245, 245, 255]));
    let current = previous.clone();
    assert!(detect_vertical_overlap(&previous, &current, None).is_none());
    assert!(frames_near_stagnant(&previous, &current));
}

#[test]
fn bottom_detection_requires_two_stagnant_captures() {
    let mut progress = CaptureProgress::default();
    assert_eq!(
        progress.record_measured_with_height(220, None),
        CaptureDecision::AppendMeasured(220)
    );
    assert_eq!(
        progress.record_measured_with_height(221, None),
        CaptureDecision::AppendMeasured(221)
    );
    assert_eq!(progress.record_stagnant(), CaptureDecision::Retry);
    assert_eq!(progress.record_stagnant(), CaptureDecision::ReachedBottom);
}

#[test]
fn near_full_overlap_requires_two_bottom_confirmations() {
    let mut progress = CaptureProgress::default();
    progress.record_measured_with_height(220, None);
    progress.record_measured_with_height(221, None);
    assert_eq!(
        progress.record_measured_with_height(479, Some(480)),
        CaptureDecision::AppendMeasured(479)
    );
    assert_eq!(
        progress.record_measured_with_height(479, Some(480)),
        CaptureDecision::ReachedBottom
    );
}

#[test]
fn valid_scroll_variation_never_triggers_bottom_confirmation() {
    let mut progress = CaptureProgress::default();
    for overlap in [220, 247, 191, 291, 226, 264] {
        assert_eq!(
            progress.record_measured_with_height(overlap, None),
            CaptureDecision::AppendMeasured(overlap)
        );
    }
    assert_eq!(progress.record_stagnant(), CaptureDecision::Retry);
}

#[test]
fn unmatched_captures_use_history_only_with_a_hard_bound() {
    let mut progress = CaptureProgress::default();
    progress.record_measured_with_height(220, None);
    progress.record_measured_with_height(221, None);

    for _ in 0..5 {
        assert!(matches!(
            progress.record_unmatched(480),
            CaptureDecision::AppendEstimated(_)
        ));
    }
    assert_eq!(
        progress.record_unmatched(480),
        CaptureDecision::StopUnreliable
    );
}

#[test]
fn localized_animation_at_bottom_is_stagnant() {
    let source = support::dense_text_source(480, 1200);
    let previous = support::crop(&source, 400, 480);
    let current = support::add_animated_panel(&previous, 0, 40, 65);
    assert!(frames_near_stagnant(&previous, &current));
}

#[test]
fn real_dense_text_scroll_is_not_stagnant() {
    let source = support::dense_text_source(480, 1600);
    let previous = support::crop(&source, 0, 480);
    let current = support::crop(&source, 220, 480);
    assert!(!frames_near_stagnant(&previous, &current));
}

#[test]
fn fixed_header_with_real_scroll_is_not_stagnant() {
    let source = support::dense_text_source(480, 1600);
    let previous = support::fixed_header_frame(&source, 0, 480, 48);
    let current = support::fixed_header_frame(&source, 220, 480, 48);
    assert!(!frames_near_stagnant(&previous, &current));
}

#[test]
fn full_width_animated_strip_at_bottom_is_stagnant() {
    let source = support::dense_text_source(480, 1200);
    let previous = support::crop(&source, 400, 480);
    let current = support::add_animated_strip(&previous, 0, 42, 65);
    assert!(frames_near_stagnant(&previous, &current));
}

#[test]
fn static_sidebar_does_not_hide_real_scroll() {
    let source = support::dense_text_source(480, 1600);
    let previous = support::with_static_sidebar(&support::crop(&source, 0, 480), 150);
    let current = support::with_static_sidebar(&support::crop(&source, 220, 480), 150);
    assert!(!frames_near_stagnant(&previous, &current));
}

#[test]
fn headless_capture_flow_bounds_repeated_unmatched_frames() {
    let source = support::dense_text_source(320, 1600);
    let mut input = vec![
        support::crop(&source, 0, 420),
        support::crop(&source, 180, 420),
        support::crop(&source, 360, 420),
    ];
    for shade in [30, 70, 110, 150, 190, 230] {
        input.push(RgbaImage::from_pixel(
            320,
            420,
            Rgba([shade, shade, shade, 255]),
        ));
    }

    let (frames, overlaps, end) = run_headless_capture(&input);
    assert_eq!(end, HeadlessCaptureEnd::Unreliable);
    assert_eq!(frames.len(), 8, "five bounded estimates should be appended");
    assert_eq!(overlaps.len(), 7);
}
