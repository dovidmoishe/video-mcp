//! Optional local OCR. These readings are evidence candidates, not ground truth.
use crate::Region;
#[cfg(feature = "native")]
use crate::control::Control;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TextWord {
    pub text: String,
    /// Normalized full-frame rectangle in display orientation (same space as crop).
    pub region: Region,
}
#[derive(Debug, Clone, Serialize)]
pub struct TextReading {
    pub status: String,
    pub language: Option<String>,
    pub words: Vec<TextWord>,
    pub truncated: bool,
}
#[cfg(feature = "native")]
pub(crate) fn read(
    pixels: &[u8],
    width: u32,
    height: u32,
    source_region: Region,
    control: &Control,
) -> anyhow::Result<TextReading> {
    control.check()?;
    #[cfg(windows)]
    {
        use std::{thread, time::Duration};
        use windows::{
            Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
            Media::Ocr::OcrEngine,
            Storage::Streams::DataWriter,
        };
        let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
            Ok(engine) => engine,
            Err(e) => {
                return Ok(TextReading {
                    status: format!("unavailable: {e}"),
                    language: None,
                    words: Vec::new(),
                    truncated: false,
                });
            }
        };
        anyhow::ensure!(
            width <= OcrEngine::MaxImageDimension()? && height <= OcrEngine::MaxImageDimension()?,
            "OCR dimensions exceed Windows engine limits"
        );
        let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
        for rgb in pixels.as_chunks::<3>().0 {
            bgra.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
        )?;
        let operation = engine.RecognizeAsync(&bitmap)?;
        // Poll only inside spawn_blocking. On cancellation signal WinRT, then
        // retain bitmap/operation and the worker permit until native work settles.
        while operation.Status()?.0 == 0 {
            if control.check().is_err() {
                let _ = operation.Cancel();
            }
            thread::sleep(Duration::from_millis(5));
        }
        control.check()?;
        let result = operation.GetResults()?;
        let mut words = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        'lines: for line in result.Lines()? {
            for word in line.Words()? {
                control.check()?;
                let text = word.Text()?.to_string();
                if words.len() >= 512 || bytes + text.len() > 32 * 1024 {
                    truncated = true;
                    break 'lines;
                }
                bytes += text.len();
                let rect = word.BoundingRect()?;
                let region = Region {
                    x: source_region.x + f64::from(rect.X) / f64::from(width) * source_region.width,
                    y: source_region.y
                        + f64::from(rect.Y) / f64::from(height) * source_region.height,
                    width: f64::from(rect.Width) / f64::from(width) * source_region.width,
                    height: f64::from(rect.Height) / f64::from(height) * source_region.height,
                };
                words.push(TextWord { text, region });
            }
        }
        Ok(TextReading {
            status: "recognized_unverified".into(),
            language: Some(engine.RecognizerLanguage()?.LanguageTag()?.to_string()),
            words,
            truncated,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (pixels, width, height, source_region);
        Ok(TextReading {
            status: "unavailable: Windows OCR is not built on this platform".into(),
            language: None,
            words: Vec::new(),
            truncated: false,
        })
    }
}
