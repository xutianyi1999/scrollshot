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

fn dense_text_overlap(c: &mut Criterion) {
    let source = support::dense_text_source(480, 2400);
    let previous = support::crop(&source, 0, 480);
    let current = support::crop(&source, 263, 480);

    c.bench_function("dense_text_overlap_480x480", |b| {
        b.iter(|| stitch::detect_vertical_overlap(black_box(&previous), black_box(&current), None))
    });

    c.bench_function("dense_text_overlap_480x480_with_history", |b| {
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

    c.bench_function("dense_text_overlap_1280x720_with_history", |b| {
        b.iter(|| {
            stitch::detect_vertical_overlap(
                black_box(&large_previous),
                black_box(&large_current),
                Some(320.0),
            )
        })
    });
}

criterion_group!(benches, dense_text_overlap);
criterion_main!(benches);
