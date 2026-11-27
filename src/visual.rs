//! Cheap spatial hints, not object recognition. Periodic samples remain necessary.
use crate::Region;

pub(crate) const PROBE_WIDTH: u32 = 96;
pub(crate) const PROBE_HEIGHT: u32 = 96;

#[derive(Default)]
pub(crate) struct ChangeDetector {
    previous: Option<Vec<u8>>,
    anchor: Option<(f64, Vec<u8>)>,
}
impl ChangeDetector {
    pub fn observe(&mut self, time: f64, pixels: Vec<u8>) -> Option<(f64, Region)> {
        let instant = self.previous.as_ref().and_then(|p| difference(p, &pixels));
        let accumulated = self
            .anchor
            .as_ref()
            .and_then(|(_, p)| difference(p, &pixels));
        let result = match (instant, accumulated) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
            (a, b) => a.or(b),
        };
        if self.anchor.as_ref().is_none_or(|(t, _)| time - *t >= 0.5) {
            self.anchor = Some((time, pixels.clone()));
        }
        self.previous = Some(pixels);
        result
    }
}
fn difference(a: &[u8], b: &[u8]) -> Option<(f64, Region)> {
    if a.len() != 96 * 96 * 3 || b.len() != a.len() {
        return None;
    }
    let mut tiles = [0u32; 256];
    for (i, (a, b)) in a.iter().zip(b).enumerate() {
        tiles[(i / 3 / 96 / 6) * 16 + (i / 3 % 96 / 6)] += u32::from(a.abs_diff(*b));
    }
    let global = tiles.iter().sum::<u32>() as f64 / (96.0 * 96.0 * 3.0 * 255.0);
    let mut bounds = (16usize, 16usize, 0usize, 0usize);
    let mut peak = 0f64;
    for (i, sum) in tiles.iter().enumerate() {
        let score = f64::from(*sum) / (6.0 * 6.0 * 3.0 * 255.0);
        peak = peak.max(score);
        if score >= 0.10 {
            bounds.0 = bounds.0.min(i % 16);
            bounds.1 = bounds.1.min(i / 16);
            bounds.2 = bounds.2.max(i % 16 + 1);
            bounds.3 = bounds.3.max(i / 16 + 1);
        }
    }
    if global < 0.06 && peak < 0.10 {
        return None;
    }
    let region = if bounds.0 == 16 {
        Region {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    } else {
        Region {
            x: bounds.0 as f64 / 16.0,
            y: bounds.1 as f64 / 16.0,
            width: (bounds.2 - bounds.0) as f64 / 16.0,
            height: (bounds.3 - bounds.1) as f64 / 16.0,
        }
    };
    Some((global.max(peak), region))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_change_survives_global_dilution_and_slow_changes_accumulate() {
        let mut detector = ChangeDetector::default();
        assert!(detector.observe(0.0, vec![0; 27648]).is_none());
        let mut small = vec![0; 27648];
        for y in 36..48 {
            for x in 60..72 {
                small[(y * 96 + x) * 3..(y * 96 + x) * 3 + 3].fill(180);
            }
        }
        let (_, r) = detector.observe(0.033, small).unwrap();
        assert_eq!(r.x, 0.625);
        assert_eq!(r.y, 0.375);
        let mut slow = ChangeDetector::default();
        slow.observe(0.0, vec![0; 27648]);
        let mut detected = false;
        for i in 1..15 {
            detected |= slow.observe(i as f64 / 30.0, vec![i * 2; 27648]).is_some();
        }
        assert!(detected);
    }
    #[test]
    fn locally_different_colors_are_not_lost_to_grayscale() {
        let mut first = vec![0; 96 * 96 * 3];
        for p in first.as_chunks_mut::<3>().0 {
            p[0] = 90;
        }
        let mut second = first.clone();
        for y in 24..30 {
            for x in 24..30 {
                second[(y * 96 + x) * 3..(y * 96 + x) * 3 + 3].copy_from_slice(&[0, 0, 255]);
            }
        }
        let mut detector = ChangeDetector::default();
        detector.observe(0.0, first);
        let (score, region) = detector.observe(0.033, second).unwrap();
        assert!(score > 0.1);
        assert_eq!(region.x, 0.25);
        assert_eq!(region.width, 0.0625);
    }
}
