use image::imageops::{crop_imm, replace};
use image::{GrayImage, Luma, RgbaImage};
use imageproc::contrast::otsu_level;
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
const OVERLAP_VOTE_TOLERANCE: u32 = 1;
const MIN_VOTE_WINDOW_WIDTH: u32 = 48;
const VOTE_WINDOW_RATIO: f32 = 0.6;
const TEXT_PAGE_MIN_BRIGHT_RATIO: f32 = 0.7;
const TEXT_PAGE_MAX_INK_RATIO: f32 = 0.22;
const TEXT_PAGE_MIN_ROW_VARIATION: f32 = 0.015;
const TEXT_BODY_SEARCH_EDGE_RATIO: f32 = 0.05;
const TEXT_BODY_ACTIVE_RATIO: f32 = 0.35;
const TEXT_BODY_MIN_DENSITY: f32 = 0.012;
const TEXT_BODY_MIN_WIDTH_RATIO: f32 = 0.18;
const TEXT_BODY_PADDING_RATIO: f32 = 0.03;

#[derive(Clone, Copy, Debug)]
struct MatchCandidate {
    overlap: u32,
    score: f32,
    alternative_score: f32,
    template_height: u32,
}

#[derive(Clone, Copy, Debug)]
struct OverlapVote {
    overlap: u32,
    votes: usize,
    best_score: f32,
    average_score: f32,
}

#[derive(Clone, Copy, Debug)]
struct HorizontalBand {
    left: u32,
    right: u32,
}

impl HorizontalBand {
    fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }
}

pub fn frames_are_similar(previous: &RgbaImage, current: &RgbaImage) -> bool {
    if previous.dimensions() != current.dimensions() {
        return false;
    }

    sampled_difference(
        previous,
        current,
        0,
        0,
        previous.height(),
        SAMPLE_STEP * 2,
        None,
    ) <= IDENTICAL_THRESHOLD
}

