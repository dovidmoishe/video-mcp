//! Bounded, source-resolution evidence for an agent's unresolved visual questions.
use crate::{EncodedFrame, Region};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailRequest {
    pub source: Option<String>,
    pub video_id: Option<String>,
    /// Sorted source times in seconds. At most eight over a 30-second span.
    #[schemars(length(min = 1, max = 8))]
    pub timestamps: Vec<f64>,
    /// Normalized full-frame crop in display orientation (as returned images
    /// appear, rotation metadata applied), not overview/filmstrip pixels.
    pub crop: Option<Region>,
    /// Optional normalized full-frame rectangle (display orientation) of an
    /// appearance patch in the first requested frame. Use dense timestamps; this
    /// is tentative tracking, not semantic identity.
    pub track_region: Option<Region>,
    #[serde(default = "width_default")]
    #[schemars(range(min = 1, max = 2048))]
    pub max_width: u32,
    #[serde(default = "height_default")]
    #[schemars(range(min = 1, max = 2048))]
    pub max_height: u32,
    /// Opt-in local Windows OCR; words must be checked against returned images.
    #[serde(default)]
    pub include_ocr: bool,
}
fn width_default() -> u32 {
    1280
}
fn height_default() -> u32 {
    1280
}
impl DetailRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.source.is_some() != self.video_id.is_some(),
            "provide exactly one of source or video_id"
        );
        ensure!(
            !self.timestamps.is_empty() && self.timestamps.len() <= 8,
            "timestamps must contain 1..8 values; batch nearby evidence requests"
        );
        ensure!(
            self.timestamps
                .iter()
                .all(|t| t.is_finite() && *t >= 0.0 && *t < 604800.0)
                && self.timestamps.windows(2).all(|p| p[1] > p[0])
                && self.timestamps.last().unwrap() - self.timestamps[0] <= 30.0,
            "timestamps must be finite, strictly increasing, nonnegative, within seven days and span <=30 seconds"
        );
        ensure!(
            (1..=2048).contains(&self.max_width) && (1..=2048).contains(&self.max_height),
            "max_width and max_height must each be 1..2048"
        );
        if let Some(crop) = self.crop {
            crop.validate()?;
        }
        if let Some(target) = self.track_region {
            target.validate()?;
            if let Some(view) = self.crop {
                ensure!(
                    target.x >= view.x
                        && target.y >= view.y
                        && target.x + target.width <= view.x + view.width
                        && target.y + target.height <= view.y + view.height,
                    "track_region must lie within crop; retain surrounding space for motion"
                );
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct DetailFrame {
    pub requested_timestamp: f64,
    pub timestamp: f64,
    pub estimated_timestamp: bool,
    pub width: u32,
    pub height: u32,
    /// Full-resolution frame size in display orientation.
    pub source_width: u32,
    pub source_height: u32,
    /// Crop rectangle `[x, y, width, height]` in display-oriented source pixels.
    pub source_pixels: [u32; 4],
    pub ocr: Option<crate::ocr::TextReading>,
    #[serde(skip)]
    pub image: EncodedFrame,
}
#[derive(Debug, Clone, Serialize)]
pub struct TextChange {
    pub before_timestamp: f64,
    pub after_timestamp: f64,
    pub before: String,
    pub after: String,
    pub region: Region,
}
#[derive(Debug, Clone, Serialize)]
pub struct TextMotion {
    pub text: String,
    pub from_timestamp: f64,
    pub to_timestamp: f64,
    pub from_region: Region,
    pub to_region: Region,
}
#[derive(Debug, Clone, Serialize)]
pub struct DetailPage {
    pub frames: Vec<DetailFrame>,
    pub text_changes: Vec<TextChange>,
    pub text_motion: Vec<TextMotion>,
    pub motion: Vec<crate::tracking::MotionObservation>,
    pub cache_hit: bool,
    pub decoded_frames: u64,
    pub processing_ms: u128,
    pub encoded_bytes: usize,
    pub evidence_note: &'static str,
}

#[cfg(feature = "native")]
pub(crate) fn extract(
    reader: &mut crate::video::VideoReader,
    metadata: &crate::Metadata,
    request: &DetailRequest,
    control: &crate::control::Control,
) -> Result<DetailPage> {
    use crate::{FrameOptions, ImageFormat, Window};
    use anyhow::Context;
    let started = std::time::Instant::now();
    let mut frames = Vec::new();
    let mut decoded_frames = 0;
    let mut tracker = crate::tracking::PatchTracker::default();
    let mut motion = Vec::new();
    let mut encoded_bytes = 0;
    let stream = metadata
        .streams
        .iter()
        .find(|s| s.kind == "video")
        .context("no video stream")?;
    let _ = stream;
    let crop = request.crop.unwrap_or(Region {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    });
    for time in &request.timestamps {
        control.check()?;
        ensure!(
            metadata.duration_seconds.is_none_or(|d| *time < d),
            "requested timestamp is at or beyond the media duration"
        );
        let end = metadata
            .duration_seconds
            .map_or(time + 1.0, |d| (time + 1.0).min(d));
        let options = FrameOptions {
            window: Window { start: *time, end },
            count: 1,
            interval: None,
            max_width: request.max_width,
            max_height: request.max_height,
            crop: request.crop,
            format: ImageFormat::Jpeg,
        };
        let mut found = false;
        let stats = reader.sample(&options, control, |sample| {
            control.check()?;
            let source_width = sample.source_width;
            let source_height = sample.source_height;
            let (x, y, w, h) = crop.pixels(source_width, source_height);
            let actual_region = Region {
                x: f64::from(x) / f64::from(source_width),
                y: f64::from(y) / f64::from(source_height),
                width: f64::from(w) / f64::from(source_width),
                height: f64::from(h) / f64::from(source_height),
            };
            if let Some(target) = request.track_region {
                motion.push(tracker.observe(
                    &sample.pixels,
                    (sample.width, sample.height),
                    actual_region,
                    target,
                    sample.timestamp,
                    control,
                )?);
            }
            let ocr = if request.include_ocr {
                Some(crate::ocr::read(
                    &sample.pixels,
                    sample.width,
                    sample.height,
                    actual_region,
                    control,
                )?)
            } else {
                None
            };
            let image = crate::media::encode_sample(&sample, ImageFormat::Jpeg)?;
            encoded_bytes += image.bytes.len();
            ensure!(
                encoded_bytes <= 12 * 1024 * 1024,
                "detail images exceed 12 MiB; reduce dimensions, timestamps or crop size"
            );
            frames.push(DetailFrame {
                requested_timestamp: *time,
                timestamp: sample.timestamp,
                estimated_timestamp: sample.estimated,
                width: sample.width,
                height: sample.height,
                source_width,
                source_height,
                source_pixels: [x, y, w, h],
                ocr,
                image,
            });
            found = true;
            Ok(())
        })?;
        decoded_frames += stats.decoded_frames;
        ensure!(
            found,
            "no decoded frame within one second after requested timestamp; try an earlier time or inspect metadata"
        );
    }
    let (text_changes, text_motion) = compare_text(&frames);
    Ok(DetailPage {
        frames,
        text_changes,
        text_motion,
        motion,
        cache_hit: false,
        decoded_frames,
        processing_ms: started.elapsed().as_millis(),
        encoded_bytes,
        evidence_note: "Images come from original decoded pixels in display orientation (rotation/flip metadata applied), cropped before resizing without upscaling. Crop, source_pixels, OCR and motion regions share that display-oriented full-frame space. Actual timestamps may follow requested times, especially for VFR. OCR is unverified and may be unavailable or incorrect. Patch motion reports reference/matched/lost/ambiguous/low-texture states and is approximate; text changes/motion are bounded spatial matches between adjacent observations, not semantic object identity or exact transition onset. Unobserved intervals remain unknown; inspect denser timestamps around uncertainty.",
    })
}
#[cfg(feature = "native")]
fn compare_text(frames: &[DetailFrame]) -> (Vec<TextChange>, Vec<TextMotion>) {
    let mut changes = Vec::new();
    let mut motion = Vec::new();
    for pair in frames.windows(2) {
        let (Some(a), Some(b)) = (&pair[0].ocr, &pair[1].ocr) else {
            continue;
        };
        for old in &a.words {
            // Same text and nearby location gives a tentative text-region track.
            let nearest = b
                .words
                .iter()
                .filter(|w| w.text == old.text)
                .min_by(|x, y| {
                    distance(x.region, old.region).total_cmp(&distance(y.region, old.region))
                });
            if let Some(new) = nearest.filter(|w| distance(w.region, old.region) < 0.15) {
                if distance(new.region, old.region) > 0.002 && motion.len() < 128 {
                    motion.push(TextMotion {
                        text: old.text.clone(),
                        from_timestamp: pair[0].timestamp,
                        to_timestamp: pair[1].timestamp,
                        from_region: old.region,
                        to_region: new.region,
                    });
                }
            } else if let Some(new) = b.words.iter().find(|w| overlap(w.region, old.region) > 0.5)
                && changes.len() < 128
            {
                changes.push(TextChange {
                    before_timestamp: pair[0].timestamp,
                    after_timestamp: pair[1].timestamp,
                    before: old.text.clone(),
                    after: new.text.clone(),
                    region: new.region,
                });
            }
        }
    }
    (changes, motion)
}
#[cfg(feature = "native")]
fn distance(a: Region, b: Region) -> f64 {
    ((a.x + a.width / 2.0 - b.x - b.width / 2.0).powi(2)
        + (a.y + a.height / 2.0 - b.y - b.height / 2.0).powi(2))
    .sqrt()
}
#[cfg(feature = "native")]
fn overlap(a: Region, b: Region) -> f64 {
    let intersection = ((a.x + a.width).min(b.x + b.width) - a.x.max(b.x)).max(0.0)
        * ((a.y + a.height).min(b.y + b.height) - a.y.max(b.y)).max(0.0);
    intersection / (a.width * a.height + b.width * b.height - intersection).max(f64::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detail_requests_reject_ambiguous_or_excessive_work() {
        let mut r = DetailRequest {
            source: Some("clip.mp4".into()),
            video_id: None,
            timestamps: vec![1.0, 1.033, 1.066],
            crop: None,
            track_region: None,
            max_width: 1280,
            max_height: 1280,
            include_ocr: false,
        };
        r.validate().unwrap();
        r.timestamps[1] = 1.0;
        assert!(r.validate().is_err());
        r.timestamps = vec![1.0, 32.0];
        assert!(r.validate().is_err());
        r.timestamps = vec![f64::NAN];
        assert!(r.validate().is_err());
        r.timestamps = vec![1.0];
        r.video_id = Some("both".into());
        assert!(r.validate().is_err());
        r.video_id = None;
        r.crop = Some(Region {
            x: 0.5,
            y: 0.5,
            width: 0.5,
            height: 0.5,
        });
        r.track_region = Some(Region {
            x: 0.0,
            y: 0.0,
            width: 0.1,
            height: 0.1,
        });
        assert!(r.validate().is_err());
    }
}
