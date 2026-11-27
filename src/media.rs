//! FFmpeg owns native buffers. Only selected, scaled images leave this module.
use crate::{control::Control, timestamp::seconds, *};
use anyhow::{Context, Result, ensure};
use ff::{
    ChannelLayout, Packet, codec, format, frame, media::Type, software, util::format::Sample,
};
use ffmpeg_next as ff;
use std::{path::Path, sync::OnceLock, time::Instant};

pub(crate) fn open(path: &Path, control: &Control) -> Result<format::context::Input> {
    control.check()?;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| ff::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))?;
    let mut options = ff::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("probesize", "5000000");
    options.set("analyzeduration", "5000000");
    let input =
        format::input_with_dictionary(path, options).context("open media and discover streams")?;
    ensure!(input.nb_streams() <= 64, "too many streams");
    control.check()?;
    Ok(input)
}
pub(crate) fn origin(input: &format::context::Input) -> f64 {
    // SAFETY: read-only access to a live, exclusively owned format context. The
    // wrapper has no accessor for AVFormatContext.start_time (AV_TIME_BASE units).
    let start = unsafe { (*input.as_ptr()).start_time };
    seconds(Some(start), 1, ff::ffi::AV_TIME_BASE, 0.0).unwrap_or(0.0)
}
fn stream_seconds(ts: i64, tb: ff::Rational, origin: f64) -> Option<f64> {
    seconds(Some(ts), tb.numerator(), tb.denominator(), origin)
}
/// A stream's display matrix (`AV_PKT_DATA_DISPLAYMATRIX` in the codec
/// parameters' coded side data, where mov/mp4 export the track matrix) and the
/// transform FFmpeg's CLI autorotation applies for it. Returns the rotation as
/// ffprobe reports it (whole counterclockwise degrees, 0 without a matrix,
/// None for a singular matrix) and the transform.
pub(crate) fn display_orientation(
    stream: &format::stream::Stream,
) -> (Option<i32>, crate::orientation::DisplayTransform) {
    let matrix = stream.side_data().find_map(|side| {
        let data = side.data();
        (side.kind() == codec::packet::side_data::Type::DisplayMatrix && data.len() >= 36).then(
            || {
                let mut matrix = [0i32; 9];
                for (value, bytes) in matrix.iter_mut().zip(data.as_chunks::<4>().0) {
                    *value = i32::from_ne_bytes(*bytes);
                }
                matrix
            },
        )
    });
    let Some(matrix) = matrix else {
        return (Some(0), Default::default());
    };
    // SAFETY: av_display_rotation_get only reads nine int32 values; the pointer
    // refers to a live, fully initialized local array of exactly that length.
    let counterclockwise = unsafe { ff::ffi::av_display_rotation_get(matrix.as_ptr()) };
    (
        counterclockwise
            .is_finite()
            .then(|| counterclockwise.round() as i32),
        crate::orientation::DisplayTransform::from_display_matrix(&matrix, counterclockwise),
    )
}
pub fn inspect(path: &Path, control: &Control) -> Result<Metadata> {
    let started = Instant::now();
    let input = open(path, control)?;
    let base = origin(&input);
    let mut streams = Vec::new();
    for s in input.streams() {
        control.check()?;
        let p = s.parameters();
        let mut m = StreamMetadata {
            index: s.index(),
            kind: format!("{:?}", p.medium()).to_lowercase(),
            codec: p.id().name().into(),
            start_seconds: stream_seconds(s.start_time(), s.time_base(), base),
            duration_seconds: stream_seconds(s.duration(), s.time_base(), 0.0).filter(|d| *d > 0.0),
            width: None,
            height: None,
            rotation: None,
            display_transform: None,
            display_width: None,
            display_height: None,
            frame_rate: None,
            channels: None,
            sample_rate: None,
        };
        if p.medium() == Type::Video {
            let d = codec::context::Context::from_parameters(p)?
                .decoder()
                .video()?;
            let (rotation, transform) = display_orientation(&s);
            let (display_width, display_height) = transform.display_size(d.width(), d.height());
            m.width = Some(d.width());
            m.height = Some(d.height());
            m.rotation = rotation;
            m.display_transform = Some(transform);
            m.display_width = Some(display_width);
            m.display_height = Some(display_height);
            m.frame_rate = rate(s.avg_frame_rate());
        } else if p.medium() == Type::Audio {
            let d = codec::context::Context::from_parameters(p)?
                .decoder()
                .audio()?;
            m.channels = Some(d.channels());
            m.sample_rate = Some(d.rate());
        }
        streams.push(m);
    }
    tracing::info!(elapsed_ms = started.elapsed().as_millis(), "probe complete");
    Ok(Metadata {
        duration_seconds: seconds(Some(input.duration()), 1, ff::ffi::AV_TIME_BASE, 0.0)
            .filter(|d| *d > 0.0),
        format: input.format().name().into(),
        streams,
    })
}
pub(crate) fn rate(r: ff::Rational) -> Option<f64> {
    (r.numerator() > 0 && r.denominator() > 0)
        .then(|| f64::from(r))
        .filter(|r| r.is_finite() && *r > 0.0)
}
pub(crate) fn again(e: ff::Error) -> bool {
    matches!(e,ff::Error::Other { errno } if errno==ff::error::EAGAIN)
}
pub(crate) fn seek(input: &mut format::context::Input, start: f64, base: f64) -> Result<()> {
    if start > 0.0 {
        let ts = ((start + base) * f64::from(ff::ffi::AV_TIME_BASE)) as i64;
        input.seek(ts, ..ts).context("seek to preceding keyframe")?;
    }
    Ok(())
}
/// Extract bounded in-memory images using the same decoder as viewing sessions.
pub fn extract(
    path: &Path,
    options: &FrameOptions,
    control: &Control,
) -> Result<Vec<EncodedFrame>> {
    options.validate()?;
    let started = Instant::now();
    let mut reader = crate::video::VideoReader::open(path, options.window.start, control)?;
    let mut output = Vec::new();
    let mut total_bytes = 0usize;
    let stats = reader.sample(options, control, |sample| {
        let encoded = encode_sample(&sample, options.format)?;
        let bytes = encoded.bytes;
        total_bytes += bytes.len();
        ensure!(
            total_bytes <= 12 * 1024 * 1024,
            "encoded frames exceed 12 MiB; reduce count or resolution"
        );
        output.push(EncodedFrame {
            timestamp: sample.timestamp,
            estimated_timestamp: sample.estimated,
            width: sample.width,
            height: sample.height,
            format: options.format,
            bytes,
        });
        Ok(())
    })?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        decoded_frames = stats.decoded_frames,
        frames = output.len(),
        total_bytes,
        "frame extraction complete"
    );
    Ok(output)
}
pub(crate) fn encode_sample(
    sample: &crate::video::RgbSample,
    format: ImageFormat,
) -> Result<EncodedFrame> {
    let mut bytes = Vec::new();
    match format {
        ImageFormat::Jpeg => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
            .encode(
                &sample.pixels,
                sample.width,
                sample.height,
                image::ExtendedColorType::Rgb8,
            )?,
        ImageFormat::Webp => image::codecs::webp::WebPEncoder::new_lossless(&mut bytes).encode(
            &sample.pixels,
            sample.width,
            sample.height,
            image::ExtendedColorType::Rgb8,
        )?,
    }
    Ok(EncodedFrame {
        timestamp: sample.timestamp,
        estimated_timestamp: sample.estimated,
        width: sample.width,
        height: sample.height,
        format,
        bytes,
    })
}
/// Bounded PCM for a <=10 minute window. Timestamp gaps become silence; overlaps
/// are trimmed so Whisper offsets stay on the media presentation timeline.
pub fn audio(path: &Path, window: &Window, control: &Control) -> Result<Vec<f32>> {
    window.validate()?;
    let mut input = open(path, control)?;
    let base = origin(&input);
    let s = input
        .streams()
        .best(Type::Audio)
        .context("no audio stream")?;
    let index = s.index();
    let tb = s.time_base();
    let mut decoder = codec::context::Context::from_parameters(s.parameters())?
        .decoder()
        .audio()?;
    seek(&mut input, window.start, base)?;
    decoder.flush();
    let mut resampler: Option<software::resampling::Context> = None;
    let mut out = Vec::new();
    let limit = ((window.end - window.start) * 16000.0).ceil() as usize;
    let mut cursor = None;
    let mut expected_input = None;
    {
        let mut consume = |f: &mut frame::Audio| -> Result<bool> {
            control.check()?;
            ensure!(
                f.rate() > 0 && f.samples() <= 1_048_576,
                "invalid audio frame"
            );
            let layout = if f.channel_layout().is_empty() {
                ChannelLayout::default(i32::from(f.channels()))
            } else {
                f.channel_layout()
            };
            ensure!(
                layout.channels() > 0 && layout.channels() <= 32,
                "invalid channel layout"
            );
            f.set_channel_layout(layout);
            let t = seconds(
                f.timestamp().or(f.pts()),
                tb.numerator(),
                tb.denominator(),
                base,
            )
            .unwrap_or(expected_input.unwrap_or(window.start));
            if expected_input.is_some_and(|expected: f64| (t - expected).abs() > 0.01) {
                if let (Some(r), Some(time)) = (resampler.as_mut(), cursor.as_mut()) {
                    flush_audio(r, &mut out, time, window, limit, control)?;
                }
                resampler = None;
            }
            expected_input = Some(t + f.samples() as f64 / f64::from(f.rate()));
            if t >= window.end {
                return Ok(true);
            }
            if let Some(r) = resampler.as_ref() {
                ensure!(
                    r.input().format == f.format()
                        && r.input().rate == f.rate()
                        && r.input().channel_layout == layout,
                    "audio format changed midstream"
                );
            } else {
                resampler = Some(software::resampling::Context::get(
                    f.format(),
                    layout,
                    f.rate(),
                    Sample::F32(ff::format::sample::Type::Packed),
                    ChannelLayout::MONO,
                    16000,
                )?);
                cursor = Some(t);
            }
            let r = resampler.as_mut().unwrap();
            // Delay is in output samples; allocating for rate conversion also handles upsampling.
            let capacity =
                ((f.samples() as u64 * 16000).div_ceil(u64::from(f.rate())) + 4096) as usize;
            let mut converted = frame::Audio::new(
                Sample::F32(ff::format::sample::Type::Packed),
                capacity,
                ChannelLayout::MONO,
            );
            r.run(f, &mut converted)?;
            append_audio(
                &mut out,
                converted.plane::<f32>(0),
                cursor.as_mut().unwrap(),
                window,
                limit,
            );
            Ok(out.len() >= limit)
        };
        let mut done = false;
        let mut packets = 0u64;
        loop {
            packets += 1;
            ensure!(packets <= 2_000_000, "audio packet budget exceeded");
            control.check()?;
            let mut p = Packet::empty();
            match p.read(&mut input) {
                Ok(()) => {}
                Err(ff::Error::Eof) => break,
                Err(e) => return Err(e).context("read audio packet"),
            }
            if p.stream() != index {
                continue;
            }
            if let Err(e) = decoder.send_packet(&p) {
                if !again(e) {
                    return Err(e.into());
                }
                done = drain_audio(&mut decoder, &mut consume)?;
                if done {
                    break;
                }
                decoder.send_packet(&p)?;
            }
            done = drain_audio(&mut decoder, &mut consume)?;
            if done {
                break;
            }
        }
        if !done {
            decoder.send_eof()?;
            drain_audio(&mut decoder, &mut consume)?;
        }
    }
    if let (Some(r), Some(mut time)) = (resampler.as_mut(), cursor) {
        flush_audio(r, &mut out, &mut time, window, limit, control)?;
    }
    Ok(out)
}
fn flush_audio(
    r: &mut software::resampling::Context,
    out: &mut Vec<f32>,
    time: &mut f64,
    window: &Window,
    limit: usize,
    control: &Control,
) -> Result<()> {
    loop {
        control.check()?;
        let mut f = frame::Audio::new(
            Sample::F32(ff::format::sample::Type::Packed),
            4096,
            ChannelLayout::MONO,
        );
        r.flush(&mut f)?;
        if f.samples() == 0 {
            break;
        }
        append_audio(out, f.plane::<f32>(0), time, window, limit);
    }
    Ok(())
}
fn drain_audio(
    d: &mut ff::decoder::Audio,
    consume: &mut impl FnMut(&mut frame::Audio) -> Result<bool>,
) -> Result<bool> {
    let mut f = frame::Audio::empty();
    loop {
        match d.receive_frame(&mut f) {
            Ok(()) => {
                if consume(&mut f)? {
                    return Ok(true);
                }
            }
            Err(ff::Error::Eof) => return Ok(false),
            Err(e) if again(e) => return Ok(false),
            Err(e) => return Err(e).context("decode audio frame"),
        }
    }
}
fn append_audio(
    out: &mut Vec<f32>,
    samples: &[f32],
    cursor: &mut f64,
    window: &Window,
    limit: usize,
) {
    let offset = ((*cursor - window.start) * 16000.0).round() as i64;
    let skip = if offset < 0 {
        (-offset) as usize
    } else {
        out.len().saturating_sub(offset as usize)
    }
    .min(samples.len());
    if offset > out.len() as i64 {
        out.resize((offset as usize).min(limit), 0.0);
    }
    let n = (samples.len() - skip).min(limit - out.len());
    out.extend_from_slice(&samples[skip..skip + n]);
    *cursor += samples.len() as f64 / 16000.0;
}

