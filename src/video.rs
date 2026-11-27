//! A seekable decoder retained across bounded viewing requests. SwsContext stays
//! local to each request because its wrapper is not Send; no unsafe Send is added.
use crate::{
    FrameOptions,
    control::Control,
    media,
    orientation::DisplayTransform,
    timestamp::{Clock, seconds},
};
use anyhow::{Context, Result, ensure};
use ff::{Packet, codec, format, frame, media::Type, software, util::format::Pixel};
use ffmpeg_next as ff;
use std::path::Path;

/// A rendered frame in display orientation (display matrix applied).
pub(crate) struct RgbSample {
    pub timestamp: f64,
    pub estimated: bool,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Full-resolution frame size in display orientation.
    pub source_width: u32,
    pub source_height: u32,
}
pub(crate) struct SampleStats {
    pub decoded_frames: u64,
    pub seek_performed: bool,
    pub changes: Vec<crate::watch::ChangeCandidate>,
    pub changes_truncated: bool,
    pub scanned_until: Option<f64>,
}
pub(crate) struct VideoReader {
    input: format::context::Input,
    decoder: ff::decoder::Video,
    decoded: frame::Video,
    index: usize,
    tb: ff::Rational,
    origin: f64,
    fps: f64,
    clock: Clock,
    position: Option<(f64, bool)>,
    anchor: f64,
    pending: bool,
    draining: bool,
    pub eof: bool,
    pub decoded_frames: u64,
    packets: u64,
    initial_seek: bool,
    previous_end: f64,
    detector: crate::visual::ChangeDetector,
    /// Stored-to-display transform from the stream's display matrix.
    transform: DisplayTransform,
}
impl VideoReader {
    pub fn open(path: &Path, start: f64, control: &Control) -> Result<Self> {
        let mut input = media::open(path, control)?;
        let origin = media::origin(&input);
        let stream = input
            .streams()
            .best(Type::Video)
            .context("no video stream")?;
        let index = stream.index();
        let tb = stream.time_base();
        let fps = media::rate(stream.avg_frame_rate()).unwrap_or(25.0);
        let (_, transform) = media::display_orientation(&stream);
        let mut context = codec::context::Context::from_parameters(stream.parameters())?;
        context.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Frame,
            count: 2,
        });
        let mut decoder = context.decoder().video()?;
        media::seek(&mut input, start, origin)?;
        decoder.flush();
        Ok(Self {
            input,
            decoder,
            decoded: frame::Video::empty(),
            index,
            tb,
            origin,
            fps,
            clock: Clock::new(1.0 / fps, start),
            position: None,
            anchor: start,
            pending: false,
            draining: false,
            eof: false,
            decoded_frames: 0,
            packets: 0,
            initial_seek: start > 0.0,
            previous_end: start,
            detector: Default::default(),
            transform,
        })
    }
    fn reset(&mut self, start: f64) -> Result<()> {
        // Unlike initial open, rewinding to zero must also perform a real seek.
        let ts = ((start + self.origin) * f64::from(ff::ffi::AV_TIME_BASE)) as i64;
        self.input
            .seek(ts, ..ts)
            .context("seek video viewing session")?;
        self.decoder.flush();
        self.clock = Clock::new(1.0 / self.fps, start);
        self.position = None;
        self.anchor = start;
        self.pending = false;
        self.draining = false;
        self.eof = false;
        self.previous_end = start;
        self.detector = Default::default();
        Ok(())
    }
    fn advance(&mut self, control: &Control, packet_start: u64) -> Result<bool> {
        loop {
            control.check()?;
            match self.decoder.receive_frame(&mut self.decoded) {
                Ok(()) => {
                    ensure!(
                        self.decoded.width() > 0
                            && self.decoded.height() > 0
                            && u64::from(self.decoded.width()) * u64::from(self.decoded.height())
                                <= 33_177_600,
                        "invalid or excessive decoded dimensions"
                    );
                    self.decoded_frames += 1;
                    self.position = Some(self.clock.next(seconds(
                        self.decoded.timestamp().or(self.decoded.pts()),
                        self.tb.numerator(),
                        self.tb.denominator(),
                        self.origin,
                    )));
                    return Ok(true);
                }
                Err(ff::Error::Eof) => {
                    self.eof = true;
                    return Ok(false);
                }
                Err(e) if media::again(e) => {}
                Err(e) => return Err(e).context("receive video frame"),
            }
            ensure!(
                !self.draining,
                "decoder requested more input after EOF submission"
            );
            // receive_frame returned EAGAIN, so submit one selected packet and
            // immediately return to receive_frame. Never drop an unsubmitted packet.
            loop {
                control.check()?;
                self.packets += 1;
                ensure!(
                    self.packets - packet_start <= 2_000_000,
                    "video packet budget exceeded"
                );
                let mut packet = Packet::empty();
                match packet.read(&mut self.input) {
                    Ok(()) if packet.stream() == self.index => {
                        self.decoder
                            .send_packet(&packet)
                            .context("submit video packet after draining")?;
                        break;
                    }
                    Ok(()) => continue,
                    Err(ff::Error::Eof) => {
                        self.decoder.send_eof()?;
                        self.draining = true;
                        break;
                    }
                    Err(e) => return Err(e).context("read video packet"),
                }
            }
        }
    }
    pub fn sample(
        &mut self,
        options: &FrameOptions,
        control: &Control,
        visit: impl FnMut(RgbSample) -> Result<()>,
    ) -> Result<SampleStats> {
        self.visit(options, control, false, visit)
    }
    pub fn scan(
        &mut self,
        options: &FrameOptions,
        control: &Control,
        visit: impl FnMut(RgbSample) -> Result<()>,
    ) -> Result<SampleStats> {
        self.visit(options, control, true, visit)
    }
    fn visit(
        &mut self,
        options: &FrameOptions,
        control: &Control,
        scan_changes: bool,
        mut visit: impl FnMut(RgbSample) -> Result<()>,
    ) -> Result<SampleStats> {
        options.validate()?;
        control.check()?;
        let before = self.decoded_frames;
        let packet_start = self.packets;
        let mut seek_performed = self.initial_seek;
        self.initial_seek = false;
        let start = options.window.start;
        if (start - self.previous_end).abs() > 1e-6 {
            self.detector = Default::default();
        }
        if self.position.is_some_and(|(t, _)| {
            (start < t - 1e-6 && (!self.pending || start < self.previous_end - 1e-6))
                || start > t + 5.0
        }) || (self.position.is_none() && (start - self.anchor).abs() > 1e-6)
        {
            self.reset(start)?;
            seek_performed = true;
        }
        let mut renderer = FrameRenderer::default();
        // A previous detail call may have returned this exact presentation frame.
        // Reuse it rather than silently advancing one frame beyond a repeated time.
        if self
            .position
            .is_some_and(|(time, _)| (time - start).abs() < 1e-6)
        {
            self.pending = true;
        }
        let mut next = start;
        let mut count = 0;
        let mut probe: Option<software::scaling::Context> = None;
        let mut probe_frame = frame::Video::empty();
        let mut changes: Vec<crate::watch::ChangeCandidate> = Vec::new();
        let mut changes_truncated = false;
        let mut scanned_until = None;
        while scan_changes || count < options.count {
            control.check()?;
            if self.pending {
                self.pending = false;
            } else if self.eof || !self.advance(control, packet_start)? {
                break;
            }
            let (timestamp, estimated) = self
                .position
                .context("decoded frame has no presentation position")?;
            if timestamp >= options.window.end {
                self.pending = true;
                break;
            }
            if scan_changes && timestamp >= start {
                let frame = &self.decoded;
                if probe.as_ref().is_none_or(|s| {
                    s.input().width != frame.width()
                        || s.input().height != frame.height()
                        || s.input().format != frame.format()
                }) {
                    probe = Some(software::scaling::Context::get(
                        frame.format(),
                        frame.width(),
                        frame.height(),
                        Pixel::RGB24,
                        crate::visual::PROBE_WIDTH,
                        crate::visual::PROBE_HEIGHT,
                        software::scaling::flag::Flags::FAST_BILINEAR,
                    )?);
                }
                probe.as_mut().unwrap().run(frame, &mut probe_frame)?;
                let mut signature = Vec::with_capacity(96 * 96 * 3);
                for row in 0..96 {
                    let offset = row * probe_frame.stride(0);
                    signature.extend_from_slice(&probe_frame.data(0)[offset..offset + 96 * 3]);
                }
                if let Some((score, region)) = self.detector.observe(timestamp, signature) {
                    // The probe covers the whole stored frame, so its grid region is
                    // stored-normalized; report it where the change is displayed.
                    let candidate = crate::watch::ChangeCandidate {
                        timestamp,
                        estimated_timestamp: estimated,
                        score,
                        region: self.transform.region_to_display(region),
                        suggested_window: crate::Window {
                            start: (timestamp - 0.15).max(start),
                            end: (timestamp + 0.25).min(options.window.end),
                        },
                    };
                    if let Some(last) = changes
                        .last_mut()
                        .filter(|e| timestamp - e.timestamp < 0.15)
                    {
                        if score > last.score {
                            *last = candidate;
                        }
                    } else if changes.len() < 64 {
                        changes.push(candidate);
                    } else {
                        changes_truncated = true;
                    }
                }
                scanned_until = Some(timestamp);
            }
            if count >= options.count || timestamp + 1e-6 < next {
                continue;
            }
            let sample =
                renderer.render(&self.decoded, self.transform, options, timestamp, estimated)?;
            visit(sample)?;
            count += 1;
            next = start + (((timestamp - start) / options.step()).floor() + 1.0) * options.step();
        }
        self.previous_end = options.window.end;
        Ok(SampleStats {
            decoded_frames: self.decoded_frames - before,
            seek_performed,
            changes,
            changes_truncated,
            scanned_until,
        })
    }
}

