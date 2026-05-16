use image::imageops::crop_imm;
use image::{GrayImage, Luma, Rgba, RgbaImage};
use imageproc::gradients::sobel_gradients;
use imageproc::template_matching::{MatchTemplateMethod, find_extremes, match_template};

use crate::error::{AppError, AppResult};

const SAMPLE_STEP: u32 = 4;
const IDENTICAL_THRESHOLD: f32 = 1.5;
const MIN_OVERLAP_RATIO: f32 = 0.02;
const MAX_OVERLAP_RATIO: f32 = 0.995;
const MIN_TEMPLATE_HEIGHT: u32 = 8;
const TEMPLATE_HEIGHT_FACTORS: [u32; 5] = [1, 2, 3, 5, 8];
const MATCH_SCORE_THRESHOLD: f32 = 0.94;
const LOCAL_CONFIDENCE_DELTA: f32 = 0.01;
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.005;
const ALTERNATIVE_GAP: u32 = 4;

#[derive(Clone, Copy, Debug)]
struct MatchCandidate {
    overlap: u32,
    score: f32,
    alternative_score: f32,
    template_height: u32,
}

pub fn frames_are_similar(previous: &RgbaImage, current: &RgbaImage) -> bool {
    if previous.dimensions() != current.dimensions() {
        return false;
    }

    sampled_difference(previous, current, 0, 0, previous.height(), SAMPLE_STEP * 2)
        <= IDENTICAL_THRESHOLD
}

pub fn detect_vertical_overlap(previous: &RgbaImage, current: &RgbaImage) -> Option<u32> {
    if previous.width() != current.width() || previous.height() != current.height() {
        return None;
    }

    let height = previous.height();
    let min_overlap = ((height as f32) * MIN_OVERLAP_RATIO).max(MIN_TEMPLATE_HEIGHT as f32) as u32;
    let max_overlap = ((height as f32) * MAX_OVERLAP_RATIO)
        .min((height.saturating_sub(1)) as f32) as u32;

    if min_overlap > max_overlap {
        return None;
    }

    let previous_features = to_feature_map(previous);
    let current_features = to_feature_map(current);
    let search_region =
        crop_imm(&current_features, 0, 0, current_features.width(), max_overlap).to_image();

    let mut candidates = candidate_template_heights(min_overlap, max_overlap)
        .into_iter()
        .filter_map(|template_height| {
            match_overlap_candidate(
                &previous_features,
                &search_region,
                template_height,
                min_overlap,
                max_overlap,
            )
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.template_height.cmp(&a.template_height))
    });

    let best = *candidates.first()?;
    if best.score < MATCH_SCORE_THRESHOLD {
        return None;
    }

    let local_margin_ok = best.alternative_score.is_nan()
        || (best.score - best.alternative_score) >= LOCAL_CONFIDENCE_DELTA;
    if !local_margin_ok {
        return None;
    }

    let global_alternative = candidates
        .iter()
        .skip(1)
        .find(|candidate| candidate.overlap.abs_diff(best.overlap) >= ALTERNATIVE_GAP);
    if let Some(other) = global_alternative {
        let global_margin_ok = (best.score - other.score) >= GLOBAL_CONFIDENCE_DELTA;
        if !global_margin_ok {
            return None;
        }
    }

    Some(best.overlap)
}

pub fn stitch_vertical(frames: &[RgbaImage], overlaps: &[u32]) -> AppResult<RgbaImage> {
    if frames.is_empty() {
        return Err(AppError::Message("No frames were captured".to_string()));
    }
    if frames.len() != overlaps.len() + 1 {
        return Err(AppError::InvalidStitchState);
    }

    let width = frames[0].width();
    let mut total_height = frames[0].height();

    for (frame, overlap) in frames.iter().skip(1).zip(overlaps.iter().copied()) {
        if frame.width() != width || overlap >= frame.height() {
            return Err(AppError::InvalidStitchState);
        }
        total_height += frame.height() - overlap;
    }

    let mut output = RgbaImage::new(width, total_height);
    let mut cursor_y = 0;

    for (index, frame) in frames.iter().enumerate() {
        let start_y = if index == 0 { 0 } else { overlaps[index - 1] };
        for y in start_y..frame.height() {
            for x in 0..frame.width() {
                let pixel: Rgba<u8> = *frame.get_pixel(x, y);
                output.put_pixel(x, cursor_y, pixel);
            }
            cursor_y += 1;
        }
    }

    Ok(output)
}

fn candidate_template_heights(min_overlap: u32, max_overlap: u32) -> Vec<u32> {
    let mut heights = TEMPLATE_HEIGHT_FACTORS
        .iter()
        .map(|factor| min_overlap.saturating_mul(*factor))
        .collect::<Vec<_>>();
    heights.push(MIN_TEMPLATE_HEIGHT.max(min_overlap));
    heights.retain(|height| *height >= min_overlap && *height <= max_overlap);
    heights.sort_unstable();
    heights.dedup();
    heights
}