#[cfg(test)]
mod tests {
    use crate::orientation::DisplayTransform as T;
    use ffmpeg_next::ffi;

    /// The matrix `ffmpeg -display_rotation R [-display_hflip] [-display_vflip] -i`
    /// attaches (fftools add_display_matrix_to_stream).
    fn cli_matrix(rotation: f64, hflip: bool, vflip: bool) -> [i32; 9] {
        let mut matrix = [0i32; 9];
        // SAFETY: both functions access exactly nine int32 values of a local array.
        unsafe {
            ffi::av_display_rotation_set(matrix.as_mut_ptr(), -rotation);
            ffi::av_display_matrix_flip(matrix.as_mut_ptr(), hflip.into(), vflip.into());
        }
        matrix
    }

    #[test]
    fn cli_display_options_select_autorotation_transforms() {
        for (rotation, hflip, vflip, ffprobe_rotation, expected) in [
            (0.0, false, false, 0, T::None),
            (90.0, false, false, 90, T::Rotate90Counterclockwise),
            (-90.0, false, false, -90, T::Rotate90Clockwise),
            (180.0, false, false, -180, T::Rotate180),
            (0.0, true, false, -180, T::FlipHorizontal),
            (0.0, false, true, 0, T::FlipVertical),
            (90.0, true, false, 90, T::Transverse),
            (-90.0, true, false, -90, T::Transpose),
        ] {
            let matrix = cli_matrix(rotation, hflip, vflip);
            // SAFETY: reads nine int32 values of a live local array.
            let angle = unsafe { ffi::av_display_rotation_get(matrix.as_ptr()) };
            let case = format!("rotation {rotation} hflip {hflip} vflip {vflip}: {matrix:?}");
            assert_eq!(angle.round() as i32, ffprobe_rotation, "{case}");
            assert_eq!(T::from_display_matrix(&matrix, angle), expected, "{case}");
        }
    }
}
