//! Persistent, bounded temporal viewing. A filmstrip is sampled visual evidence,
//! not a claim that the model received or perceived continuous audiovisual input.
use crate::{Metadata, Transcript, Window};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WatchRequest {
    pub source: Option<String>,
    pub video_id: Option<String>,
    pub start: Option<f64>,
    #[serde(default = "seconds_default")]
    #[schemars(range(min = 0.25, max = 30.0))]
    pub seconds: f64,
    #[serde(default = "fps_default")]
    #[schemars(range(min = 0.5, max = 30.0))]
    pub fps: f64,
    #[serde(default = "width_default")]
    #[schemars(range(min = 160, max = 640))]
    pub width: u32,
    /// Automatic event evidence: 0 disables; each event adds up to three samples.
    #[serde(default = "event_default")]
    #[schemars(range(min = 0, max = 4))]
    pub event_budget: usize,
    #[serde(default = "speech_default")]
    pub include_speech: bool,
    pub language: Option<String>,
}
fn event_default() -> usize {
    2
}
fn seconds_default() -> f64 {
    15.0
}
fn fps_default() -> f64 {
    1.0
}
fn width_default() -> u32 {
    320
}
fn speech_default() -> bool {
    false
}
impl Default for WatchRequest {
    fn default() -> Self {
        Self {
            source: None,
            video_id: None,
            start: None,
            seconds: 15.0,
            fps: 1.0,
            width: 320,
            event_budget: 2,
            include_speech: false,
            language: None,
        }
    }
}
impl WatchRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.source.is_some() != self.video_id.is_some(),
            "provide exactly one of source or video_id"
        );
        ensure!(
            self.seconds.is_finite() && (0.25..=30.0).contains(&self.seconds),
            "seconds must be 0.25..30"
        );
        ensure!(
            self.fps.is_finite() && (0.5..=30.0).contains(&self.fps),
            "fps must be 0.5..30; keep seconds*fps <= 16 (e.g. 0.5s at 30fps)"
        );
        ensure!(
            (self.seconds * self.fps).ceil() <= 16.0,
            "a viewing window may contain at most 16 samples; narrow seconds or lower fps"
        );
        ensure!((160..=640).contains(&self.width), "width must be 160..640");
        ensure!(
            self.event_budget <= 4,
            "event_budget must be 0..4; set 0 for the cheapest overview"
        );
        if let Some(start) = self.start {
            Window {
                start,
                end: start + self.seconds,
            }
            .validate()?;
        }
        if let Some(language) = &self.language {
            ensure!(
                language.len() <= 16
                    && language
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || c == '-'),
                "invalid language code"
            );
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub timestamp: f64,
    pub estimated_timestamp: bool,
    /// Mean absolute RGB difference from the previous sampled image, normalized
    /// to 0..1. Includes camera movement, cuts, lighting and compression changes.
    pub visual_change: Option<f64>,
    pub mean_luma: f64,
    pub near_black: bool,
    pub width: u32,
    pub height: u32,
    /// Pixel rectangle in the returned filmstrip, excluding the timestamp header.
    pub image_rect: [u32; 4],
}
#[derive(Debug, Clone, Serialize)]
pub struct ChangeCandidate {
    pub timestamp: f64,
    pub estimated_timestamp: bool,
    pub score: f64,
    /// Normalized full-frame rectangle in display orientation, usable as a crop.
    pub region: crate::Region,
    /// Time range (seconds) worth inspecting around the candidate.
    pub suggested_window: Window,
}
#[derive(Debug, Clone, Serialize)]
pub struct EventObservation {
    pub candidate_timestamp: f64,
    pub requested_timestamp: f64,
    pub frame: Observation,
}
#[derive(Debug, Clone, Serialize)]
pub struct WatchPage {
    pub video_id: String,
    pub metadata: Option<Metadata>,
    pub window: Window,
    pub next_start: Option<f64>,
    pub sample_interval_seconds: f64,
    pub frames: Vec<Observation>,
    pub columns: u32,
    pub width: u32,
    pub height: u32,
    pub cache_hit: bool,
    pub decoded_frames: u64,
    pub seek_performed: bool,
    pub change_candidates: Vec<ChangeCandidate>,
    pub change_candidates_truncated: bool,
    pub scanned_until: Option<f64>,
    pub transcript: Option<Transcript>,
    /// Requested inference context; overlapping segments retain their original
    /// timestamps and text, which can extend beyond this visual window.
    pub transcript_context: Option<Window>,
    pub audio_status: String,
    pub event_frames: Vec<EventObservation>,
    pub event_evidence_truncated: bool,
    pub processing_ms: u128,
    #[serde(skip)]
    pub event_jpeg: Vec<u8>,
    pub sampling_note: &'static str,
    #[serde(skip)]
    pub jpeg: Vec<u8>,
}

