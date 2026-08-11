// Never invent an overlap for an unmatched frame: a single wrong estimate
// permanently shifts every later seam. A full viewport typically retains
// ample overlap across several wheel steps, so allow a dynamic page enough
// time to settle and recover before stopping.
const MAX_CONSECUTIVE_RETRIES: usize = 10;
const BOTTOM_CONFIRMATIONS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureDecision {
    AppendMeasured(u32),
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
                    .recent_median_overlap()
                    .is_some_and(|expected| height.saturating_sub(expected) > 4)
        });
        if no_progress {
            self.stagnant_captures += 1;
            self.consecutive_unmatched = 0;
            if self.stagnant_captures >= BOTTOM_CONFIRMATIONS {
                return CaptureDecision::ReachedBottom;
            }
            return CaptureDecision::AppendMeasured(overlap);
        }

        // A verified pairwise match is more trustworthy than scroll history:
        // real browsers legitimately vary their scroll distance. Keep it
        // verbatim for diagnostics and bottom detection instead of rewriting
        // it with a temporal average.
        self.measured_overlaps.push(overlap);
        self.stagnant_captures = 0;
        self.consecutive_unmatched = 0;
        CaptureDecision::AppendMeasured(overlap)
    }

    pub(crate) fn record_unmatched(&mut self) -> CaptureDecision {
        self.stagnant_captures = 0;
        self.consecutive_unmatched += 1;

        if self.consecutive_unmatched >= MAX_CONSECUTIVE_RETRIES {
            CaptureDecision::StopUnreliable
        } else {
            CaptureDecision::Retry
        }
    }

    pub(crate) fn measured_overlaps(&self) -> &[u32] {
        &self.measured_overlaps
    }

    pub(crate) fn is_recovering(&self) -> bool {
        self.consecutive_unmatched > 0
    }

    pub(crate) fn recovery_attempts(&self) -> usize {
        self.consecutive_unmatched
    }

    fn recent_median_overlap(&self) -> Option<u32> {
        if self.measured_overlaps.len() < 2 {
            return None;
        }

        let start = self.measured_overlaps.len().saturating_sub(5);
        let mut recent = self.measured_overlaps[start..].to_vec();
        recent.sort_unstable();
        Some(recent[recent.len() / 2])
    }
}
