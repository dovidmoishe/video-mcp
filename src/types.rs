use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Window {
    #[serde(default)]
    pub start: f64,
    #[serde(default = "default_end")]
    pub end: f64,
}
fn default_end() -> f64 {
    30.0
}
impl Default for Window {
    fn default() -> Self {
        Self {
            start: 0.0,
            end: 30.0,
        }
    }
}
impl Window {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.start.is_finite()
                && self.end.is_finite()
                && self.start >= 0.0
                && self.end > self.start,
            "expected finite 0 <= start < end"
        );
        ensure!(
            self.end <= 604800.0 && self.end - self.start <= 600.0,
            "window must be <= 600 seconds within the first seven days"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Jpeg,
    Webp,
}
impl ImageFormat {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FrameOptions {
    #[serde(default)]
    pub window: Window,
    #[serde(default = "default_count")]
    #[schemars(range(min = 1, max = 32))]
    pub count: usize,
    #[serde(default)]
    #[schemars(range(min = 0.001))]
    pub interval: Option<f64>,
    #[serde(default = "default_width")]
    #[schemars(range(min = 1, max = 2048))]
    pub max_width: u32,
    #[serde(default = "default_height")]
    #[schemars(range(min = 1, max = 2048))]
    pub max_height: u32,
    /// Optional normalized rectangle of the full frame in display orientation,
    /// cropped at source resolution before output resizing.
    #[serde(default)]
    pub crop: Option<Region>,
    #[serde(default)]
    pub format: ImageFormat,
}
fn default_count() -> usize {
    8
}
fn default_width() -> u32 {
    960
}
fn default_height() -> u32 {
    540
}
impl Default for FrameOptions {
    fn default() -> Self {
        Self {
            window: Window::default(),
            count: 8,
            interval: None,
            max_width: 960,
            max_height: 540,
            crop: None,
            format: ImageFormat::Jpeg,
        }
    }
}
impl FrameOptions {
    pub fn validate(&self) -> Result<()> {
        self.window.validate()?;
        ensure!((1..=32).contains(&self.count), "count must be 1..32");
        ensure!(
            (1..=2048).contains(&self.max_width) && (1..=2048).contains(&self.max_height),
            "max_width and max_height must each be 1..2048; use a source crop for small text"
        );
        if let Some(i) = self.interval {
            ensure!(
                i.is_finite() && i >= 0.001,
                "interval must be finite and >= 0.001 seconds; output remains capped at count (1..32)"
            );
        }
        if let Some(crop) = &self.crop {
            crop.validate()?;
        }
        Ok(())
    }
    pub fn step(&self) -> f64 {
        self.interval
            .unwrap_or((self.window.end - self.window.start) / self.count as f64)
    }
}
/// Full-frame coordinates normalized to 0..1 in display orientation: the upright
/// orientation of returned images, with any rotation/flip display metadata
/// already applied. Not filmstrip pixels. Pixel bounds are returned with evidence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Region {
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub width: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub height: f64,
}
impl Region {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [self.x, self.y, self.width, self.height]
                .iter()
                .all(|v| v.is_finite())
                && (0.0..1.0).contains(&self.x)
                && (0.0..1.0).contains(&self.y)
                && self.width > 0.0
                && self.height > 0.0
                && self.x + self.width <= 1.0
                && self.y + self.height <= 1.0,
            "crop requires finite normalized coordinates: x,y >= 0; width,height > 0; x+width,y+height <= 1"
        );
        Ok(())
    }
    pub fn pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let x = (self.x * f64::from(width)).floor() as u32;
        let y = (self.y * f64::from(height)).floor() as u32;
        let right = ((self.x + self.width) * f64::from(width))
            .ceil()
            .min(f64::from(width)) as u32;
        let bottom = ((self.y + self.height) * f64::from(height))
            .ceil()
            .min(f64::from(height)) as u32;
        (
            x,
            y,
            right.saturating_sub(x).max(1),
            bottom.saturating_sub(y).max(1),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMetadata {
    pub index: usize,
    pub kind: String,
    pub codec: String,
    pub start_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    /// Coded (stored) picture size, before the display matrix is applied.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Display-matrix rotation in degrees as ffprobe reports it: counterclockwise,
    /// so -90 means the stored picture is shown turned 90 degrees clockwise.
    /// 0 without a display matrix.
    pub rotation: Option<i32>,
    /// What every returned frame and coordinate has had applied, e.g.
    /// "rotate_90_clockwise"; "none" when the stored picture is already upright.
    pub display_transform: Option<crate::orientation::DisplayTransform>,
    /// Picture size as displayed and as frames are returned.
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub duration_seconds: Option<f64>,
    pub format: String,
    pub streams: Vec<StreamMetadata>,
}
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub timestamp: f64,
    pub estimated_timestamp: bool,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub segments: Vec<Segment>,
    pub truncated: bool,
    pub speech_detected: bool,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_unbounded_requests() {
        let mut o = FrameOptions {
            count: 33,
            ..Default::default()
        };
        assert!(o.validate().is_err());
        o.count = 1;
        o.window.start = f64::NAN;
        assert!(o.validate().is_err());
    }
    #[test]
    fn defaults_valid() {
        FrameOptions::default().validate().unwrap();
    }
    #[test]
    fn source_regions_and_portrait_bounds() {
        let mut options = FrameOptions {
            max_width: 1080,
            max_height: 1920,
            interval: Some(1.0 / 60.0),
            crop: Some(Region {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 1.0,
            }),
            ..Default::default()
        };
        options.validate().unwrap();
        assert_eq!(
            options.crop.unwrap().pixels(1080, 1920),
            (540, 0, 540, 1920)
        );
        options.crop.as_mut().unwrap().width = 0.51;
        assert!(options.validate().is_err());
        options.crop.as_mut().unwrap().width = f64::NAN;
        assert!(options.validate().is_err());
        assert!(
            Region {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 1.0
            }
            .validate()
            .is_err()
        );
    }
}