#[cfg(feature = "native")]
mod native {
    use super::*;
    use crate::{
        FrameOptions, ImageFormat, control::Control, media, source::Source, video::VideoReader,
    };
    use anyhow::Context;
    use std::{
        collections::{HashMap, VecDeque},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant, SystemTime},
    };

    const TTL: Duration = Duration::from_secs(600);
    const MAX_SESSIONS: usize = 4;
    #[derive(Clone, PartialEq)]
    struct PageKey {
        start: f64,
        end: f64,
        fps: f64,
        width: u32,
        event_budget: usize,
        speech: bool,
        language: Option<String>,
    }
    struct Session {
        // Declaration order releases decoder/file handles before deleting a URL's directory.
        reader: VideoReader,
        source: Source,
        metadata: Metadata,
        fingerprint: (u64, Option<SystemTime>),
        playhead: f64,
        touched: Instant,
        page: Option<(PageKey, WatchPage)>,
        transcripts: VecDeque<(f64, Option<String>, Transcript)>,
        details: Option<(crate::detail::DetailRequest, crate::detail::DetailPage)>,
    }
    #[derive(Default)]
    pub(crate) struct Sessions {
        entries: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
        next: AtomicU64,
        #[cfg(feature = "whisper")]
        speech: Option<Arc<crate::speech::Transcriber>>,
    }
    impl Sessions {
        #[cfg(feature = "whisper")]
        pub fn with_speech(speech: Option<Arc<crate::speech::Transcriber>>) -> Self {
            Self {
                speech,
                ..Default::default()
            }
        }
        fn prune(&self) -> Result<()> {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?;
            entries.retain(|_, session| match session.try_lock() {
                Ok(s) => s.touched.elapsed() < TTL,
                Err(std::sync::TryLockError::WouldBlock) => true,
                Err(std::sync::TryLockError::Poisoned(_)) => false,
            });
            Ok(())
        }
        pub fn open(
            &self,
            source: Source,
            request: &WatchRequest,
            control: &Control,
        ) -> Result<WatchPage> {
            self.prune()?;
            ensure!(
                self.entries
                    .lock()
                    .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?
                    .len()
                    < MAX_SESSIONS,
                "four viewing sessions are already open; close_video first"
            );
            let metadata = media::inspect(&source.path, control)?;
            let start = request.start.unwrap_or(0.0);
            let reader = VideoReader::open(&source.path, start, control)?;
            let fingerprint = fingerprint(&source)?;
            let mut session = Session {
                reader,
                source,
                metadata,
                fingerprint,
                playhead: start,
                touched: Instant::now(),
                page: None,
                transcripts: VecDeque::new(),
                details: None,
            };
            let id = format!(
                "video-{:x}-{:016x}",
                std::process::id(),
                self.next.fetch_add(1, Ordering::Relaxed)
            );
            let mut page = self.render(&mut session, &id, request, control)?;
            page.metadata = Some(session.metadata.clone());
            // Include initial metadata in a repeated first-page response too.
            if let Some((_, cached)) = session.page.as_mut() {
                cached.metadata = page.metadata.clone();
            }
            control.check()?;
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?;
            ensure!(
                entries.len() < MAX_SESSIONS,
                "viewing session capacity reached"
            );
            entries.insert(id, Arc::new(Mutex::new(session)));
            Ok(page)
        }
        pub fn watch(
            &self,
            id: &str,
            request: &WatchRequest,
            control: &Control,
        ) -> Result<WatchPage> {
            self.prune()?;
            let session = self
                .entries
                .lock()
                .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?
                .get(id)
                .cloned()
                .context("unknown or expired video_id; reopen using source")?;
            let mut session = session
                .try_lock()
                .map_err(|_| anyhow::anyhow!("this video is busy; retry after the current view"))?;
            ensure!(
                fingerprint(&session.source)? == session.fingerprint,
                "source file changed; close and reopen the video"
            );
            self.render(&mut session, id, request, control)
        }
        pub fn details(
            &self,
            id: &str,
            request: &crate::detail::DetailRequest,
            control: &Control,
        ) -> Result<crate::detail::DetailPage> {
            self.prune()?;
            let session = self
                .entries
                .lock()
                .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?
                .get(id)
                .cloned()
                .context("unknown or expired video_id; reopen using source")?;
            let mut session = session
                .try_lock()
                .map_err(|_| anyhow::anyhow!("this video is busy; retry after the current view"))?;
            ensure!(
                fingerprint(&session.source)? == session.fingerprint,
                "source file changed; close and reopen the video"
            );
            session.touched = Instant::now();
            control.check()?;
            if let Some((previous, page)) = &session.details
                && previous == request
            {
                let mut page = page.clone();
                page.cache_hit = true;
                page.decoded_frames = 0;
                page.processing_ms = 0;
                return Ok(page);
            }
            let metadata = session.metadata.clone();
            let page = crate::detail::extract(&mut session.reader, &metadata, request, control)?;
            control.check()?;
            session.details = Some((request.clone(), page.clone()));
            Ok(page)
        }
        pub fn close(&self, id: &str) -> Result<bool> {
            Ok(self
                .entries
                .lock()
                .map_err(|_| anyhow::anyhow!("viewing session registry poisoned"))?
                .remove(id)
                .is_some())
        }
        fn render(
            &self,
            session: &mut Session,
            id: &str,
            request: &WatchRequest,
            control: &Control,
        ) -> Result<WatchPage> {
            let started = Instant::now();
            let start = request.start.unwrap_or(session.playhead);
            let end = session
                .metadata
                .duration_seconds
                .map_or(start + request.seconds, |duration| {
                    (start + request.seconds).min(duration)
                });
            let window = Window { start, end };
            window.validate()?;
            let key = PageKey {
                start,
                end,
                fps: request.fps,
                width: request.width,
                event_budget: request.event_budget,
                speech: request.include_speech,
                language: request.language.clone(),
            };
            session.touched = Instant::now();
            if let Some((previous, page)) = &session.page
                && previous.start == key.start
                && previous.end == key.end
                && previous.fps == key.fps
                && previous.width == key.width
                && previous.event_budget == key.event_budget
            {
                let mut page = page.clone();
                if previous.speech != key.speech || previous.language != key.language {
                    let (transcript, status) = self.audio(session, &window, request, control);
                    page.transcript_context = transcript
                        .as_ref()
                        .map(|_| transcription_context(&session.metadata, &window));
                    page.transcript = transcript;
                    page.audio_status = status;
                    control.check()?;
                    session.page = Some((key, page.clone()));
                }
                page.processing_ms = started.elapsed().as_millis();
                page.cache_hit = true;
                page.decoded_frames = 0;
                page.seek_performed = false;
                return Ok(page);
            }
            let video = session
                .metadata
                .streams
                .iter()
                .find(|s| s.kind == "video")
                .context("no video stream")?;
            // Display size: a portrait phone clip stored 1920x1080 with a display
            // matrix is shown, and sampled, as 1080x1920.
            let source_width = video.display_width.or(video.width).unwrap_or(16);
            let source_height = video.display_height.or(video.height).unwrap_or(9);
            // Width is the source-image width, not a landscape cell. Tall media keeps
            // its aspect ratio; the long edge remains bounded at 1280 pixels.
            let max_height = (f64::from(request.width) * f64::from(source_height)
                / f64::from(source_width.max(1)))
            .ceil()
            .clamp(1.0, 1280.0) as u32;
            let options = FrameOptions {
                window: window.clone(),
                count: ((end - start) * request.fps).ceil() as usize,
                interval: Some(1.0 / request.fps),
                max_width: request.width,
                max_height,
                format: ImageFormat::Jpeg,
                crop: None,
            };
            let mut samples = Vec::new();
            let stats = session.reader.scan(&options, control, |sample| {
                samples.push(sample);
                Ok(())
            })?;
            let Filmstrip {
                jpeg,
                frames,
                columns,
                width,
                height,
            } = filmstrip(&samples, request.width, control)?;
            let mut candidates = stats.changes.clone();
            candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
            let mut selected: Vec<ChangeCandidate> = Vec::new();
            for candidate in &candidates {
                if selected.len() >= request.event_budget {
                    break;
                }
                if selected
                    .iter()
                    .all(|p| (p.timestamp - candidate.timestamp).abs() >= 0.30)
                {
                    selected.push(candidate.clone());
                }
            }
            selected.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
            let mut event_decoded_frames = 0;
            let mut event_samples = Vec::new();
            let mut event_times = Vec::new();
            for candidate in &selected {
                for offset in [-0.10, 0.0, 0.10] {
                    control.check()?;
                    let time = (candidate.timestamp + offset).max(start);
                    if time >= end
                        || event_times
                            .last()
                            .is_some_and(|(_, t): &(f64, f64)| (*t - time).abs() < 0.001)
                    {
                        continue;
                    }
                    let focus = FrameOptions {
                        window: Window {
                            start: time,
                            end: (time + 0.25).min(end),
                        },
                        count: 1,
                        interval: None,
                        ..options.clone()
                    };
                    let focus_stats = session.reader.sample(&focus, control, |sample| {
                        event_times.push((candidate.timestamp, time));
                        event_samples.push(sample);
                        Ok(())
                    })?;
                    event_decoded_frames += focus_stats.decoded_frames;
                }
            }
            let Filmstrip {
                jpeg: event_jpeg,
                frames: event_observations,
                ..
            } = filmstrip(&event_samples, request.width, control)?;
            let event_frames = event_observations
                .into_iter()
                .zip(event_times)
                .map(
                    |(frame, (candidate_timestamp, requested_timestamp))| EventObservation {
                        candidate_timestamp,
                        requested_timestamp,
                        frame,
                    },
                )
                .collect();
            let event_evidence_truncated = selected.len() < candidates.len();
            let (transcript, audio_status) = self.audio(session, &window, request, control);
            let transcript_context = transcript
                .as_ref()
                .map(|_| transcription_context(&session.metadata, &window));
            control.check()?;
            session.playhead = end;
            let finished = session.reader.eof
                || session
                    .metadata
                    .duration_seconds
                    .is_some_and(|duration| end >= duration);
            let page = WatchPage {
                video_id: id.into(),
                metadata: None,
                window,
                next_start: (!finished).then_some(end),
                sample_interval_seconds: 1.0 / request.fps,
                frames,
                columns,
                width,
                height,
                cache_hit: false,
                decoded_frames: stats.decoded_frames + event_decoded_frames,
                seek_performed: stats.seek_performed,
                change_candidates: stats.changes,
                change_candidates_truncated: stats.changes_truncated,
                scanned_until: stats.scanned_until,
                transcript,
                transcript_context,
                audio_status,
                event_frames,
                event_evidence_truncated,
                processing_ms: started.elapsed().as_millis(),
                event_jpeg,
                sampling_note: "First image: periodic overview in row-major order. Optional second image: bounded samples around strongest change candidates; event_frames maps actual timestamps and pixel rectangles. Requested times can differ from actual VFR frames. A 96x96 RGB regional/rolling-baseline scan supplies hints, not semantic event labels or guaranteed coverage. Use inspect_video_frames(video_id,timestamps,crop,include_ocr) to resolve small text or uncertain motion; crop coordinates and change_candidates regions are normalized full-frame coordinates in display orientation (rotation metadata applied, as images appear), not filmstrip pixels. event_budget=0 disables automatic event images. Samples do not constitute continuous perception.",
                jpeg,
            };
            session.page = Some((key, page.clone()));
            Ok(page)
        }
        fn audio(
            &self,
            session: &mut Session,
            window: &Window,
            request: &WatchRequest,
            control: &Control,
        ) -> (Option<Transcript>, String) {
            if !request.include_speech {
                return (None, "not_requested".into());
            }
            if !session.metadata.streams.iter().any(|s| s.kind == "audio") {
                return (None, "no_audio_stream".into());
            }
            #[cfg(not(feature = "whisper"))]
            {
                let _ = (window, control);
                (None, "whisper_not_built".into())
            }
            #[cfg(feature = "whisper")]
            {
                let Some(speech) = self.speech.as_ref() else {
                    return (None, "model_unconfigured".into());
                };
                let result = (|| -> Result<Transcript> {
                    let mut segments = Vec::new();
                    let mut truncated = false;
                    let mut text_bytes = 0;
                    let mut start = (window.start / 30.0).floor() * 30.0;
                    while start < window.end {
                        control.check()?;
                        let cached = session.transcripts.iter().position(|(time, language, _)| {
                            *time == start && *language == request.language
                        });
                        let transcript = if let Some(index) = cached {
                            session.transcripts[index].2.clone()
                        } else {
                            let end = session
                                .metadata
                                .duration_seconds
                                .map_or(start + 30.0, |d| (start + 30.0).min(d));
                            let transcript = speech.transcribe(
                                &session.source.path,
                                &Window { start, end },
                                request.language.as_deref(),
                                control,
                            )?;
                            if session.transcripts.len() == 2 {
                                session.transcripts.pop_front();
                            }
                            session.transcripts.push_back((
                                start,
                                request.language.clone(),
                                transcript.clone(),
                            ));
                            transcript
                        };
                        truncated |= transcript.truncated;
                        for segment in transcript
                            .segments
                            .iter()
                            .filter(|s| s.end > window.start && s.start < window.end)
                        {
                            if segments.len() >= 512 || text_bytes + segment.text.len() > 64 * 1024
                            {
                                truncated = true;
                                break;
                            }
                            text_bytes += segment.text.len();
                            // Clipping timestamps without word alignment would
                            // incorrectly assign the full sentence to a shorter view.
                            segments.push(segment.clone());
                        }
                        start += 30.0;
                    }
                    Ok(Transcript {
                        speech_detected: crate::speech::has_transcribed_speech(&segments),
                        segments,
                        truncated,
                    })
                })();
                match result {
                    Ok(transcript) => (Some(transcript), "transcribed".into()),
                    Err(e) => (None, format!("transcription_failed: {e:#}")),
                }
            }
        }
    }
    fn fingerprint(source: &Source) -> Result<(u64, Option<SystemTime>)> {
        let m = std::fs::metadata(&source.path)?;
        Ok((m.len(), m.modified().ok()))
    }
    fn transcription_context(metadata: &Metadata, window: &Window) -> Window {
        Window {
            start: (window.start / 30.0).floor() * 30.0,
            end: metadata
                .duration_seconds
                .map_or((window.end / 30.0).ceil() * 30.0, |duration| {
                    ((window.end / 30.0).ceil() * 30.0).min(duration)
                }),
        }
    }

    #[derive(Default)]
    struct Filmstrip {
        jpeg: Vec<u8>,
        frames: Vec<Observation>,
        columns: u32,
        width: u32,
        height: u32,
    }
    fn filmstrip(
        samples: &[crate::video::RgbSample],
        cell_width: u32,
        control: &Control,
    ) -> Result<Filmstrip> {
        if samples.is_empty() {
            return Ok(Filmstrip::default());
        }
        let columns = (if cell_width > 320 { 2 } else { 4 }).min(samples.len() as u32);
        let cell_height = samples.iter().map(|s| s.height).max().unwrap() + 22;
        let width = columns * cell_width;
        let height = (samples.len() as u32).div_ceil(columns) * cell_height;
        ensure!(
            u64::from(width) * u64::from(height) <= 12_000_000,
            "filmstrip exceeds 12 megapixels; reduce width or sample count"
        );
        let mut canvas = image::RgbImage::new(width, height);
        let mut observations = Vec::new();
        let mut previous: Option<&[u8]> = None;
        for (i, sample) in samples.iter().enumerate() {
            control.check()?;
            let difference = previous
                .filter(|p| p.len() == sample.pixels.len())
                .map(|p| {
                    p.iter()
                        .zip(&sample.pixels)
                        .map(|(a, b)| f64::from(a.abs_diff(*b)))
                        .sum::<f64>()
                        / (sample.pixels.len() as f64 * 255.0)
                });
            previous = Some(&sample.pixels);
            let luma = sample
                .pixels
                .chunks_exact(3)
                .map(|p| u64::from(p[0]) * 54 + u64::from(p[1]) * 183 + u64::from(p[2]) * 19)
                .sum::<u64>() as f64
                / (f64::from(sample.width) * f64::from(sample.height) * 256.0);
            let x = (i as u32 % columns) * cell_width;
            let y = (i as u32 / columns) * cell_height;
            label(
                &mut canvas,
                x + 5,
                y + 3,
                &format!("{i:02} {:06.2}", sample.timestamp),
            );
            let image =
                image::RgbImage::from_raw(sample.width, sample.height, sample.pixels.clone())
                    .context("invalid RGB sample")?;
            let image_x = x + (cell_width - sample.width) / 2;
            image::imageops::replace(&mut canvas, &image, i64::from(image_x), i64::from(y + 22));
            observations.push(Observation {
                timestamp: sample.timestamp,
                estimated_timestamp: sample.estimated,
                visual_change: difference,
                mean_luma: luma,
                near_black: luma < 2.0,
                width: sample.width,
                height: sample.height,
                image_rect: [image_x, y + 22, sample.width, sample.height],
            });
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85).encode(
            canvas.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )?;
        ensure!(
            jpeg.len() <= 2 * 1024 * 1024,
            "filmstrip exceeds 2 MiB; reduce width or sample count"
        );
        Ok(Filmstrip {
            jpeg,
            frames: observations,
            columns,
            width,
            height,
        })
    }
    // Tiny numeric glyphs keep filmstrip timestamps portable without bundling a font.
    fn label(canvas: &mut image::RgbImage, x: u32, y: u32, text: &str) {
        const DIGITS: [[u8; 7]; 10] = [
            [14, 17, 19, 21, 25, 17, 14],
            [4, 12, 4, 4, 4, 4, 14],
            [14, 17, 1, 2, 4, 8, 31],
            [30, 1, 1, 14, 1, 1, 30],
            [2, 6, 10, 18, 31, 2, 2],
            [31, 16, 16, 30, 1, 1, 30],
            [14, 16, 16, 30, 17, 17, 14],
            [31, 1, 2, 4, 8, 8, 8],
            [14, 17, 17, 14, 17, 17, 14],
            [14, 17, 17, 15, 1, 1, 14],
        ];
        for (column, ch) in text.chars().enumerate() {
            let glyph = match ch {
                '0'..='9' => DIGITS[ch as usize - '0' as usize],
                '.' => [0, 0, 0, 0, 0, 4, 4],
                _ => [0; 7],
            };
            for (row, bits) in glyph.iter().enumerate() {
                for bit in 0..5 {
                    if bits & (1 << (4 - bit)) != 0 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let px = x + column as u32 * 12 + bit * 2 + dx;
                                let py = y + row as u32 * 2 + dy;
                                if px < canvas.width() && py < canvas.height() {
                                    canvas.put_pixel(px, py, image::Rgb([225, 225, 225]));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
#[cfg(feature = "native")]
pub(crate) use native::Sessions;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewing_requests_are_bounded() {
        let mut request = WatchRequest {
            source: Some("clip.mp4".into()),
            ..Default::default()
        };
        request.validate().unwrap();
        request.fps = 8.0;
        assert!(request.validate().is_err());
        request.seconds = 2.0;
        request.validate().unwrap();
        request.video_id = Some("both".into());
        assert!(request.validate().is_err());
        request.video_id = None;
        request.start = Some(f64::NAN);
        assert!(request.validate().is_err());
    }
}
