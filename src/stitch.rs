use image::imageops::{self, FilterType, crop_imm, replace};
use image::{GrayImage, Luma, RgbaImage};
use imageproc::contrast::{ThresholdType, otsu_level, threshold};
use imageproc::gradients::sobel_gradients;
use imageproc::template_matching::{MatchTemplateMethod, find_extremes, match_template};

use average::Variance;
use rayon::prelude::*;

use crate::error::{AppError, AppResult};

const SAMPLE_STEP: u32 = 2;
// Very small overlaps admit accidental matches in repetitive page content.
// Treat them as a failed capture instead; the caller will retry with a fresh,
// slower scroll rather than committing a bad seam.
const MIN_OVERLAP_RATIO: f32 = 0.05;
const MAX_OVERLAP_RATIO: f32 = 0.995;
const MIN_TEMPLATE_HEIGHT: u32 = 12;
const TEMPLATE_HEIGHT_FACTORS: [u32; 5] = [1, 2, 3, 5, 8];
const MATCH_SCORE_THRESHOLD: f32 = 0.75;
const LOCAL_CONFIDENCE_DELTA: f32 = 0.005;
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.002;
const ALTERNATIVE_GAP: u32 = 4;
const MAX_PIXEL_DIFFERENCE: f32 = 15.0;
// Use a shared Sobel scale for both frames. Per-frame adaptive normalization
// makes identical pixels receive different feature values when one viewport
// contains a photo or illustration and the next does not.
const FEATURE_GRADIENT_NORMALIZER: f32 = 1024.0;
// Text occupies a small fraction of a white page, so raw RGB averages can
// make a one-line misalignment look deceptively close. Compare the Sobel
// feature maps across the whole proposed overlap before committing a seam.
const MAX_FEATURE_DIFFERENCE: f32 = 0.25;
const PREDICTED_SEARCH_MIN_RADIUS: u32 = 24;
const PREDICTED_SEARCH_RADIUS_RATIO: f32 = 0.15;
const COARSE_SEARCH_MAX_HEIGHT: u32 = 180;
const STATIC_EDGE_MAX_RATIO: f32 = 0.12;
// A few coincidentally blank or repetitive rows near a table/image edge are
// not a fixed browser chrome. Requiring a meaningful run prevents pairwise
// edge cropping from changing seam coordinates for ordinary document content.
const STATIC_EDGE_MIN_ROWS: u32 = 16;
const STATIC_EDGE_MAX_DIFFERENCE: f32 = 1.0;
const STATIC_EDGE_MIN_CONTRAST: f32 = 6.0;
const STATIC_EDGE_MIN_DARK_RATIO: f32 = 0.01;
const STATIC_EDGE_DARK_LUMA: f32 = 245.0;
const STAGNANT_AXIS_MIN_RATIO: f32 = 0.85;
const STAGNANT_AXIS_MAX_DIFFERENCE: f32 = 2.0;
// Scrolled sparse content (tall blank table cells) can satisfy the stagnant
// axis heuristic even though the document moved. A shifted alignment that
// beats the at-rest alignment by this margin proves real scroll progress.
const SCROLL_PROGRESS_MIN_REST_DIFFERENCE: f32 = 4.0;

const TEXT_PAGE_MIN_BRIGHT_RATIO: f32 = 0.7;
const TEXT_PAGE_MAX_INK_RATIO: f32 = 0.22;
const TEXT_PAGE_MIN_ROW_VARIATION: f32 = 0.015;
const TEXT_BODY_SEARCH_EDGE_RATIO: f32 = 0.05;
const TEXT_BODY_ACTIVE_RATIO: f32 = 0.35;
const TEXT_BODY_MIN_DENSITY: f32 = 0.012;
const TEXT_BODY_MIN_WIDTH_RATIO: f32 = 0.18;
const TEXT_BODY_PADDING_RATIO: f32 = 0.03;
const SCROLLBAR_MARGIN_RATIO: f32 = 0.012;
const SCROLLBAR_MARGIN_MAX: u32 = 24;

