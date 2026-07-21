//! Process-lifetime counters and latency histograms for the pipeline
//! (M5 / Atlas §5–§6).
//!
//! Deliberately dependency-free: plain atomics behind a snapshot type, with
//! the Prometheus *text* rendering left to `synthpass-serve`. A metrics crate
//! would buy registries and exporters this project doesn't need — there is one
//! process, one registry, and one endpoint.
//!
//! **PII rule.** Nothing here may ever record document content or an extracted
//! field value. Metrics are counts and durations only; the label space is
//! closed (`Method::as_str`), never derived from input. That is what makes the
//! `/metrics` endpoint safe to scrape from outside the trust boundary while
//! the audit log stays SHA-256-only.

use crate::Method;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Upper bounds, in seconds, for the latency histograms. Chosen for the two
/// things actually being timed: OCR passes that run in tenths of a second on a
/// clean scan but climb through the retry budget on a bad one, and Tier-2
/// generations that take seconds on CPU. The final `+Inf` bucket is implicit.
pub const LATENCY_BUCKETS_SECONDS: [f64; 8] = [0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

/// A cumulative-bucket latency histogram in the Prometheus sense: each bucket
/// counts observations *less than or equal to* its bound, so the rendered
/// series is monotonic and `histogram_quantile` works against it.
#[derive(Debug, Default)]
pub struct Histogram {
    /// One counter per bound in [`LATENCY_BUCKETS_SECONDS`], each holding the
    /// count of observations that fell at or below it.
    buckets: [AtomicU64; LATENCY_BUCKETS_SECONDS.len()],
    /// Total observed time, in nanoseconds, to avoid accumulating float error
    /// across a long-running process. Rendered as seconds.
    sum_nanos: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    pub fn observe(&self, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        for (i, bound) in LATENCY_BUCKETS_SECONDS.iter().enumerate() {
            if seconds <= *bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // `as u64` saturates at u64::MAX rather than wrapping, so an absurd
        // duration can't silently corrupt the sum.
        self.sum_nanos
            .fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: std::array::from_fn(|i| {
                (
                    LATENCY_BUCKETS_SECONDS[i],
                    self.buckets[i].load(Ordering::Relaxed),
                )
            }),
            sum_seconds: self.sum_nanos.load(Ordering::Relaxed) as f64 / 1e9,
            count: self.count.load(Ordering::Relaxed),
        }
    }
}

/// A consistent-enough point-in-time read of a [`Histogram`].
///
/// Not atomic as a whole: counters are read one at a time, so a concurrent
/// `observe` can land between reads. That is the same guarantee every
/// Prometheus client gives — a scrape is a sample, not a transaction.
#[derive(Debug, Clone)]
pub struct HistogramSnapshot {
    pub buckets: [(f64, u64); LATENCY_BUCKETS_SECONDS.len()],
    pub sum_seconds: f64,
    pub count: u64,
}

/// Counters and histograms covering one pipeline's lifetime.
#[derive(Debug, Default)]
pub struct PipelineMetrics {
    documents_tier1: AtomicU64,
    documents_tier2: AtomicU64,
    ocr_failures: AtomicU64,
    tier2_failures: AtomicU64,
    pub(crate) ocr_seconds: Histogram,
    pub(crate) tier2_seconds: Histogram,
}

impl PipelineMetrics {
    /// Record a document that reached a terminal extraction, tagged by the
    /// tier that produced it.
    pub(crate) fn record_document(&self, method: Method) {
        match method {
            Method::MrzDeterministic => &self.documents_tier1,
            Method::Llm => &self.documents_tier2,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_ocr_failure(&self) {
        self.ocr_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_tier2_failure(&self) {
        self.tier2_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self, queue_depth: u64) -> MetricsSnapshot {
        MetricsSnapshot {
            documents_tier1: self.documents_tier1.load(Ordering::Relaxed),
            documents_tier2: self.documents_tier2.load(Ordering::Relaxed),
            ocr_failures: self.ocr_failures.load(Ordering::Relaxed),
            tier2_failures: self.tier2_failures.load(Ordering::Relaxed),
            ocr_seconds: self.ocr_seconds.snapshot(),
            tier2_seconds: self.tier2_seconds.snapshot(),
            queue_depth,
        }
    }
}

/// Everything `/metrics` needs, read once so the renderer can't observe the
/// counters shifting mid-format.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub documents_tier1: u64,
    pub documents_tier2: u64,
    pub ocr_failures: u64,
    pub tier2_failures: u64,
    pub ocr_seconds: HistogramSnapshot,
    pub tier2_seconds: HistogramSnapshot,
    pub queue_depth: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_are_cumulative() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(30)); // <= 0.05, so <= every bound
        h.observe(Duration::from_millis(300)); // > 0.25, <= 0.5 and up

        let snap = h.snapshot();
        assert_eq!(snap.count, 2);
        // 0.05 and 0.1 catch only the 30ms observation...
        assert_eq!(snap.buckets[0], (0.05, 1));
        assert_eq!(snap.buckets[1], (0.1, 1));
        assert_eq!(snap.buckets[2], (0.25, 1));
        // ...and from 0.5 up, both.
        assert_eq!(snap.buckets[3], (0.5, 2));
        assert_eq!(snap.buckets[7], (10.0, 2));
        // Counts never decrease as the bound grows — the property that makes
        // this a valid Prometheus histogram.
        for pair in snap.buckets.windows(2) {
            assert!(pair[0].1 <= pair[1].1, "buckets must be monotonic");
        }
    }

    #[test]
    fn histogram_sums_in_seconds() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(1500));
        h.observe(Duration::from_millis(500));
        let snap = h.snapshot();
        assert!(
            (snap.sum_seconds - 2.0).abs() < 1e-9,
            "expected 2s, got {}",
            snap.sum_seconds
        );
    }

    #[test]
    fn an_observation_beyond_every_bound_counts_but_lands_in_no_bucket() {
        let h = Histogram::default();
        h.observe(Duration::from_secs(60));
        let snap = h.snapshot();
        assert_eq!(snap.count, 1, "+Inf still counts it");
        assert!(
            snap.buckets.iter().all(|(_, n)| *n == 0),
            "nothing at or below 10s"
        );
    }

    #[test]
    fn documents_are_counted_by_tier() {
        let m = PipelineMetrics::default();
        m.record_document(Method::MrzDeterministic);
        m.record_document(Method::MrzDeterministic);
        m.record_document(Method::Llm);

        let snap = m.snapshot(3);
        assert_eq!(snap.documents_tier1, 2);
        assert_eq!(snap.documents_tier2, 1);
        assert_eq!(snap.queue_depth, 3, "queue depth is passed in, not stored");
    }

    #[test]
    fn failures_are_tracked_separately_from_successes() {
        let m = PipelineMetrics::default();
        m.record_ocr_failure();
        m.record_tier2_failure();
        m.record_tier2_failure();

        let snap = m.snapshot(0);
        assert_eq!(snap.ocr_failures, 1);
        assert_eq!(snap.tier2_failures, 2);
        assert_eq!(
            snap.documents_tier1 + snap.documents_tier2,
            0,
            "a failure is not a processed document"
        );
    }
}
