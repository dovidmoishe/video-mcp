/// Convert a presentation timestamp to seconds relative to the container origin.
pub fn seconds(ts: Option<i64>, numerator: i32, denominator: i32, origin: f64) -> Option<f64> {
    let ts = ts.filter(|t| *t != i64::MIN)?;
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    let value = ts as f64 * numerator as f64 / denominator as f64 - origin;
    value.is_finite().then_some(value)
}
pub struct Clock {
    previous: Option<f64>,
    step: f64,
    fallback: f64,
}
impl Clock {
    pub fn new(step: f64, fallback: f64) -> Self {
        Self {
            previous: None,
            step,
            fallback,
        }
    }
    pub fn next(&mut self, timestamp: Option<f64>) -> (f64, bool) {
        let valid = timestamp.filter(|t| t.is_finite() && self.previous.is_none_or(|p| *t >= p));
        let estimated = valid.is_none();
        let t = valid.unwrap_or_else(|| self.previous.map_or(self.fallback, |p| p + self.step));
        self.previous = Some(t);
        (t, estimated)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timebase_and_origin() {
        assert_eq!(seconds(Some(180000), 1, 90000, 1.5), Some(0.5));
        assert_eq!(seconds(Some(i64::MIN), 1, 1, 0.0), None);
        assert_eq!(seconds(Some(1), 1, 0, 0.0), None);
    }
    #[test]
    fn vfr_and_missing() {
        let mut c = Clock::new(0.04, 0.0);
        assert_eq!(c.next(Some(0.0)), (0.0, false));
        assert_eq!(c.next(Some(0.1)), (0.1, false));
        assert_eq!(c.next(None), (0.14, true));
        assert!(c.next(Some(0.02)).1);
    }
}
