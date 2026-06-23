//! gpui-free geometry for equity line charts: value<->pixel scales, "nice" axis
//! ticks, and stride downsampling to ~chart-width points. Pure + unit-tested so
//! it runs on a machine that can't link gpui; the painter (auracle_charts) is the
//! only consumer. Non-finite inputs are clamped/skipped, never panic.

/// An ordered inclusive numeric range (`min <= max` is maintained).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

impl Range {
    /// Build a range, ordering the bounds.
    pub fn new(a: f64, b: f64) -> Self {
        if a <= b {
            Self { min: a, max: b }
        } else {
            Self { min: b, max: a }
        }
    }

    /// The span; always >= 0, and 0 for a degenerate range.
    pub fn span(&self) -> f64 {
        (self.max - self.min).max(0.0)
    }

    /// Tight range over the finite samples, or None if none are finite.
    pub fn from_values(values: &[f64]) -> Option<Range> {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &v in values {
            if v.is_finite() {
                if v < lo {
                    lo = v;
                }
                if v > hi {
                    hi = v;
                }
            }
        }
        if lo.is_finite() && hi.is_finite() {
            Some(Range { min: lo, max: hi })
        } else {
            None
        }
    }
}

/// Maps a value in `domain` onto a pixel offset in `0..=length`. A zero-span
/// domain maps everything to the midpoint, so a flat series draws centered.
#[derive(Debug, Clone, Copy)]
pub struct Scale {
    pub domain: Range,
    pub length: f64,
}

impl Scale {
    pub fn new(domain: Range, length: f64) -> Self {
        Self {
            domain,
            length: length.max(0.0),
        }
    }

    /// Value -> pixel. Non-finite value or zero span -> centered; clamped to range.
    pub fn to_pixel(&self, value: f64) -> f64 {
        let span = self.domain.span();
        if span <= 0.0 || !value.is_finite() {
            return self.length / 2.0;
        }
        let t = (value - self.domain.min) / span;
        (t * self.length).clamp(0.0, self.length)
    }
}

/// Choose ~`target_count` "nice" tick values spanning `range` (rounded to a
/// 1/2/5 * 10^k step). Always returns at least the two endpoints; never hangs and
/// never returns unbounded output even for absurd ranges or counts.
pub fn nice_ticks(range: Range, target_count: usize) -> Vec<f64> {
    let span = range.span();
    // Clamp the target so the step math can't be driven to a degenerate value.
    let target = target_count.clamp(1, 1000) as f64;
    if span <= 0.0 || !span.is_finite() {
        return vec![range.min, range.max];
    }

    let raw = span / target;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.0 {
        2.0
    } else if norm < 7.0 {
        5.0
    } else {
        10.0
    };
    let step = nice * mag;
    let start = (range.min / step).ceil() * step;

    // Bail to endpoints if the step can't make progress (fp granularity / non-
    // finite). This is also the true-infinite-loop backstop.
    if !step.is_finite() || step <= 0.0 || start + step == start {
        return vec![range.min, range.max];
    }

    let mut ticks = Vec::new();
    // Absolute ceiling, independent of `target`, so a tiny step over a huge range
    // can never allocate without bound.
    let max_ticks = 10_000usize;
    let mut value = start;
    while value <= range.max + step * 0.5 && ticks.len() < max_ticks {
        ticks.push(value);
        value += step;
    }
    ticks.dedup();
    if ticks.is_empty() {
        return vec![range.min, range.max];
    }
    ticks
}

/// Downsample `len` points to at most `target` by striding, returning the kept
/// indices (always includes the first and last when `len > 0`).
pub fn downsample_stride(len: usize, target: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let target = target.max(1);
    if len <= target {
        return (0..len).collect();
    }
    let stride = (len as f64 / target as f64).max(1.0);
    let mut out: Vec<usize> = Vec::with_capacity(target + 1);
    let mut cursor = 0.0;
    while (cursor as usize) < len {
        let idx = cursor as usize;
        if out.last() != Some(&idx) {
            out.push(idx);
        }
        cursor += stride;
    }
    let last = len - 1;
    if out.last() != Some(&last) {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_orders_and_spans() {
        assert_eq!(Range::new(3.0, 1.0), Range { min: 1.0, max: 3.0 });
        assert_eq!(Range::new(1.0, 3.0).span(), 2.0);
        assert_eq!(Range::new(5.0, 5.0).span(), 0.0);
    }

    #[test]
    fn range_from_values_ignores_non_finite_and_empty() {
        assert_eq!(Range::from_values(&[]), None);
        assert_eq!(Range::from_values(&[f64::NAN, f64::INFINITY]), None);
        assert_eq!(
            Range::from_values(&[2.0, f64::NAN, -1.0, 5.0]),
            Some(Range {
                min: -1.0,
                max: 5.0
            })
        );
    }

    #[test]
    fn scale_maps_endpoints_and_clamps() {
        let s = Scale::new(Range::new(0.0, 100.0), 200.0);
        assert_eq!(s.to_pixel(0.0), 0.0);
        assert_eq!(s.to_pixel(100.0), 200.0);
        assert_eq!(s.to_pixel(50.0), 100.0);
        // Out-of-domain clamps into the pixel range, never panics.
        assert_eq!(s.to_pixel(1000.0), 200.0);
        assert_eq!(s.to_pixel(-1000.0), 0.0);
    }

    #[test]
    fn scale_flat_series_centers() {
        let s = Scale::new(Range::new(7.0, 7.0), 200.0);
        assert_eq!(s.to_pixel(7.0), 100.0);
        // Non-finite is centered, not a panic.
        assert_eq!(s.to_pixel(f64::NAN), 100.0);
    }

    #[test]
    fn nice_ticks_basic_round_numbers() {
        let ticks = nice_ticks(Range::new(0.0, 100.0), 5);
        assert_eq!(ticks.first(), Some(&0.0));
        assert!(ticks.last().is_some_and(|&v| v >= 100.0 - 1e-9));
        assert!(ticks.len() >= 2 && ticks.len() <= 12);
    }

    #[test]
    fn nice_ticks_degenerate_is_endpoints() {
        assert_eq!(nice_ticks(Range::new(5.0, 5.0), 5), vec![5.0, 5.0]);
    }

    #[test]
    fn nice_ticks_absurd_count_is_bounded() {
        // BUG 1 regression: the loop ceiling must not derive from target_count.
        let ticks = nice_ticks(Range::new(0.0, 1000.0), usize::MAX);
        assert!(ticks.len() <= 10_000);
        let ticks = nice_ticks(Range::new(0.0, 1e300), usize::MAX);
        assert!(ticks.len() <= 10_000);
    }

    #[test]
    fn nice_ticks_tiny_range_does_not_explode() {
        // BUG 2 regression: a span at fp granularity must not emit hundreds of
        // identical ticks.
        let ticks = nice_ticks(Range::new(1.0, 1.0 + f64::EPSILON), 5);
        assert!(ticks.len() <= 3, "got {} ticks", ticks.len());
    }

    #[test]
    fn downsample_keeps_first_and_last() {
        let idx = downsample_stride(1000, 100);
        assert!(idx.len() <= 101);
        assert_eq!(idx.first(), Some(&0));
        assert_eq!(idx.last(), Some(&999));
    }

    #[test]
    fn downsample_small_is_identity() {
        assert_eq!(downsample_stride(5, 100), vec![0, 1, 2, 3, 4]);
        assert_eq!(downsample_stride(0, 100), Vec::<usize>::new());
        // target 0 is treated as 1, still returns first+last.
        assert_eq!(downsample_stride(3, 0), vec![0, 2]);
    }
}