pub fn detect_vertical_overlap(previous: &RgbaImage, current: &RgbaImage) -> Option<u32> {
    if previous.width() != current.width() || previous.height() != current.height() {
        return None;
    }

    let content_height = previous.height();
    let min_overlap =
        ((content_height as f32) * MIN_OVERLAP_RATIO).max(MIN_TEMPLATE_HEIGHT as f32) as u32;
    let max_overlap = ((content_height as f32) * MAX_OVERLAP_RATIO)
        .min(content_height.saturating_sub(1) as f32) as u32;

    if min_overlap > max_overlap {
        return None;
    }

    let focus_band = shared_text_body_band(previous, current);
    let (previous_map, current_map) = overlap_match_maps(previous, current, focus_band);
    let template_heights = candidate_template_heights(min_overlap, max_overlap);
    let mut primary_candidates = template_heights
        .iter()
        .copied()
        .filter_map(|template_height| {
            match_overlap_candidate(
                &previous_map,
                &current_map,
                template_height,
                min_overlap,
                max_overlap,
                None,
            )
        })
        .collect::<Vec<_>>();
    primary_candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.template_height.cmp(&a.template_height))
    });

    let best = *primary_candidates.first()?;
    if best.score < MATCH_SCORE_THRESHOLD {
        return None;
    }

    let local_margin_ok = best.alternative_score.is_nan()
        || (best.score - best.alternative_score) >= LOCAL_CONFIDENCE_DELTA;
    if !local_margin_ok {
        return None;
    }

    let global_alternative = primary_candidates
        .iter()
        .skip(1)
        .find(|candidate| candidate.overlap.abs_diff(best.overlap) >= ALTERNATIVE_GAP);
    if let Some(other) = global_alternative {
        let global_margin_ok = (best.score - other.score) >= GLOBAL_CONFIDENCE_DELTA;
        if !global_margin_ok {
            return None;
        }
    }

    let vote_bands = candidate_vote_bands(previous_map.width());
    let support_votes = ranked_overlap_votes(
        &previous_map,
        &current_map,
        &template_heights,
        &vote_bands,
        min_overlap,
        max_overlap,
    );
    if let Some(top_vote) = support_votes.first()
        && top_vote.votes >= 2
        && top_vote.overlap.abs_diff(best.overlap) > OVERLAP_VOTE_TOLERANCE
    {
        let runner_up_supports_primary = support_votes
            .iter()
            .skip(1)
            .any(|vote| vote.overlap.abs_diff(best.overlap) <= OVERLAP_VOTE_TOLERANCE);
        if !runner_up_supports_primary {
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
        let slice = crop_imm(frame, 0, start_y, frame.width(), frame.height() - start_y).to_image();
        replace(&mut output, &slice, 0, cursor_y as i64);
        cursor_y += slice.height();
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

fn candidate_vote_bands(width: u32) -> Vec<HorizontalBand> {
    let mut bands = Vec::new();
    if width < MIN_VOTE_WINDOW_WIDTH.saturating_mul(2) {
        return bands;
    }

    let window_width = ((width as f32) * VOTE_WINDOW_RATIO).round() as u32;
    let window_width = window_width.clamp(MIN_VOTE_WINDOW_WIDTH, width);
    if window_width >= width {
        return bands;
    }

    let start_positions = [0, (width - window_width) / 2, width - window_width];
    for left in start_positions {
        let band = HorizontalBand {
            left,
            right: left + window_width,
        };
        if !bands
            .iter()
            .any(|existing| existing.left == band.left && existing.right == band.right)
        {
            bands.push(band);
        }
    }

    bands
}

fn ranked_overlap_votes(
    previous: &GrayImage,
    current: &GrayImage,
    template_heights: &[u32],
    vote_bands: &[HorizontalBand],
    min_overlap: u32,
    max_overlap: u32,
) -> Vec<OverlapVote> {
    let mut candidates = Vec::new();
    for band in vote_bands.iter().copied() {
        for template_height in template_heights.iter().copied() {
            if let Some(candidate) = match_overlap_candidate(
                previous,
                current,
                template_height,
                min_overlap,
                max_overlap,
                Some(band),
            ) {
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|a, b| a.overlap.cmp(&b.overlap).then_with(|| b.score.total_cmp(&a.score)));
    let mut votes = Vec::new();
    let mut cluster_start = 0usize;

    while cluster_start < candidates.len() {
        let mut cluster_end = cluster_start + 1;
        let anchor_overlap = candidates[cluster_start].overlap;
        while cluster_end < candidates.len()
            && candidates[cluster_end].overlap.abs_diff(anchor_overlap) <= OVERLAP_VOTE_TOLERANCE
        {
            cluster_end += 1;
        }

        let cluster = &candidates[cluster_start..cluster_end];
        let representative = cluster
            .iter()
            .max_by(|a, b| {
                a.score
                    .total_cmp(&b.score)
                    .then_with(|| a.template_height.cmp(&b.template_height))
            })
            .copied()
            .expect("cluster is non-empty");
        let average_score =
            cluster.iter().map(|candidate| candidate.score).sum::<f32>() / cluster.len() as f32;

        votes.push(OverlapVote {
            overlap: representative.overlap,
            votes: cluster.len(),
            best_score: representative.score,
            average_score,
        });
        cluster_start = cluster_end;
    }

    votes.sort_by(|a, b| {
        b.votes
            .cmp(&a.votes)
            .then_with(|| b.average_score.total_cmp(&a.average_score))
            .then_with(|| b.best_score.total_cmp(&a.best_score))
    });
    votes
}

fn match_overlap_candidate(
    previous: &GrayImage,
    current: &GrayImage,
    template_height: u32,
    min_overlap: u32,
    max_overlap: u32,
    band: Option<HorizontalBand>,
) -> Option<MatchCandidate> {
    let previous = crop_gray_to_band(previous, band);
    let current = crop_gray_to_band(current, band);
    let template = crop_imm(
        &previous,
        0,
        previous.height().checked_sub(template_height)?,
        previous.width(),
        template_height,
    )
    .to_image();
    let search_region = crop_imm(&current, 0, 0, current.width(), max_overlap).to_image();
    let response = match_template(
        &search_region,
        &template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1;
    let usable_overlap = best_y + template_height;
    if !(min_overlap..=max_overlap).contains(&usable_overlap) {
        return None;
    }

    Some(MatchCandidate {
        overlap: usable_overlap,
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

fn overlap_match_maps(
    previous: &RgbaImage,
    current: &RgbaImage,
    band: Option<HorizontalBand>,
) -> (GrayImage, GrayImage) {
    let (previous_map, previous_has_features) = to_feature_map(previous, band);
    let (current_map, current_has_features) = to_feature_map(current, band);

    if previous_has_features && current_has_features {
        (previous_map, current_map)
    } else {
        (to_grayscale(previous), to_grayscale(current))
    }
}

fn to_feature_map(image: &RgbaImage, band: Option<HorizontalBand>) -> (GrayImage, bool) {
    let grayscale = to_grayscale(image);
    let grayscale = crop_gray_to_band(&grayscale, band);
    let gradients = sobel_gradients(&grayscale);
    let max_gradient = gradients.pixels().map(|pixel| pixel[0]).max().unwrap_or(0);

    if max_gradient == 0 {
        return (GrayImage::new(grayscale.width(), grayscale.height()), false);
    }

    (
        GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
            let gradient = gradients.get_pixel(x, y)[0] as f32;
            let scaled = (gradient / max_gradient as f32) * 255.0;
            Luma([scaled.round().clamp(0.0, 255.0) as u8])
        }),
        true,
    )
}

fn sampled_difference(
    previous: &RgbaImage,
    current: &RgbaImage,
    previous_start_y: u32,
    current_start_y: u32,
    height: u32,
    step: u32,
    band: Option<HorizontalBand>,
) -> f32 {
    let mut total = 0f32;
    let mut count = 0u32;
    let band = normalized_band(band, previous.width()).unwrap_or(HorizontalBand {
        left: 0,
        right: previous.width(),
    });

    for y in (0..height).step_by(step as usize) {
        let py = previous_start_y + y;
        let cy = current_start_y + y;
        for x in (band.left..band.right).step_by(step as usize) {
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

fn detect_text_body_band(image: &GrayImage) -> Option<HorizontalBand> {
    let threshold = otsu_threshold(image)?;
    let left = image.width() / 10;
    let right = image.width().saturating_sub(left);
    if left >= right {
        return None;
    }

    let mut total_pixels = 0u32;
    let mut bright_pixels = 0u32;
    let mut ink_pixels = 0u32;
    let mut row_ink = Vec::new();
    let mut binary = GrayImage::new(image.width(), image.height());

    for y in 0..image.height() {
        let mut row_ink_count = 0u32;
        for x in left..right {
            let value = image.get_pixel(x, y)[0];
            total_pixels += 1;
            if value >= threshold {
                bright_pixels += 1;
            } else {
                ink_pixels += 1;
                row_ink_count += 1;
                binary.put_pixel(x, y, Luma([255]));
            }
        }
        row_ink.push(row_ink_count as f32 / (right - left) as f32);
    }

    if total_pixels == 0 {
        return None;
    }

    let bright_ratio = bright_pixels as f32 / total_pixels as f32;
    let ink_ratio = ink_pixels as f32 / total_pixels as f32;
    if bright_ratio < TEXT_PAGE_MIN_BRIGHT_RATIO || ink_ratio > TEXT_PAGE_MAX_INK_RATIO {
        return None;
    }

    let mean_row_ink = row_ink.iter().copied().sum::<f32>() / row_ink.len() as f32;
    let row_variation = row_ink
        .iter()
        .map(|value| {
            let centered = *value - mean_row_ink;
            centered * centered
        })
        .sum::<f32>()
        / row_ink.len() as f32;
    if row_variation.sqrt() < TEXT_PAGE_MIN_ROW_VARIATION {
        return None;
    }

    detect_body_band(&binary)
}

fn otsu_threshold(image: &GrayImage) -> Option<u8> {
    let left = image.width() / 10;
    let right = image.width().saturating_sub(left);
    if left >= right {
        return None;
    }

    let focus = crop_imm(image, left, 0, right - left, image.height()).to_image();
    if focus.width() == 0 || focus.height() == 0 {
        return None;
    }

    Some(otsu_level(&focus))
}

fn crop_gray_to_band(image: &GrayImage, band: Option<HorizontalBand>) -> GrayImage {
    let Some(band) = normalized_band(band, image.width()) else {
        return image.clone();
    };
    crop_imm(image, band.left, 0, band.width(), image.height()).to_image()
}

fn normalized_band(band: Option<HorizontalBand>, width: u32) -> Option<HorizontalBand> {
    let band = band?;
    let left = band.left.min(width);
    let right = band.right.min(width);
    (left < right).then_some(HorizontalBand { left, right })
}

fn shared_text_body_band(
    previous: &RgbaImage,
    current: &RgbaImage,
) -> Option<HorizontalBand> {
    let previous_gray = to_grayscale(previous);
    let current_gray = to_grayscale(current);
    let previous_band = detect_text_body_band(&previous_gray)?;
    let current_band = detect_text_body_band(&current_gray)?;
    let left = previous_band.left.max(current_band.left);
    let right = previous_band.right.min(current_band.right);
    let min_width = minimum_body_band_width(previous.width());
    if right.saturating_sub(left) >= min_width {
        Some(HorizontalBand { left, right })
    } else {
        None
    }
}

fn detect_body_band(binary: &GrayImage) -> Option<HorizontalBand> {
    let width = binary.width();
    let height = binary.height();
    if height == 0 {
        return None;
    }

    let search_margin = ((width as f32) * TEXT_BODY_SEARCH_EDGE_RATIO).round() as u32;
    let search_left = search_margin.min(width);
    let search_right = width.saturating_sub(search_margin);
    if search_left >= search_right {
        return None;
    }

    let mut density = vec![0f32; width as usize];
    for x in search_left..search_right {
        let mut ink = 0u32;
        for y in 0..binary.height() {
            if binary.get_pixel(x, y)[0] > 0 {
                ink += 1;
            }
        }
        density[x as usize] = ink as f32 / height as f32;
    }
    let smoothed = smooth_density(&density, 2);
    let peak = smoothed[search_left as usize..search_right as usize]
        .iter()
        .copied()
        .fold(0.0, f32::max);
    if peak < TEXT_BODY_MIN_DENSITY {
        return None;
    }

    let active_threshold = (peak * TEXT_BODY_ACTIVE_RATIO).max(TEXT_BODY_MIN_DENSITY);
    let min_width = minimum_body_band_width(width);
    let preferred_center = width as f32 / 2.0;
    let mut best_band = None;
    let mut best_score = f32::MIN;
    let mut run_start = None;

    for x in search_left..=search_right {
        let active = x < search_right && smoothed[x as usize] >= active_threshold;
        match (run_start, active) {
            (None, true) => run_start = Some(x),
            (Some(start), false) => {
                let end = x;
                if end.saturating_sub(start) >= min_width {
                    let score =
                        score_body_band(&smoothed, start, end, preferred_center, width as f32);
                    if score > best_score {
                        best_score = score;
                        best_band = Some((start, end));
                    }
                }
                run_start = None;
            }
            _ => {}
        }
    }

    let (start, end) = best_band?;
    let padding = ((width as f32) * TEXT_BODY_PADDING_RATIO).round() as u32;
    let left = start.saturating_sub(padding);
    let right = (end + padding).min(width);
    (left < right).then_some(HorizontalBand { left, right })
}

fn minimum_body_band_width(width: u32) -> u32 {
    ((width as f32) * TEXT_BODY_MIN_WIDTH_RATIO).round() as u32
}

fn smooth_density(values: &[f32], radius: usize) -> Vec<f32> {
    let mut smoothed = vec![0f32; values.len()];
    for (index, out) in smoothed.iter_mut().enumerate() {
        let start = index.saturating_sub(radius);
        let end = (index + radius + 1).min(values.len());
        let window = &values[start..end];
        *out = window.iter().copied().sum::<f32>() / window.len() as f32;
    }
    smoothed
}

fn score_body_band(
    density: &[f32],
    start: u32,
    end: u32,
    preferred_center: f32,
    width: f32,
) -> f32 {
    let mut mass = 0f32;
    for x in start..end {
        mass += density[x as usize];
    }
    let center = (start + end) as f32 / 2.0;
    let center_distance = ((center - preferred_center).abs() / (width / 2.0)).clamp(0.0, 1.0);
    let center_bonus = 1.0 - center_distance * 0.2;
    mass * center_bonus
}

fn pixel_difference(a: [u8; 4], b: [u8; 4]) -> f32 {
    ((a[0] as f32 - b[0] as f32).abs()
        + (a[1] as f32 - b[1] as f32).abs()
        + (a[2] as f32 - b[2] as f32).abs())
        / 3.0
}

#[cfg(test)]
mod tests {
    use super::{detect_vertical_overlap, frames_are_similar, shared_text_body_band, stitch_vertical};
    use image::imageops::crop_imm;
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

        assert_eq!(
            detect_vertical_overlap(&first, &second),
            Some(37)
        );
    }

    #[test]
    fn overlap_detection_handles_large_final_overlap() {
        let source = build_source(48, 100);
        let second = crop(&source, 25, 60);
        let third = crop(&source, 40, 60);

        assert_eq!(
            detect_vertical_overlap(&second, &third),
            Some(45)
        );
    }

    #[test]
    fn overlap_detection_handles_tiny_scroll_steps() {
        let source = build_source(48, 120);
        let first = crop(&source, 0, 100);
        let second = crop(&source, 2, 100);

        assert_eq!(
            detect_vertical_overlap(&first, &second),
            Some(98)
        );
    }

    #[test]
    fn uniform_frames_do_not_report_an_ambiguous_overlap() {
        let first = RgbaImage::from_pixel(48, 120, Rgba([245, 245, 245, 255]));
        let second = RgbaImage::from_pixel(48, 120, Rgba([245, 245, 245, 255]));

        assert_eq!(
            detect_vertical_overlap(&first, &second),
            None
        );
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

        assert_eq!(
            detect_vertical_overlap(&first, &second),
            Some(72)
        );
    }

    #[test]
    fn text_body_band_focuses_on_the_main_document_column() {
        let source = build_sidebar_document_source(120, 220);
        let first = crop(&source, 0, 120);
        let second = crop(&source, 24, 120);
        let band = shared_text_body_band(&first, &second).unwrap();

        assert!(band.left >= 20);
        assert!(band.width() < first.width());
        assert!(band.width() >= 60);
        assert!(band.left < 40 && band.right > 80);
    }

    #[test]
    fn overlap_detection_handles_document_with_sidebar_noise() {
        let source = build_sidebar_document_source(120, 260);
        let first = crop(&source, 0, 120);
        let second = crop(&source, 24, 120);

        assert_eq!(
            detect_vertical_overlap(&first, &second),
            Some(96)
        );
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
        crop_imm(source, 0, start_y, source.width(), height).to_image()
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

    fn build_sidebar_document_source(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([249, 249, 249, 255]));
        let sidebar_end = (width / 5).max(16);
        let body_left = sidebar_end + 10;
        let body_right = width.saturating_sub(12);

        for y in 0..height {
            for x in 0..sidebar_end {
                let shade = if ((y / 18) + (x / 6)) % 2 == 0 { 228 } else { 238 };
                image.put_pixel(x, y, Rgba([shade, shade, shade, 255]));
            }
        }

        for y in (10..height).step_by(12) {
            let ragged_right = body_right.saturating_sub((y % 17) / 3);
            for x in body_left..ragged_right {
                let shade = 28 + ((x + y * 3) % 22) as u8;
                image.put_pixel(x, y, Rgba([shade, shade, shade, 255]));
            }
        }

        for y in (28..height).step_by(56) {
            for line_y in y..(y + 4).min(height) {
                for x in body_left..body_right.saturating_sub(18) {
                    image.put_pixel(x, line_y, Rgba([70, 92, 136, 255]));
                }
            }
        }

        for y in (44..height).step_by(48) {
            for x in body_left.saturating_sub(4)..body_right {
                image.put_pixel(x, y, Rgba([228, 228, 228, 255]));
            }
        }

        image
    }

}