fn match_overlap_candidate(
    previous: &GrayImage,
    search_region: &GrayImage,
    template_height: u32,
    min_overlap: u32,
    max_overlap: u32,
) -> Option<MatchCandidate> {
    let template = crop_imm(
        previous,
        0,
        previous.height().checked_sub(template_height)?,
        previous.width(),
        template_height,
    )
    .to_image();
    let response = match_template(
        search_region,
        &template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1;
    let overlap = best_y + template_height;

    (min_overlap..=max_overlap)
        .contains(&overlap)
        .then_some(MatchCandidate {
            overlap,
            score: extremes.max_value,
            alternative_score: best_alternative_score(&response, best_y),
            template_height,
        })
}

fn best_alternative_score(
    response: &imageproc::definitions::Image<image::Luma<f32>>,
    best_y: u32,
) -> f32 {
    response
        .enumerate_pixels()
        .filter(|(x, y, _)| *x == 0 && y.abs_diff(best_y) >= ALTERNATIVE_GAP)
        .map(|(_, _, pixel)| pixel[0])
        .max_by(|a, b| a.total_cmp(b))
        .unwrap_or(f32::NAN)
}

fn to_grayscale(image: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y).0;
        let value =
            (pixel[0] as f32 * 0.2126) + (pixel[1] as f32 * 0.7152) + (pixel[2] as f32 * 0.0722);
        image::Luma([value.round().clamp(0.0, 255.0) as u8])
    })
}

fn to_feature_map(image: &RgbaImage) -> GrayImage {
    let grayscale = to_grayscale(image);
    let gradients = sobel_gradients(&grayscale);
    let max_gradient = gradients.pixels().map(|pixel| pixel[0]).max().unwrap_or(0);

    if max_gradient == 0 {
        return GrayImage::new(grayscale.width(), grayscale.height());
    }

    GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
        let gradient = gradients.get_pixel(x, y)[0] as f32;
        let scaled = (gradient / max_gradient as f32) * 255.0;
        Luma([scaled.round().clamp(0.0, 255.0) as u8])
    })
}

fn sampled_difference(
    previous: &RgbaImage,
    current: &RgbaImage,
    previous_start_y: u32,
    current_start_y: u32,
    height: u32,
    step: u32,
) -> f32 {
    let mut total = 0f32;
    let mut count = 0u32;

    for y in (0..height).step_by(step as usize) {
        let py = previous_start_y + y;
        let cy = current_start_y + y;
        for x in (0..previous.width()).step_by(step as usize) {
            let a = previous.get_pixel(x, py).0;
            let b = current.get_pixel(x, cy).0;
            total += pixel_difference(a, b);
            count += 1;
        }
    }

    if count == 0 {
        f32::MAX
    } else {
        total / count as f32
    }
}

fn pixel_difference(a: [u8; 4], b: [u8; 4]) -> f32 {
    ((a[0] as f32 - b[0] as f32).abs()
        + (a[1] as f32 - b[1] as f32).abs()
        + (a[2] as f32 - b[2] as f32).abs())
        / 3.0
}

#[cfg(test)]
mod tests {
    use super::{detect_vertical_overlap, frames_are_similar, stitch_vertical};
    use image::{Rgba, RgbaImage};

    #[test]
    fn duplicate_frames_are_detected() {
        let source = build_source(32, 80);
        let frame = crop(&source, 0, 40);
        assert!(frames_are_similar(&frame, &frame));
    }

    #[test]
    fn overlap_detection_handles_regular_scroll() {
        let source = build_source(48, 140);
        let first = crop(&source, 0, 60);
        let second = crop(&source, 23, 60);

        assert_eq!(detect_vertical_overlap(&first, &second), Some(37));
    }

    #[test]
    fn overlap_detection_handles_large_final_overlap() {
        let source = build_source(48, 100);
        let second = crop(&source, 25, 60);
        let third = crop(&source, 40, 60);

        assert_eq!(detect_vertical_overlap(&second, &third), Some(45));
    }

    #[test]
    fn overlap_detection_handles_tiny_scroll_steps() {
        let source = build_source(48, 120);
        let first = crop(&source, 0, 100);
        let second = crop(&source, 2, 100);

        assert_eq!(detect_vertical_overlap(&first, &second), Some(98));
    }

    #[test]
    fn stitching_rebuilds_the_original_image() {
        let source = build_source(40, 105);
        let first = crop(&source, 0, 50);
        let second = crop(&source, 20, 50);
        let third = crop(&source, 55, 50);
        let overlaps = vec![
            detect_vertical_overlap(&first, &second).unwrap(),
            detect_vertical_overlap(&second, &third).unwrap(),
        ];

        let stitched = stitch_vertical(&[first, second, third], &overlaps).unwrap();
        assert_eq!(stitched, source);
    }

    #[test]
    fn overlap_detection_handles_low_texture_document_like_content() {
        let source = build_document_like_source(64, 180);
        let first = crop(&source, 0, 90);
        let second = crop(&source, 18, 90);

        assert_eq!(detect_vertical_overlap(&first, &second), Some(72));
    }

    fn build_source(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 17 + y * 13) % 251) as u8;
                let g = ((x * 7 + y * 19) % 251) as u8;
                let b = ((x * 23 + y * 5) % 251) as u8;
                image.put_pixel(x, y, image::Rgba([r, g, b, 255]));
            }
        }
        image
    }

    fn crop(source: &RgbaImage, start_y: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(source.width(), height);
        for y in 0..height {
            for x in 0..source.width() {
                image.put_pixel(x, y, *source.get_pixel(x, start_y + y));
            }
        }
        image
    }

    fn build_document_like_source(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([248, 248, 248, 255]));

        for y in (8..height).step_by(14) {
            for x in 6..width.saturating_sub(6) {
                let shade = 40 + ((x + y) % 30) as u8;
                image.put_pixel(x, y, Rgba([shade, shade, shade, 255]));
            }
        }

        for y in (20..height).step_by(42) {
            for line_y in y..(y + 5).min(height) {
                for x in 10..width.saturating_sub(10) {
                    image.put_pixel(x, line_y, Rgba([90, 120, 180, 255]));
                }
            }
        }

        for y in (35..height).step_by(56) {
            for x in 0..width {
                image.put_pixel(x, y, Rgba([225, 225, 225, 255]));
            }
        }

        image
    }
}