#[derive(Clone, Copy, Debug)]
struct MatchCandidate {
    overlap: u32,
    score: f32,
    alternative_score: f32,
    template_height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HorizontalBand {
    left: u32,
    right: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StaticEdges {
    top: u32,
    bottom: u32,
}

impl HorizontalBand {
    fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }
}

/// Check whether two consecutive frames are nearly identical (relaxed
/// threshold) — used as a safety net for noisy stuck-at-bottom detection.
pub fn frames_near_stagnant(prev: &RgbaImage, curr: &RgbaImage) -> bool {
    if prev.dimensions() != curr.dimensions() {
        return false;
    }
    let edges = detect_static_edges(&[prev, curr]);
    let start_y = edges.top;
    let height = prev
        .height()
        .saturating_sub(edges.top.saturating_add(edges.bottom));
    if height == 0 {
        return false;
    }

    // A small animated widget can change every row or column while the
    // document itself remains stationary. Requiring one stable axis avoids
    // treating that local animation as continued scrolling.
    stable_row_ratio(prev, curr, start_y, height) >= STAGNANT_AXIS_MIN_RATIO
        || stable_column_ratio(prev, curr, start_y, height) >= STAGNANT_AXIS_MIN_RATIO
}

/// Check whether a verified overlap represents real scroll progress rather
/// than a coincidental match on stationary frames. Sparse content such as
/// tall blank table cells can trip the stagnant-axis heuristic while the
/// document scrolls; identical frames at a real bottom (or perfectly
/// periodic content) align equally well at rest, so the shifted alignment
/// must beat the at-rest alignment by a meaningful margin.
pub fn scroll_progress_evident(previous: &RgbaImage, current: &RgbaImage, overlap: u32) -> bool {
    if previous.dimensions() != current.dimensions() || overlap >= previous.height() {
        return false;
    }

    let shifted = sampled_difference(
        previous,
        current,
        previous.height() - overlap,
        0,
        overlap,
        SAMPLE_STEP,
        None,
    );
    if shifted > MAX_PIXEL_DIFFERENCE {
        return false;
    }

    let at_rest = sampled_difference(previous, current, 0, 0, current.height(), SAMPLE_STEP, None);
    at_rest > SCROLL_PROGRESS_MIN_REST_DIFFERENCE
}

pub fn detect_vertical_overlap(
    previous: &RgbaImage,
    current: &RgbaImage,
    expected_overlap: Option<f32>,
) -> Option<u32> {
    // Retained for API compatibility. Scroll history used to bias this
    // decision, but it can be stale after smooth or variable scrolling and
    // must never affect a seam that will be committed to the output.
    let _ = expected_overlap;
    if previous.dimensions() != current.dimensions() {
        return None;
    }
    let edges = detect_static_edges(&[previous, current]);
    if edges == StaticEdges::default() {
        return detect_overlap_inner(previous, current);
    }
    let previous = crop_static_edges(previous, edges);
    let current = crop_static_edges(current, edges);
    detect_overlap_inner(&previous, &current)
}

fn detect_overlap_inner(previous: &RgbaImage, current: &RgbaImage) -> Option<u32> {
    if previous.width() != current.width() || previous.height() != current.height() {
        return None;
    }

    let h = previous.height() as f32;
    let min_overlap = (h * MIN_OVERLAP_RATIO).max(MIN_TEMPLATE_HEIGHT as f32) as u32;
    let max_overlap = (h * MAX_OVERLAP_RATIO).min(h - 1.0) as u32;

    if min_overlap > max_overlap {
        return None;
    }

    let (previous_gray, current_gray) = rayon::join(
        || imageops::grayscale(previous),
        || imageops::grayscale(current),
    );

    let scrollbar_margin =
        ((previous.width() as f32 * SCROLLBAR_MARGIN_RATIO) as u32).min(SCROLLBAR_MARGIN_MAX);
    let scrollbar_safe_right = previous.width().saturating_sub(scrollbar_margin);
    let default_band = (scrollbar_safe_right > 0).then_some(HorizontalBand {
        left: 0,
        right: scrollbar_safe_right,
    });
    let focus_band = detect_text_body_band(&previous_gray)
        .map(|band| {
            let right = band.right.min(scrollbar_safe_right);
            HorizontalBand {
                left: band.left,
                right,
            }
        })
        .filter(|band| band.left < band.right)
        .or(default_band);

    let focus_band_crop = focus_band.and_then(|band| normalized_band(Some(band), previous.width()));
    let ((previous_map, previous_has_features), (current_map, current_has_features)) = rayon::join(
        || to_feature_map_from_gray(&previous_gray, focus_band_crop),
        || to_feature_map_from_gray(&current_gray, focus_band_crop),
    );

    let has_feature_maps = previous_has_features && current_has_features;
    let (previous_map, current_map) = if has_feature_maps {
        (previous_map, current_map)
    } else {
        (previous_gray.clone(), current_gray.clone())
    };

    if let Some(overlap) = detect_overlap_from_maps(
        previous,
        current,
        &previous_map,
        &current_map,
        has_feature_maps,
        min_overlap,
        max_overlap,
        focus_band_crop,
    ) {
        return Some(overlap);
    }

    // The narrow document band avoids unrelated UI during the common text
    // case. If it cannot verify a seam, retry over the full content area so
    // images, tables, code blocks, and other mixed layouts retain their own
    // alignment evidence.
    let full_band = default_band.and_then(|band| normalized_band(Some(band), previous.width()));
    if full_band == focus_band_crop {
        return None;
    }
    let ((previous_map, previous_has_features), (current_map, current_has_features)) = rayon::join(
        || to_feature_map_from_gray(&previous_gray, full_band),
        || to_feature_map_from_gray(&current_gray, full_band),
    );
    let has_feature_maps = previous_has_features && current_has_features;
    let (previous_map, current_map) = if has_feature_maps {
        (previous_map, current_map)
    } else {
        (
            crop_gray_to_band(&previous_gray, full_band),
            crop_gray_to_band(&current_gray, full_band),
        )
    };

    detect_overlap_from_maps(
        previous,
        current,
        &previous_map,
        &current_map,
        has_feature_maps,
        min_overlap,
        max_overlap,
        full_band,
    )
}

fn detect_overlap_from_maps(
    previous: &RgbaImage,
    current: &RgbaImage,
    previous_map: &GrayImage,
    current_map: &GrayImage,
    has_feature_maps: bool,
    min_overlap: u32,
    max_overlap: u32,
    focus_band: Option<HorizontalBand>,
) -> Option<u32> {
    let template_heights = candidate_template_heights(min_overlap, max_overlap);
    // Derive the search window from the two images being matched, never from
    // a previous wheel distance. This keeps the fast path responsive without
    // allowing stale history to move a seam.
    let predicted_overlap =
        coarse_overlap_prediction(&previous_map, &current_map, min_overlap, max_overlap);
    let mut primary_candidates = collect_match_candidates(
        previous_map,
        current_map,
        &template_heights,
        min_overlap,
        max_overlap,
        predicted_overlap,
    );

    if let Some(overlap) = select_overlap(
        &mut primary_candidates,
        previous,
        current,
        previous_map,
        current_map,
        has_feature_maps,
        focus_band,
    ) {
        return Some(overlap);
    }

    // A coarse image can be ambiguous for a highly repetitive page. It is an
    // optimization only: fall back to the complete range before declaring the
    // frame unmatched.
    if predicted_overlap.is_some() {
        primary_candidates = collect_match_candidates(
            previous_map,
            current_map,
            &template_heights,
            min_overlap,
            max_overlap,
            None,
        );
        return select_overlap(
            &mut primary_candidates,
            previous,
            current,
            previous_map,
            current_map,
            has_feature_maps,
            focus_band,
        );
    }

    None
}

fn collect_match_candidates(
    previous_map: &GrayImage,
    current_map: &GrayImage,
    template_heights: &[u32],
    min_overlap: u32,
    max_overlap: u32,
    expected_overlap: Option<f32>,
) -> Vec<MatchCandidate> {
    template_heights
        .par_iter()
        .copied()
        .filter_map(|template_height| {
            match_overlap_candidate(
                previous_map,
                current_map,
                template_height,
                min_overlap,
                max_overlap,
                expected_overlap,
            )
        })
        .collect()
}

fn coarse_overlap_prediction(
    previous_map: &GrayImage,
    current_map: &GrayImage,
    min_overlap: u32,
    max_overlap: u32,
) -> Option<f32> {
    let scale = previous_map.height().div_ceil(COARSE_SEARCH_MAX_HEIGHT);
    if scale <= 1 {
        return None;
    }

    let target_width = previous_map.width().div_ceil(scale).max(1);
    let target_height = previous_map.height().div_ceil(scale).max(1);
    let previous_small = imageops::resize(
        previous_map,
        target_width,
        target_height,
        FilterType::Nearest,
    );
    let current_small = imageops::resize(
        current_map,
        target_width,
        target_height,
        FilterType::Nearest,
    );
    let min_overlap = min_overlap.div_ceil(scale);
    let max_overlap = max_overlap / scale;
    if min_overlap > max_overlap {
        return None;
    }

    let template_heights = candidate_template_heights(min_overlap, max_overlap);
    let mut candidates = collect_match_candidates(
        &previous_small,
        &current_small,
        &template_heights,
        min_overlap,
        max_overlap,
        None,
    );
    let best = select_match_candidate(&mut candidates)?;
    Some(best.overlap as f32 * previous_map.height() as f32 / target_height as f32)
}

fn select_overlap(
    primary_candidates: &mut [MatchCandidate],
    previous: &RgbaImage,
    current: &RgbaImage,
    previous_map: &GrayImage,
    current_map: &GrayImage,
    has_feature_maps: bool,
    focus_band: Option<HorizontalBand>,
) -> Option<u32> {
    let best = select_match_candidate(primary_candidates)?;

    if has_feature_maps
        && sampled_feature_difference(
            previous_map,
            current_map,
            previous_map.height() - best.overlap,
            0,
            best.overlap,
            SAMPLE_STEP,
        ) > MAX_FEATURE_DIFFERENCE
    {
        return None;
    }

    let pixel_diff = sampled_difference(
        previous,
        current,
        previous.height() - best.overlap,
        0,
        best.overlap,
        SAMPLE_STEP,
        focus_band,
    );
    if pixel_diff > MAX_PIXEL_DIFFERENCE {
        return None;
    }

    Some(best.overlap)
}

fn sampled_feature_difference(
    previous: &GrayImage,
    current: &GrayImage,
    previous_start_y: u32,
    current_start_y: u32,
    height: u32,
    step: u32,
) -> f32 {
    if previous.dimensions() != current.dimensions() {
        return f32::MAX;
    }

    let mut total_difference = 0f32;
    let mut total_energy = 0f32;
    for y in (0..height).step_by(step as usize) {
        let previous_y = previous_start_y + y;
        let current_y = current_start_y + y;
        for x in (0..previous.width()).step_by(step as usize) {
            let left = previous.get_pixel(x, previous_y)[0] as f32;
            let right = current.get_pixel(x, current_y)[0] as f32;
            total_difference += (left - right).abs();
            total_energy += left.max(right);
        }
    }

    if total_energy == 0.0 {
        f32::MAX
    } else {
        total_difference / total_energy
    }
}

fn select_match_candidate(primary_candidates: &mut [MatchCandidate]) -> Option<MatchCandidate> {
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

    if let Some(other) = primary_candidates
        .iter()
        .skip(1)
        .find(|candidate| candidate.overlap.abs_diff(best.overlap) >= ALTERNATIVE_GAP)
        && (best.score - other.score) < GLOBAL_CONFIDENCE_DELTA
    {
        return None;
    }

    Some(best)
}

pub fn stitch_vertical(frames: &[RgbaImage], overlaps: &[u32]) -> AppResult<RgbaImage> {
    if frames.is_empty() {
        return Err(AppError::Message("No frames were captured".to_string()));
    }
    if frames.len() != overlaps.len() + 1 {
        return Err(AppError::InvalidStitchState);
    }

    let frame_refs: Vec<&RgbaImage> = frames.iter().collect();
    let edges = detect_static_edges(&frame_refs);
    let cropped_frames = (edges != StaticEdges::default()).then(|| {
        frames
            .iter()
            .map(|frame| crop_static_edges(frame, edges))
            .collect::<Vec<_>>()
    });
    let frames = cropped_frames.as_deref().unwrap_or(frames);
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

fn detect_static_edges(frames: &[&RgbaImage]) -> StaticEdges {
    if frames.len() < 2 {
        return StaticEdges::default();
    }

    let min_height = frames.iter().map(|frame| frame.height()).min().unwrap_or(0);
    let max_rows = ((min_height as f32 * STATIC_EDGE_MAX_RATIO).floor() as u32)
        .max(STATIC_EDGE_MIN_ROWS)
        .min(min_height / 2);

    let top = count_static_edge_rows(frames, max_rows, false);
    let bottom = count_static_edge_rows(frames, max_rows, true);
    StaticEdges {
        top: (top >= STATIC_EDGE_MIN_ROWS && static_edge_has_content(frames, top, false))
            .then_some(top)
            .unwrap_or(0),
        bottom: (bottom >= STATIC_EDGE_MIN_ROWS && static_edge_has_content(frames, bottom, true))
            .then_some(bottom)
            .unwrap_or(0),
    }
}

fn static_edge_has_content(frames: &[&RgbaImage], height: u32, from_bottom: bool) -> bool {
    let reference = frames[0];
    let mut total_luminance = 0f32;
    let mut total_squared_luminance = 0f32;
    let mut dark_pixels = 0u32;
    let mut count = 0u32;

    for offset_y in (0..height).step_by(SAMPLE_STEP as usize) {
        let y = if from_bottom {
            reference.height() - 1 - offset_y
        } else {
            offset_y
        };
        for x in (0..reference.width()).step_by(SAMPLE_STEP as usize) {
            let pixel = reference.get_pixel(x, y).0;
            let luminance = pixel[..3]
                .iter()
                .map(|channel| *channel as f32)
                .sum::<f32>()
                / 3.0;
            total_luminance += luminance;
            total_squared_luminance += luminance * luminance;
            if luminance < STATIC_EDGE_DARK_LUMA {
                dark_pixels += 1;
            }
            count += 1;
        }
    }

    if count == 0 {
        return false;
    }

    let mean = total_luminance / count as f32;
    let variance = (total_squared_luminance / count as f32 - mean * mean).max(0.0);
    variance.sqrt() >= STATIC_EDGE_MIN_CONTRAST
        || dark_pixels as f32 / count as f32 >= STATIC_EDGE_MIN_DARK_RATIO
}

fn count_static_edge_rows(frames: &[&RgbaImage], max_rows: u32, from_bottom: bool) -> u32 {
    let reference = frames[0];
    let mut count = 0;
    for offset in 0..max_rows {
        let reference_y = if from_bottom {
            reference.height() - 1 - offset
        } else {
            offset
        };
        let unchanged = frames.iter().skip(1).all(|frame| {
            let y = if from_bottom {
                frame.height() - 1 - offset
            } else {
                offset
            };
            sampled_row_difference(reference, frame, reference_y, y) <= STATIC_EDGE_MAX_DIFFERENCE
        });
        if !unchanged {
            break;
        }
        count += 1;
    }
    count
}

fn sampled_row_difference(
    previous: &RgbaImage,
    current: &RgbaImage,
    previous_y: u32,
    current_y: u32,
) -> f32 {
    if previous.width() != current.width() {
        return f32::MAX;
    }

    let mut total = 0f32;
    let mut count = 0u32;
    for x in (0..previous.width()).step_by(SAMPLE_STEP as usize) {
        let a = previous.get_pixel(x, previous_y).0;
        let b = current.get_pixel(x, current_y).0;
        total += a
            .iter()
            .zip(b.iter())
            .take(3)
            .map(|(left, right)| (*left as f32 - *right as f32).abs())
            .sum::<f32>()
            / 3.0;
        count += 1;
    }

    if count == 0 {
        f32::MAX
    } else {
        total / count as f32
    }
}

fn stable_row_ratio(previous: &RgbaImage, current: &RgbaImage, start_y: u32, height: u32) -> f32 {
    let mut stable = 0u32;
    let mut sampled = 0u32;
    for y in (start_y..start_y + height).step_by(SAMPLE_STEP as usize) {
        if sampled_row_difference(previous, current, y, y) <= STAGNANT_AXIS_MAX_DIFFERENCE {
            stable += 1;
        }
        sampled += 1;
    }
    if sampled == 0 {
        0.0
    } else {
        stable as f32 / sampled as f32
    }
}

fn stable_column_ratio(
    previous: &RgbaImage,
    current: &RgbaImage,
    start_y: u32,
    height: u32,
) -> f32 {
    let mut stable = 0u32;
    let mut sampled = 0u32;
    for x in (0..previous.width()).step_by(SAMPLE_STEP as usize) {
        if sampled_column_difference(previous, current, x, start_y, height)
            <= STAGNANT_AXIS_MAX_DIFFERENCE
        {
            stable += 1;
        }
        sampled += 1;
    }
    if sampled == 0 {
        0.0
    } else {
        stable as f32 / sampled as f32
    }
}

fn sampled_column_difference(
    previous: &RgbaImage,
    current: &RgbaImage,
    x: u32,
    start_y: u32,
    height: u32,
) -> f32 {
    let mut total = 0f32;
    let mut count = 0u32;
    for y in (start_y..start_y + height).step_by(SAMPLE_STEP as usize) {
        let a = previous.get_pixel(x, y).0;
        let b = current.get_pixel(x, y).0;
        total += a
            .iter()
            .zip(b.iter())
            .take(3)
            .map(|(left, right)| (*left as f32 - *right as f32).abs())
            .sum::<f32>()
            / 3.0;
        count += 1;
    }
    if count == 0 {
        f32::MAX
    } else {
        total / count as f32
    }
}

fn crop_static_edges(image: &RgbaImage, edges: StaticEdges) -> RgbaImage {
    let height = image.height().saturating_sub(edges.top + edges.bottom);
    crop_imm(
        image,
        0,
        edges.top.min(image.height()),
        image.width(),
        height,
    )
    .to_image()
}

fn candidate_template_heights(min_overlap: u32, max_overlap: u32) -> Vec<u32> {
    let mut heights: Vec<u32> = TEMPLATE_HEIGHT_FACTORS
        .iter()
        .map(|factor| min_overlap.saturating_mul(*factor))
        .filter(|h| *h <= max_overlap)
        .collect();
    let min_height = MIN_TEMPLATE_HEIGHT.max(min_overlap);
    if min_height <= max_overlap && !heights.contains(&min_height) {
        heights.push(min_height);
    }
    heights.sort_unstable();
    heights
}
fn match_overlap_candidate(
    previous: &GrayImage,
    current: &GrayImage,
    template_height: u32,
    min_overlap: u32,
    max_overlap: u32,
    expected_overlap: Option<f32>,
) -> Option<MatchCandidate> {
    let template = crop_imm(
        previous,
        0,
        previous.height().checked_sub(template_height)?,
        previous.width(),
        template_height,
    )
    .to_image();

    let (search_start_y, search_end_y) =
        match_search_range(template_height, min_overlap, max_overlap, expected_overlap)?;
    let search_height = search_end_y
        .checked_sub(search_start_y)?
        .checked_add(template_height)?;
    let search_region =
        crop_imm(current, 0, search_start_y, current.width(), search_height).to_image();

    let response = match_template(
        &search_region,
        &template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1;

    let (refined_y, refined_score) = refine_template_match(&response, best_y);
    let refined_overlap = search_start_y + refined_y.round() as u32 + template_height;
    if !(min_overlap..=max_overlap).contains(&refined_overlap) {
        return None;
    }

    Some(MatchCandidate {
        overlap: refined_overlap,
        score: refined_score,
        alternative_score: best_alternative_score(&response, best_y),
        template_height,
    })
}

fn match_search_range(
    template_height: u32,
    min_overlap: u32,
    max_overlap: u32,
    expected_overlap: Option<f32>,
) -> Option<(u32, u32)> {
    let min_y = min_overlap.saturating_sub(template_height);
    let max_y = max_overlap.checked_sub(template_height)?;
    if min_y > max_y {
        return None;
    }

    let Some(expected_overlap) = expected_overlap else {
        return Some((min_y, max_y));
    };

    let expected = expected_overlap.max(0.0).round() as u32;
    let predicted_y = expected.saturating_sub(template_height).clamp(min_y, max_y);
    let radius = ((expected as f32 * PREDICTED_SEARCH_RADIUS_RATIO).round() as u32)
        .max(PREDICTED_SEARCH_MIN_RADIUS);
    Some((
        predicted_y.saturating_sub(radius).max(min_y),
        predicted_y.saturating_add(radius).min(max_y),
    ))
}

fn refine_template_match(
    response: &imageproc::definitions::Image<image::Luma<f32>>,
    best_y: u32,
) -> (f32, f32) {
    if best_y == 0 || best_y + 1 >= response.height() {
        return (best_y as f32, response.get_pixel(0, best_y)[0]);
    }

    let s0 = response.get_pixel(0, best_y - 1)[0];
    let s1 = response.get_pixel(0, best_y)[0];
    let s2 = response.get_pixel(0, best_y + 1)[0];

    let peak_score = s0.max(s1).max(s2);
    (best_y as f32, peak_score)
}

fn best_alternative_score(
    response: &imageproc::definitions::Image<image::Luma<f32>>,
    best_y: u32,
) -> f32 {
    (0..response.height())
        .filter(|y| y.abs_diff(best_y) >= ALTERNATIVE_GAP)
        .filter_map(|y| {
            let v = response.get_pixel(0, y)[0];
            v.is_finite().then_some(v)
        })
        .max_by(|a, b| a.total_cmp(b))
        .unwrap_or(f32::NAN)
}

fn to_feature_map_from_gray(
    grayscale: &GrayImage,
    band: Option<HorizontalBand>,
) -> (GrayImage, bool) {
    let grayscale = crop_gray_to_band(grayscale, band);
    let gradients = sobel_gradients(&grayscale);

    let mut has_gradient = false;
    for p in gradients.pixels() {
        let v = p[0];
        has_gradient |= v > 0;
    }

    if !has_gradient {
        let blank = GrayImage::new(grayscale.width(), grayscale.height());
        return (blank, false);
    }

    (
        GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
            let gradient = gradients.get_pixel(x, y)[0] as f32;
            let scaled = (gradient / FEATURE_GRADIENT_NORMALIZER) * 255.0;
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
            total += a
                .iter()
                .zip(b.iter())
                .take(3)
                .map(|(x, y)| (*x as f32 - *y as f32).abs())
                .sum::<f32>()
                / 3.0;
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
    let left = image.width() / 10;
    let right = image.width().saturating_sub(left);
    if left >= right {
        return None;
    }

    let focus = crop_imm(image, left, 0, right - left, image.height()).to_image();
    if focus.width() == 0 || focus.height() == 0 {
        return None;
    }

    let level = otsu_level(&focus);
    let focus_binary = threshold(&focus, level, ThresholdType::BinaryInverted);

    let mut row_ink = Vec::new();
    for y in 0..focus_binary.height() {
        let count = (0..focus_binary.width())
            .filter(|x| focus_binary.get_pixel(*x, y)[0] > 0)
            .count() as u32;
        row_ink.push(count as f32 / focus_binary.width() as f32);
    }

    let ink_ratio = row_ink.iter().sum::<f32>() / row_ink.len() as f32;
    if 1.0 - ink_ratio < TEXT_PAGE_MIN_BRIGHT_RATIO || ink_ratio > TEXT_PAGE_MAX_INK_RATIO {
        return None;
    }

    let row_stats: Variance = row_ink.iter().map(|v| *v as f64).collect();
    let row_stddev = row_stats.sample_variance().sqrt() as f32;
    if row_stddev < TEXT_PAGE_MIN_ROW_VARIATION {
        return None;
    }

    let mut binary = GrayImage::new(image.width(), image.height());
    replace(&mut binary, &focus_binary, left as i64, 0);
    detect_body_band(&binary)
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

    let mut ink_count = vec![0u32; width as usize];
    for y in 0..height {
        for x in search_left..search_right {
            if binary.get_pixel(x, y)[0] > 0 {
                ink_count[x as usize] += 1;
            }
        }
    }
    let height_f = height as f32;
    let mut density = vec![0f32; width as usize];
    for x in search_left..search_right {
        density[x as usize] = ink_count[x as usize] as f32 / height_f;
    }
    let smoothed = smooth_density(&density, 2);
    let peak = smoothed[search_left as usize..search_right as usize]
        .iter()
        .copied()
        .max_by(|a, b| a.total_cmp(b))
        .unwrap_or(0.0);
    if peak < TEXT_BODY_MIN_DENSITY {
        return None;
    }

    let active_threshold = (peak * TEXT_BODY_ACTIVE_RATIO).max(TEXT_BODY_MIN_DENSITY);
    let min_width = ((width as f32) * TEXT_BODY_MIN_WIDTH_RATIO).round() as u32;
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

fn smooth_density(values: &[f32], radius: usize) -> Vec<f32> {
    let n = values.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let left = i.saturating_sub(radius);
        let right = (i + radius + 1).min(n);
        let slice = &values[left..right];
        out.push(slice.iter().sum::<f32>() / slice.len() as f32);
    }
    out
}

fn score_body_band(
    density: &[f32],
    start: u32,
    end: u32,
    preferred_center: f32,
    width: f32,
) -> f32 {
    let mass: f32 = density[start as usize..end as usize].iter().sum();
    let center = (start + end) as f32 / 2.0;
    let center_distance = ((center - preferred_center).abs() / (width / 2.0)).clamp(0.0, 1.0);
    mass * (1.0 - center_distance * 0.2)
}

#[cfg(test)]
mod tests {
    use super::{
        StaticEdges, detect_static_edges, detect_text_body_band, detect_vertical_overlap,
        frames_near_stagnant, sampled_feature_difference, stitch_vertical,
        to_feature_map_from_gray,
    };
    use image::imageops::{self, crop_imm};
    use image::{Rgba, RgbaImage};

    #[test]
    fn duplicate_frames_are_detected() {
        let source = build_source(32, 80);
        let frame = crop(&source, 0, 40);
        assert!(frames_near_stagnant(&frame, &frame));
    }

    #[test]
    fn overlap_detection_handles_regular_scroll() {
        let source = build_source(48, 140);
        let first = crop(&source, 0, 60);
        let second = crop(&source, 23, 60);

        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(37));
    }

    #[test]
    fn overlap_detection_handles_large_final_overlap() {
        let source = build_source(48, 100);
        let second = crop(&source, 25, 60);
        let third = crop(&source, 40, 60);

        assert_eq!(detect_vertical_overlap(&second, &third, None), Some(45));
    }

    #[test]
    fn overlap_detection_handles_tiny_scroll_steps() {
        let source = build_source(48, 120);
        let first = crop(&source, 0, 100);
        let second = crop(&source, 2, 100);

        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(98));
    }

    #[test]
    fn uniform_frames_do_not_report_an_ambiguous_overlap() {
        let first = RgbaImage::from_pixel(48, 120, Rgba([245, 245, 245, 255]));
        let second = RgbaImage::from_pixel(48, 120, Rgba([245, 245, 245, 255]));

        assert_eq!(detect_vertical_overlap(&first, &second, None), None);
    }

    #[test]
    fn stitching_rebuilds_the_original_image() {
        let source = build_source(40, 115);
        let first = crop(&source, 0, 60);
        let second = crop(&source, 20, 60);
        let third = crop(&source, 55, 60);
        let overlaps = vec![
            detect_vertical_overlap(&first, &second, None).unwrap(),
            detect_vertical_overlap(&second, &third, None).unwrap(),
        ];

        let stitched = stitch_vertical(&[first, second, third], &overlaps).unwrap();
        assert_eq!(stitched, source);
    }

    #[test]
    fn overlap_detection_handles_low_texture_document_like_content() {
        let source = build_document_like_source(64, 180);
        let first = crop(&source, 0, 90);
        let second = crop(&source, 18, 90);

        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(72));
    }

    #[test]
    fn blank_page_margins_are_not_mistaken_for_static_edges() {
        let mut source = build_source(64, 220);
        for (start, end) in [(0, 12), (60, 72), (108, 120), (168, 180)] {
            for y in start..end {
                for x in 0..source.width() {
                    source.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
        }

        let first = crop(&source, 0, 120);
        let second = crop(&source, 60, 120);
        assert_eq!(
            detect_static_edges(&[&first, &second]),
            StaticEdges::default()
        );
        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(60));
    }

    #[test]
    fn feature_validation_distinguishes_a_correct_overlap_from_one_line_misalignment() {
        let source = build_document_like_source(120, 320);
        let previous = crop(&source, 0, 160);
        let current = crop(&source, 67, 160);
        let (previous_features, previous_has_features) =
            to_feature_map_from_gray(&imageops::grayscale(&previous), None);
        let (current_features, current_has_features) =
            to_feature_map_from_gray(&imageops::grayscale(&current), None);
        assert!(previous_has_features && current_has_features);

        let correct_overlap = 93;
        let correct = sampled_feature_difference(
            &previous_features,
            &current_features,
            previous.height() - correct_overlap,
            0,
            correct_overlap,
            2,
        );
        let one_line_misaligned_overlap = correct_overlap + 14;
        let misaligned = sampled_feature_difference(
            &previous_features,
            &current_features,
            previous.height() - one_line_misaligned_overlap,
            0,
            one_line_misaligned_overlap,
            2,
        );

        assert!(correct < 0.1, "correct feature difference: {correct}");
        assert!(
            misaligned > 0.25,
            "one-line feature difference should fail validation: {misaligned}"
        );
    }

    #[test]
    fn text_body_band_focuses_on_the_main_document_column() {
        let source = build_sidebar_document_source(120, 220);
        let first = crop(&source, 0, 120);
        let first_gray = imageops::grayscale(&first);
        let band = detect_text_body_band(&first_gray).unwrap();

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

        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(96));
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
                let shade = if ((y / 18) + (x / 6)) % 2 == 0 {
                    228
                } else {
                    238
                };
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

    fn build_striped_source(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([240, 240, 240, 255]));
        for y in (0..height).step_by(8) {
            for x in 0..width {
                let shade = if (y / 8) % 2 == 0 { 60 } else { 140 };
                let varied = shade + ((x * 5 + y * 3) % 20) as u8;
                image.put_pixel(x, y, Rgba([varied, varied, varied, 255]));
            }
        }
        image
    }

    // ── Scrollbar exclusion ─────────────────────────────────

    fn build_scrollbar_source(width: u32, height: u32) -> RgbaImage {
        let right_start = width.saturating_sub(24);
        let mut image = build_document_like_source(width, height);
        for y in 0..height {
            for x in right_start..width {
                let v = if ((y / 4) + (x / 4)) % 2 == 0 {
                    200u8
                } else {
                    180u8
                };
                image.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        image
    }

    #[test]
    fn scrollbar_margin_excludes_right_region() {
        let src = build_scrollbar_source(120, 160);
        let first = crop(&src, 0, 80);
        let first_gray = imageops::grayscale(&first);
        let band = super::detect_text_body_band(&first_gray);
        assert!(band.is_some(), "text body band should be detected");
        if let Some(b) = band {
            assert!(b.right < 115, "scrollbar margin should exclude right ~1.2%");
        }
    }

    // ── Edge cases ──────────────────────────────────────────

    #[test]
    fn overlap_detection_rejects_different_widths() {
        let a = RgbaImage::from_pixel(48, 100, Rgba([128, 128, 128, 255]));
        let b = RgbaImage::from_pixel(64, 100, Rgba([128, 128, 128, 255]));
        assert_eq!(detect_vertical_overlap(&a, &b, None), None);
    }

    #[test]
    fn overlap_detection_rejects_no_overlap() {
        let source = build_source(48, 200);
        let first = crop(&source, 0, 100);
        let second = crop(&source, 120, 100);
        assert_eq!(detect_vertical_overlap(&first, &second, None), None);
    }

    #[test]
    fn overlap_detection_handles_maximal_overlap() {
        let source = build_source(48, 200);
        let first = crop(&source, 0, 160);
        let second = crop(&source, 1, 160);
        let result = detect_vertical_overlap(&first, &second, None);
        assert!(result.is_some());
        assert!(result.unwrap() > 150);
    }

    // ── Content types ───────────────────────────────────────

    #[test]
    fn overlap_detection_handles_varied_scroll_offset() {
        let source = build_source(48, 200);
        let first = crop(&source, 0, 100);
        let second = crop(&source, 10, 100);
        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(90));
    }

    #[test]
    fn overlap_detection_handles_striped_content() {
        let source = build_striped_source(48, 160);
        let first = crop(&source, 0, 80);
        let second = crop(&source, 12, 80);
        assert_eq!(detect_vertical_overlap(&first, &second, None), Some(68));
    }

    #[test]
    fn overlap_detection_rejects_uniform_different_colors() {
        let a = RgbaImage::from_pixel(48, 100, Rgba([200, 200, 200, 255]));
        let b = RgbaImage::from_pixel(48, 100, Rgba([100, 100, 100, 255]));
        assert_eq!(detect_vertical_overlap(&a, &b, None), None);
    }

    // ── Stitching edge cases ────────────────────────────────

    #[test]
    fn stitching_handles_single_frame() {
        let frame = build_source(40, 50);
        let stitched = stitch_vertical(&[frame.clone()], &[]).unwrap();
        assert_eq!(stitched, frame);
    }

    #[test]
    fn stitching_rejects_frame_width_mismatch() {
        let a = RgbaImage::from_pixel(40, 50, Rgba([128, 128, 128, 255]));
        let b = RgbaImage::from_pixel(48, 50, Rgba([128, 128, 128, 255]));
        assert!(stitch_vertical(&[a, b], &[10]).is_err());
    }

    #[test]
    fn stitching_rejects_invalid_overlap_count() {
        let a = RgbaImage::from_pixel(40, 50, Rgba([128, 128, 128, 255]));
        let b = RgbaImage::from_pixel(40, 50, Rgba([128, 128, 128, 255]));
        assert!(stitch_vertical(&[a, b], &[]).is_err());
    }

    #[test]
    fn stitching_rejects_empty_frames() {
        assert!(stitch_vertical(&[], &[]).is_err());
    }

    // ── Text body band ──────────────────────────────────────

    #[test]
    fn detect_text_body_band_returns_none_for_uniform() {
        let uniform = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
        let gray = imageops::grayscale(&uniform);
        assert!(detect_text_body_band(&gray).is_none());
    }
}
