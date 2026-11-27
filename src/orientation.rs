//! Display orientation. Decoders return pictures as stored; a container display
//! matrix tells players how to turn them upright (an iPhone portrait clip is
//! stored 1920x1080 with rotation -90 and shown 1080x1920). Every returned image
//! and every normalized region uses display orientation. Pixel work stays in
//! stored orientation with exactly mapped rectangles, so only small output
//! images are transformed.
use crate::Region;
use serde::{Deserialize, Serialize};

/// One of the eight right-angle rotations/reflections a display matrix can
/// request, named by what is applied to the stored picture for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayTransform {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "rotate_90_clockwise")]
    Rotate90Clockwise,
    #[serde(rename = "rotate_180")]
    Rotate180,
    #[serde(rename = "rotate_90_counterclockwise")]
    Rotate90Counterclockwise,
    #[serde(rename = "flip_horizontal")]
    FlipHorizontal,
    #[serde(rename = "flip_vertical")]
    FlipVertical,
    /// Mirror across the top-left to bottom-right diagonal.
    #[serde(rename = "transpose")]
    Transpose,
    /// Mirror across the top-right to bottom-left diagonal.
    #[serde(rename = "transverse")]
    Transverse,
}

impl DisplayTransform {
    /// The transform FFmpeg's command-line autorotation inserts for a display
    /// matrix: fftools `get_rotation` followed by its transpose/hflip/vflip
    /// choice. `counterclockwise` is `av_display_rotation_get(matrix)`. Angles
    /// that are not right angles stay untransformed (FFmpeg would rotate and
    /// fill corners); a singular matrix (NaN) is ignored like in FFmpeg.
    pub fn from_display_matrix(matrix: &[i32; 9], counterclockwise: f64) -> Self {
        if !counterclockwise.is_finite() {
            return Self::None;
        }
        let mut theta = -counterclockwise.round();
        theta -= 360.0 * (theta / 360.0 + 0.9 / 360.0).floor();
        let near = |angle: f64| (theta - angle).abs() < 1.0;
        if near(90.0) {
            if matrix[3] > 0 {
                Self::Transpose
            } else {
                Self::Rotate90Clockwise
            }
        } else if near(180.0) {
            match (matrix[0] < 0, matrix[4] < 0) {
                (true, true) => Self::Rotate180,
                (true, false) => Self::FlipHorizontal,
                (false, true) => Self::FlipVertical,
                (false, false) => Self::None,
            }
        } else if near(270.0) {
            if matrix[3] < 0 {
                Self::Transverse
            } else {
                Self::Rotate90Counterclockwise
            }
        } else if theta.abs() < 1.0 && matrix[4] < 0 {
            Self::FlipVertical
        } else {
            Self::None
        }
    }
    /// (swap axes, mirror stored x, mirror stored y): display pixel (x, y) shows
    /// stored pixel (x, y), swapped first when transposed, then mirrored along
    /// the stored axes.
    fn parts(self) -> (bool, bool, bool) {
        match self {
            Self::None => (false, false, false),
            Self::FlipHorizontal => (false, true, false),
            Self::FlipVertical => (false, false, true),
            Self::Rotate180 => (false, true, true),
            Self::Transpose => (true, false, false),
            Self::Rotate90Clockwise => (true, false, true),
            Self::Rotate90Counterclockwise => (true, true, false),
            Self::Transverse => (true, true, true),
        }
    }
    pub fn swaps_axes(self) -> bool {
        self.parts().0
    }
    /// Display size of a stored picture.
    pub fn display_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }
    /// Stored size of a display-oriented picture (swapping is its own inverse).
    pub fn stored_size(self, width: u32, height: u32) -> (u32, u32) {
        self.display_size(width, height)
    }
    /// The stored-picture pixel rectangle `(x, y, width, height)` that shows as
    /// `rect` on display. `stored` is the stored picture size; `rect` must lie
    /// inside the display picture.
    pub fn rect_to_stored(
        self,
        rect: (u32, u32, u32, u32),
        stored: (u32, u32),
    ) -> (u32, u32, u32, u32) {
        let (swap, mirror_x, mirror_y) = self.parts();
        let (x, y, width, height) = if swap {
            (rect.1, rect.0, rect.3, rect.2)
        } else {
            rect
        };
        (
            if mirror_x { stored.0 - x - width } else { x },
            if mirror_y { stored.1 - y - height } else { y },
            width,
            height,
        )
    }
    /// A normalized rectangle of the stored picture, in display coordinates.
    pub fn region_to_display(self, region: Region) -> Region {
        let (swap, mirror_x, mirror_y) = self.parts();
        let x = if mirror_x {
            (1.0 - region.x - region.width).max(0.0)
        } else {
            region.x
        };
        let y = if mirror_y {
            (1.0 - region.y - region.height).max(0.0)
        } else {
            region.y
        };
        if swap {
            Region {
                x: y,
                y: x,
                width: region.height,
                height: region.width,
            }
        } else {
            Region {
                x,
                y,
                width: region.width,
                height: region.height,
            }
        }
    }
    /// Turn a packed RGB24 picture from stored to display orientation.
    pub fn apply_rgb(self, pixels: Vec<u8>, width: u32, height: u32) -> (Vec<u8>, u32, u32) {
        if self == Self::None {
            return (pixels, width, height);
        }
        let (swap, mirror_x, mirror_y) = self.parts();
        let (stored_width, stored_height) = (width as usize, height as usize);
        let (display_width, display_height) = self.display_size(width, height);
        let mut output = vec![0; pixels.len()];
        let mut to = 0;
        for y in 0..display_height as usize {
            for x in 0..display_width as usize {
                let (u, v) = if swap { (y, x) } else { (x, y) };
                let u = if mirror_x { stored_width - 1 - u } else { u };
                let v = if mirror_y { stored_height - 1 - v } else { v };
                let from = (v * stored_width + u) * 3;
                output[to..to + 3].copy_from_slice(&pixels[from..from + 3]);
                to += 3;
            }
        }
        (output, display_width, display_height)
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayTransform as T;
    use crate::Region;

    const ALL: [T; 8] = [
        T::None,
        T::Rotate90Clockwise,
        T::Rotate180,
        T::Rotate90Counterclockwise,
        T::FlipHorizontal,
        T::FlipVertical,
        T::Transpose,
        T::Transverse,
    ];
    /// Packed RGB in which every pixel is unique: (stored x, stored y, tag).
    fn picture(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 7]);
            }
        }
        pixels
    }
    /// Rows of letters for a turned 3x2 picture whose stored rows are "abc"/"def".
    fn letters(pixels: &[u8], width: u32) -> Vec<String> {
        pixels
            .as_chunks::<3>()
            .0
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|p| ["abc", "def"][p[1] as usize].as_bytes()[p[0] as usize] as char)
                    .collect()
            })
            .collect()
    }
    fn crop(pixels: &[u8], width: u32, (x, y, w, h): (u32, u32, u32, u32)) -> Vec<u8> {
        let mut out = Vec::new();
        for row in y..y + h {
            let from = ((row * width + x) * 3) as usize;
            out.extend_from_slice(&pixels[from..from + w as usize * 3]);
        }
        out
    }

    #[test]
    fn transforms_turn_pictures_as_named() {
        // Expected layouts drawn by hand from the visual meaning of each name.
        for (transform, expected) in [
            (T::None, vec!["abc", "def"]),
            (T::Rotate90Clockwise, vec!["da", "eb", "fc"]),
            (T::Rotate180, vec!["fed", "cba"]),
            (T::Rotate90Counterclockwise, vec!["cf", "be", "ad"]),
            (T::FlipHorizontal, vec!["cba", "fed"]),
            (T::FlipVertical, vec!["def", "abc"]),
            (T::Transpose, vec!["ad", "be", "cf"]),
            (T::Transverse, vec!["fc", "eb", "da"]),
        ] {
            let (pixels, width, height) = transform.apply_rgb(picture(3, 2), 3, 2);
            assert_eq!(
                (width, height),
                (expected[0].len() as u32, expected.len() as u32)
            );
            assert_eq!(letters(&pixels, width), expected, "{transform:?}");
        }
    }

    #[test]
    fn display_crops_map_to_the_stored_pixels_they_show() {
        let (stored_width, stored_height) = (7, 4);
        let stored = picture(stored_width, stored_height);
        for transform in ALL {
            let (display, display_width, display_height) =
                transform.apply_rgb(stored.clone(), stored_width, stored_height);
            for rect in [
                (0, 0, 1, 1),
                (1, 2, 2, 1),
                (0, 1, 3, 2),
                (1, 0, 2, 4),
                (2, 1, 2, 3),
            ] {
                if rect.0 + rect.2 > display_width || rect.1 + rect.3 > display_height {
                    continue;
                }
                let source = transform.rect_to_stored(rect, (stored_width, stored_height));
                assert!(
                    source.0 + source.2 <= stored_width && source.1 + source.3 <= stored_height
                );
                // Cropping the stored picture and turning it must equal the display crop.
                let (turned, width, height) =
                    transform.apply_rgb(crop(&stored, stored_width, source), source.2, source.3);
                assert_eq!((width, height), (rect.2, rect.3), "{transform:?} {rect:?}");
                assert_eq!(
                    turned,
                    crop(&display, display_width, rect),
                    "{transform:?} {rect:?}"
                );
            }
        }
    }

    #[test]
    fn stored_regions_land_where_their_pixels_are_displayed() {
        // A tile-aligned stored region, like a change-scan grid result.
        let region = Region {
            x: 0.25,
            y: 0.125,
            width: 0.5,
            height: 0.25,
        };
        let size = 16u32;
        let mut stored = vec![0u8; (size * size * 3) as usize];
        let (x, y, w, h) = region.pixels(size, size);
        for row in y..y + h {
            for column in x..x + w {
                stored[((row * size + column) * 3) as usize] = 255;
            }
        }
        for transform in ALL {
            let (display, _, _) = transform.apply_rgb(stored.clone(), size, size);
            let marked: Vec<(u32, u32)> = display
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[0] == 255)
                .map(|(i, _)| (i as u32 % size, i as u32 / size))
                .collect();
            let left = marked.iter().map(|p| p.0).min().unwrap();
            let top = marked.iter().map(|p| p.1).min().unwrap();
            let right = marked.iter().map(|p| p.0).max().unwrap() + 1;
            let bottom = marked.iter().map(|p| p.1).max().unwrap() + 1;
            assert_eq!(
                transform.region_to_display(region).pixels(size, size),
                (left, top, right - left, bottom - top),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn display_matrices_follow_ffmpeg_autorotation() {
        // Q16.16 matrices as av_display_rotation_set/av_display_matrix_flip write
        // them for ffmpeg -display_rotation/-display_hflip/-display_vflip, with the
        // angle av_display_rotation_get returns; media.rs checks both natively.
        const ONE: i32 = 1 << 16;
        let matrix = |a: i32, b: i32, c: i32, d: i32| [a, b, 0, c, d, 0, 0, 0, 1 << 30];
        for (m, angle, expected) in [
            (matrix(ONE, 0, 0, ONE), 0.0, T::None),
            // iPhone portrait (ffprobe "rotation: -90") and -display_rotation -90.
            (matrix(0, ONE, -ONE, 0), -90.0, T::Rotate90Clockwise),
            (matrix(0, -ONE, ONE, 0), 90.0, T::Rotate90Counterclockwise),
            (matrix(-ONE, 0, 0, -ONE), -180.0, T::Rotate180),
            (matrix(-ONE, 0, 0, ONE), -180.0, T::FlipHorizontal),
            (matrix(ONE, 0, 0, -ONE), -0.0, T::FlipVertical),
            (matrix(0, ONE, ONE, 0), -90.0, T::Transpose),
            (matrix(0, -ONE, -ONE, 0), 90.0, T::Transverse),
        ] {
            assert_eq!(T::from_display_matrix(&m, angle), expected, "{m:?}");
        }
        let identity = matrix(ONE, 0, 0, ONE);
        assert_eq!(T::from_display_matrix(&identity, f64::NAN), T::None);
        assert_eq!(T::from_display_matrix(&identity, 45.0), T::None);
        // FFmpeg rounds to whole degrees and allows one degree; 270 equals -90.
        let portrait = matrix(0, ONE, -ONE, 0);
        assert_eq!(
            T::from_display_matrix(&portrait, -90.4),
            T::Rotate90Clockwise
        );
        assert_eq!(
            T::from_display_matrix(&portrait, 270.0),
            T::Rotate90Clockwise
        );
    }

    #[test]
    fn transform_names_and_sizes() {
        assert_eq!(
            serde_json::to_string(&T::Rotate90Clockwise).unwrap(),
            "\"rotate_90_clockwise\""
        );
        assert_eq!(serde_json::to_string(&T::None).unwrap(), "\"none\"");
        assert_eq!(T::Rotate90Clockwise.display_size(1920, 1080), (1080, 1920));
        assert_eq!(T::Rotate180.display_size(1920, 1080), (1920, 1080));
    }
}
