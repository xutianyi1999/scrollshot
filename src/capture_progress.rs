const HISTORY_WINDOW: usize = 5;
const SMOOTHING_WINDOW: usize = 3;
// Rendering glitches can span several settled frames; keep recovery bounded
// without treating the first few misses as page completion.
const MAX_CONSECUTIVE_ESTIMATES: usize = 5;
const BOTTOM_CONFIRMATIONS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureDecision {
    AppendMeasured(u32),
    AppendEstimated(u32),
    Retry,
    ReachedBottom,
    StopUnreliable,
}

#[derive(Debug, Default)]
pub(crate) struct CaptureProgress {
    measured_overlaps: Vec<u32>,
    stagnant_captures: usize,
    consecutive_unmatched: usize,
}

impl CaptureProgress {
    pub(crate) fn expected_overlap(&self) -> Option<f32> {
        let recent = self
            .measured_overlaps
            .iter()
            .rev()
            .take(HISTORY_WINDOW)
            .copied();
        let (total, count) = recent.fold((0u32, 0u32), |(total, count), overlap| {
            (total.saturating_add(overlap), count + 1)
        });
        (count > 0).then_some(total as f32 / count as f32)
    }

    pub(crate) fn record_stagnant(&mut self) -> CaptureDecision {
        self.stagnant_captures += 1;
        self.consecutive_unmatched = 0;
        if self.stagnant_captures >= BOTTOM_CONFIRMATIONS {
            CaptureDecision::ReachedBottom
        } else {
            CaptureDecision::Retry
        }
    }

    pub(crate) fn record_measured_with_height(
        &mut self,
        overlap: u32,
        frame_height: Option<u32>,
    ) -> CaptureDecision {
        let no_progress = frame_height.is_some_and(|height| {
            overlap >= height.saturating_sub(1)
                && self
                    .expected_overlap()
                    .is_some_and(|expected| height as f32 - expected > 4.0)
        });
        if no_progress {
            self.stagnant_captures += 1;
            self.consecutive_unmatched = 0;
            if self.stagnant_captures >= BOTTOM_CONFIRMATIONS {
                return CaptureDecision::ReachedBottom;
            }
            return CaptureDecision::AppendMeasured(overlap);
        }

        let smoothed = smooth_overlap(overlap, &self.measured_overlaps);
        self.measured_overlaps.push(smoothed);
        self.stagnant_captures = 0;
        self.consecutive_unmatched = 0;
        CaptureDecision::AppendMeasured(overlap)
    }

    pub(crate) fn record_unmatched(&mut self, frame_height: u32) -> CaptureDecision {
        self.stagnant_captures = 0;
        self.consecutive_unmatched += 1;

        if self.consecutive_unmatched <= MAX_CONSECUTIVE_ESTIMATES
            && let Some(overlap) =
                estimate_overlap_from_history(&self.measured_overlaps, frame_height)
        {
            return CaptureDecision::AppendEstimated(overlap);
        }

        if self.consecutive_unmatched < MAX_CONSECUTIVE_ESTIMATES {
            CaptureDecision::Retry
        } else {
            CaptureDecision::StopUnreliable
        }
    }

    pub(crate) fn measured_overlaps(&self) -> &[u32] {
        &self.measured_overlaps
    }
}

fn smooth_overlap(current: u32, measured: &[u32]) -> u32 {
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
    (min_allowed <= max_allowed).then_some(median.clamp(min_allowed, max_allowed))
}
