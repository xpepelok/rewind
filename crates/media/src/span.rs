use std::time::Duration;

pub const MIN_LENGTH: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Duration,
    pub end: Duration,
}

impl Span {
    pub fn whole(duration: Duration) -> Self {
        Self {
            start: Duration::ZERO,
            end: duration,
        }
    }

    pub fn length(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }

    pub fn is_whole(&self, duration: Duration) -> bool {
        self.start.is_zero() && self.end >= duration
    }

    pub fn with_start(self, start: Duration) -> Self {
        let start = start.min(self.end.saturating_sub(MIN_LENGTH));
        Self { start, ..self }
    }

    pub fn with_end(self, end: Duration, duration: Duration) -> Self {
        let end = end.min(duration).max(self.start + MIN_LENGTH);
        Self { end, ..self }
    }

    pub fn hns(value: Duration) -> i64 {
        (value.as_nanos() / 100).min(i64::MAX as u128) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn handles_never_cross() {
        let span = Span::whole(MINUTE).with_start(MINUTE);
        assert_eq!(span.length(), MIN_LENGTH);
        let span = Span::whole(MINUTE).with_end(Duration::ZERO, MINUTE);
        assert_eq!(span.end, MIN_LENGTH);
    }

    #[test]
    fn end_is_clamped_to_duration() {
        let span = Span::whole(MINUTE).with_end(MINUTE * 2, MINUTE);
        assert_eq!(span.end, MINUTE);
        assert!(span.is_whole(MINUTE));
    }

    #[test]
    fn hns_converts_hundreds_of_nanoseconds() {
        assert_eq!(Span::hns(Duration::from_secs(1)), 10_000_000);
    }
}
