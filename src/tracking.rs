//! Bounded template matching for a user-selected patch. This tracks appearance,
//! not semantic identity, and explicitly reports loss/ambiguity.
use crate::Region;
#[cfg(feature = "native")]
use crate::control::Control;
#[cfg(feature = "native")]
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MotionObservation {
    pub timestamp: f64,
    pub status: String,
    pub region: Option<Region>,
    pub mean_absolute_error: Option<f64>,
}
#[cfg(feature = "native")]
#[derive(Default)]
pub(crate) struct PatchTracker {
    template: Vec<u8>,
    shape: (u32, u32),
    position: (u32, u32),
    image_shape: (u32, u32),
    disabled: bool,
}
#[cfg(feature = "native")]
impl PatchTracker {
    pub fn observe(
        &mut self,
        pixels: &[u8],
        size: (u32, u32),
        view: Region,
        target: Region,
        time: f64,
        control: &Control,
    ) -> Result<MotionObservation> {
        let (width, height) = size;
        control.check()?;
        let scale = (192.0 / f64::from(width.max(height))).min(1.0);
        let w = (f64::from(width) * scale).round().max(1.0) as u32;
        let h = (f64::from(height) * scale).round().max(1.0) as u32;
        let rgb = image::RgbImage::from_raw(width, height, pixels.to_vec())
            .expect("validated packed RGB");
        let gray = image::imageops::resize(
            &image::DynamicImage::ImageRgb8(rgb).into_luma8(),
            w,
            h,
            image::imageops::FilterType::Triangle,
        );
        let failure = |status: &str| MotionObservation {
            timestamp: time,
            status: status.into(),
            region: None,
            mean_absolute_error: None,
        };
        if self.disabled {
            return Ok(failure("unavailable_template"));
        }
        let mut error = 0.0;
        let status = if self.template.is_empty() {
            let relative = Region {
                x: (target.x - view.x) / view.width,
                y: (target.y - view.y) / view.height,
                width: target.width / view.width,
                height: target.height / view.height,
            };
            relative.validate()?;
            let (x, y, tw, th) = relative.pixels(w, h);
            if !(3..=48).contains(&tw) || !(3..=48).contains(&th) {
                self.disabled = true;
                return Ok(failure("template_must_span_3_to_48_probe_pixels"));
            }
            self.shape = (tw, th);
            self.position = (x, y);
            self.image_shape = (w, h);
            for yy in y..y + th {
                for xx in x..x + tw {
                    self.template.push(gray.get_pixel(xx, yy)[0]);
                }
            }
            let mean = self.template.iter().map(|p| f64::from(*p)).sum::<f64>()
                / self.template.len() as f64;
            let variance = self
                .template
                .iter()
                .map(|p| (f64::from(*p) - mean).powi(2))
                .sum::<f64>()
                / self.template.len() as f64;
            if variance < 20.0 {
                self.disabled = true;
                return Ok(failure("low_texture_template"));
            }
            "reference"
        } else {
            if self.image_shape != (w, h) {
                return Ok(failure("image_dimensions_changed"));
            }
            let (tw, th) = self.shape;
            let (px, py) = self.position;
            let mut scores = Vec::new();
            for y in py.saturating_sub(32)..=(py + 32).min(h - th) {
                control.check()?;
                for x in px.saturating_sub(32)..=(px + 32).min(w - tw) {
                    let mut sum = 0u32;
                    for yy in 0..th {
                        for xx in 0..tw {
                            sum += u32::from(
                                self.template[(yy * tw + xx) as usize]
                                    .abs_diff(gray.get_pixel(x + xx, y + yy)[0]),
                            );
                        }
                    }
                    scores.push((sum as f64 / (f64::from(tw * th) * 255.0), x, y));
                }
            }
            scores.sort_by(|a, b| a.0.total_cmp(&b.0));
            let best = scores[0];
            error = best.0;
            if error > 0.15 {
                return Ok(failure("lost"));
            }
            if scores
                .iter()
                .any(|s| s.1.abs_diff(best.1) + s.2.abs_diff(best.2) > 4 && s.0 - best.0 < 0.005)
            {
                return Ok(failure("ambiguous"));
            }
            self.position = (best.1, best.2);
            "matched"
        };
        let (x, y) = self.position;
        let (tw, th) = self.shape;
        Ok(MotionObservation {
            timestamp: time,
            status: status.into(),
            region: Some(Region {
                x: view.x + f64::from(x) / f64::from(w) * view.width,
                y: view.y + f64::from(y) / f64::from(h) * view.height,
                width: f64::from(tw) / f64::from(w) * view.width,
                height: f64::from(th) / f64::from(h) * view.height,
            }),
            mean_absolute_error: Some(error),
        })
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    #[test]
    fn follows_patch_and_reports_low_texture_and_loss() {
        let view = Region {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let target = Region {
            x: 0.1,
            y: 0.2,
            width: 0.125,
            height: 0.125,
        };
        let control = Control::new(std::time::Duration::from_secs(10));
        let mut tracker = PatchTracker::default();
        let frame = |offset: u32| {
            let mut im = image::RgbImage::new(160, 160);
            for y in 34..48 {
                for x in 19 + offset..32 + offset {
                    im.put_pixel(x, y, image::Rgb([220, 140, 30]));
                }
            }
            im.into_raw()
        };
        assert_eq!(
            tracker
                .observe(&frame(0), (160, 160), view, target, 0.0, &control)
                .unwrap()
                .status,
            "reference"
        );
        let moved = tracker
            .observe(&frame(15), (160, 160), view, target, 0.1, &control)
            .unwrap();
        assert_eq!(moved.status, "matched");
        assert!(moved.region.unwrap().x > target.x + 0.07);
        let lost = tracker
            .observe(
                &vec![0; 160 * 160 * 3],
                (160, 160),
                view,
                target,
                0.2,
                &control,
            )
            .unwrap();
        assert!(matches!(lost.status.as_str(), "lost" | "ambiguous"));
        let flat = PatchTracker::default()
            .observe(
                &vec![0; 160 * 160 * 3],
                (160, 160),
                view,
                target,
                0.0,
                &control,
            )
            .unwrap();
        assert_eq!(flat.status, "low_texture_template");
    }
}
