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

#[path = "../tests/support/mod.rs"]
mod support;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn stitch_benchmarks(c: &mut Criterion) {
    let source = support::dense_text_source(480, 2400);
    let previous = support::crop(&source, 0, 480);
    let current = support::crop(&source, 263, 480);

    c.bench_function("dense_text_overlap_480x480", |b| {
        b.iter(|| stitch::detect_vertical_overlap(black_box(&previous), black_box(&current), None))
    });

    c.bench_function("dense_text_overlap_480x480_with_stale_history_hint", |b| {
        b.iter(|| {
            stitch::detect_vertical_overlap(black_box(&previous), black_box(&current), Some(217.0))
        })
    });

    let large_source = support::dense_text_source(1280, 2400);
    let large_previous = support::crop(&large_source, 0, 720);
    let large_current = support::crop(&large_source, 400, 720);

    c.bench_function("dense_text_overlap_1280x720", |b| {
        b.iter(|| {
            stitch::detect_vertical_overlap(
                black_box(&large_previous),
                black_box(&large_current),
                None,
            )
        })
    });

    c.bench_function("dense_text_overlap_1280x720_with_stale_history_hint", |b| {
        b.iter(|| {
            stitch::detect_vertical_overlap(
                black_box(&large_previous),
                black_box(&large_current),
                Some(320.0),
            )
        })
    });
    let low_overlap_previous = support::crop(&large_source, 100, 720);
    let low_overlap_current = support::crop(&large_source, 765, 720);
    c.bench_function("dense_text_low_overlap_1280x720", |b| {
        b.iter(|| {
            stitch::detect_vertical_overlap(
                black_box(&low_overlap_previous),
                black_box(&low_overlap_current),
                None,
            )
        })
    });

    let starts = [0, 173, 421, 612, 903, 1177, 1459, 1772];
    let variable_frames: Vec<_> = starts
        .iter()
        .map(|start| support::crop(&large_source, *start, 520))
        .collect();
    c.bench_function("dense_text_variable_scroll_sequence", |b| {
        b.iter(|| {
            let matched = variable_frames
                .windows(2)
                .filter(|pair| {
                    stitch::detect_vertical_overlap(black_box(&pair[0]), black_box(&pair[1]), None)
                        .is_some()
                })
                .count();
            black_box(matched)
        })
    });

    let header_frames: Vec<_> = starts
        .iter()
        .map(|start| support::fixed_header_frame(&large_source, *start, 520, 48))
        .collect();
    let header_overlaps: Vec<_> = header_frames
        .windows(2)
        .map(|pair| stitch::detect_vertical_overlap(&pair[0], &pair[1], None).unwrap())
        .collect();
    c.bench_function("fixed_header_stitch_1280x520_x8", |b| {
        b.iter(|| stitch::stitch_vertical(black_box(&header_frames), black_box(&header_overlaps)))
    });
}

criterion_group!(benches, stitch_benchmarks);
criterion_main!(benches);