/// RGB conversion precedes a crop at native resolution. Only the selected region
/// is then resized by swscale; no thumbnail is enlarged and no unsafe plane slicing
/// is required for subsampled YUV formats. Contexts live on the blocking worker.
///
/// Crop and output-size decisions use display orientation. The display crop maps
/// to exact stored pixels; conversion, cropping and scaling run in stored
/// orientation at the swapped output size, and only the scaled result is turned
/// upright. This yields the geometry of turning the frame before scaling (as
/// FFmpeg's autorotate filter does) without transforming full-resolution frames.
#[derive(Default)]
struct FrameRenderer {
    converter: Option<software::scaling::Context>,
    scaler: Option<software::scaling::Context>,
}
impl FrameRenderer {
    fn render(
        &mut self,
        source: &frame::Video,
        transform: DisplayTransform,
        options: &FrameOptions,
        timestamp: f64,
        estimated: bool,
    ) -> Result<RgbSample> {
        let stored = (source.width(), source.height());
        let (source_width, source_height) = transform.display_size(stored.0, stored.1);
        let shown = options
            .crop
            .map(|region| region.pixels(source_width, source_height));
        let (shown_width, shown_height) =
            shown.map_or((source_width, source_height), |rect| (rect.2, rect.3));
        let ratio = (f64::from(options.max_width) / f64::from(shown_width))
            .min(f64::from(options.max_height) / f64::from(shown_height))
            .min(1.0);
        let width = (f64::from(shown_width) * ratio).round().max(1.0) as u32;
        let height = (f64::from(shown_height) * ratio).round().max(1.0) as u32;
        let mut crop_frame;
        let input = if let Some(rect) = shown {
            let mut full = frame::Video::empty();
            scale(&mut self.converter, source, stored.0, stored.1, &mut full)?;
            let (x, y, w, h) = transform.rect_to_stored(rect, stored);
            crop_frame = frame::Video::new(Pixel::RGB24, w, h);
            for row in 0..h as usize {
                let from = (y as usize + row) * full.stride(0) + x as usize * 3;
                let to = row * crop_frame.stride(0);
                crop_frame.data_mut(0)[to..to + w as usize * 3]
                    .copy_from_slice(&full.data(0)[from..from + w as usize * 3]);
            }
            &crop_frame
        } else {
            source
        };
        let (scaled_width, scaled_height) = transform.stored_size(width, height);
        let mut rgb = frame::Video::empty();
        scale(
            &mut self.scaler,
            input,
            scaled_width,
            scaled_height,
            &mut rgb,
        )?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..scaled_height as usize {
            let offset = y * rgb.stride(0);
            pixels.extend_from_slice(&rgb.data(0)[offset..offset + scaled_width as usize * 3]);
        }
        let (pixels, width, height) = transform.apply_rgb(pixels, scaled_width, scaled_height);
        Ok(RgbSample {
            timestamp,
            estimated,
            width,
            height,
            pixels,
            source_width,
            source_height,
        })
    }
}
fn scale(
    context: &mut Option<software::scaling::Context>,
    input: &frame::Video,
    width: u32,
    height: u32,
    output: &mut frame::Video,
) -> Result<()> {
    if context.as_ref().is_none_or(|s| {
        s.input().format != input.format()
            || s.input().width != input.width()
            || s.input().height != input.height()
            || s.output().width != width
            || s.output().height != height
    }) {
        *context = Some(software::scaling::Context::get(
            input.format(),
            input.width(),
            input.height(),
            Pixel::RGB24,
            width,
            height,
            software::scaling::flag::Flags::BILINEAR,
        )?);
    }
    context.as_mut().unwrap().run(input, output)?;
    Ok(())
}
