#![cfg(feature = "native")]
use std::{path::PathBuf, time::Duration};
use video_mcp::{FrameOptions, ImageFormat, Window, control::Control, media};
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os("VIDEO_FIXTURES")
            .expect("set VIDEO_FIXTURES; run scripts/make-fixtures.ps1"),
    )
    .join(name)
}
fn control() -> Control {
    Control::new(Duration::from_secs(30))
}
/// The first frame of an orientation fixture as the ffmpeg CLI shows it
/// (autorotate on), written by scripts/make-fixtures.ps1 as raw rgb24.
fn cli_reference(name: &str, width: u32, height: u32) -> image::RgbImage {
    let bytes = std::fs::read(fixture(&format!("{name}.rgb"))).unwrap();
    image::RgbImage::from_raw(width, height, bytes).expect("reference has display size")
}
fn mean_difference(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| f64::from(x.abs_diff(*y)))
        .sum::<f64>()
        / a.as_raw().len() as f64
}
fn video_stream(metadata: &video_mcp::Metadata) -> &video_mcp::StreamMetadata {
    metadata
        .streams
        .iter()
        .find(|s| s.kind == "video")
        .expect("video stream")
}

#[test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
fn display_matrices_match_ffmpeg_cli_autorotation() {
    use video_mcp::{Region, orientation::DisplayTransform as T};
    // An asymmetric crop catches swapped or mirrored mappings.
    let crop = Region {
        x: 0.05,
        y: 0.1,
        width: 0.6,
        height: 0.3,
    };
    // Each fixture stores the same 640x360 picture; only the display matrix differs.
    let mut failures = Vec::new();
    for (name, rotation, transform) in [
        ("orientation-none", 0, T::None),
        ("orientation-r90", 90, T::Rotate90Counterclockwise),
        ("orientation-r-90", -90, T::Rotate90Clockwise),
        ("orientation-r180", -180, T::Rotate180),
        ("orientation-hflip", -180, T::FlipHorizontal),
        ("orientation-vflip", 0, T::FlipVertical),
        ("orientation-r90-hflip", 90, T::Transverse),
        ("orientation-r-90-hflip", -90, T::Transpose),
    ] {
        let path = fixture(&format!("{name}.mp4"));
        let metadata = media::inspect(&path, &control()).unwrap();
        let video = video_stream(&metadata);
        let (width, height) = transform.display_size(640, 360);
        assert_eq!(
            (video.width, video.height),
            (Some(640), Some(360)),
            "{name}"
        );
        assert_eq!(video.rotation, Some(rotation), "{name}");
        assert_eq!(video.display_transform, Some(transform), "{name}");
        assert_eq!(
            (video.display_width, video.display_height),
            (Some(width), Some(height)),
            "{name}"
        );
        let reference = cli_reference(name, width, height);
        for region in [None, Some(crop)] {
            let frames = media::extract(
                &path,
                &FrameOptions {
                    window: Window {
                        start: 0.0,
                        end: 0.5,
                    },
                    count: 1,
                    max_width: 2048,
                    max_height: 2048,
                    crop: region,
                    format: ImageFormat::Webp,
                    ..Default::default()
                },
                &control(),
            )
            .unwrap();
            let ours = image::load_from_memory(&frames[0].bytes).unwrap().to_rgb8();
            let expected = match region {
                None => reference.clone(),
                Some(r) => {
                    let (x, y, w, h) = r.pixels(width, height);
                    image::imageops::crop_imm(&reference, x, y, w, h).to_image()
                }
            };
            let difference = mean_difference(&ours, &expected);
            eprintln!(
                "{name} crop={:?}: mean |difference| {difference:.4}",
                region.is_some()
            );
            // The CLI turns the decoded YUV frame and then converts it to RGB; we
            // convert first and turn the RGB. The two orders can round differently:
            // byte-identical with FFmpeg 8.0 on Windows x64, about one level on average
            // with FFmpeg 8.1 on Apple silicon. A wrong transform or crop mapping
            // differs by tens of levels on average.
            if difference >= 4.0 {
                failures.push(format!(
                    "{name} crop {region:?}: mean absolute difference {difference}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
async fn portrait_phone_matrix_uses_display_coordinates_throughout() {
    use video_mcp::{Region, detail::DetailRequest, service::Service, watch::WatchRequest};
    // Stored 640x360 with an iPhone portrait matrix (ffprobe rotation -90), shown
    // 360x640 as blue|red over yellow|green. In display pixels the stored top-left
    // white marker is at x 270..330, y 40..100 and the stored bottom-right flash
    // (1.2-1.3s) at x 40..100, y 520..580.
    let path = fixture("orientation-r-90.mp4");
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    let page = service
        .watch(WatchRequest {
            source: Some(path.to_string_lossy().into_owned()),
            seconds: 2.0,
            fps: 1.0,
            event_budget: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    let video = video_stream(page.metadata.as_ref().unwrap());
    assert_eq!((video.width, video.height), (Some(640), Some(360)));
    assert_eq!(video.rotation, Some(-90));
    assert_eq!(
        (video.display_width, video.display_height),
        (Some(360), Some(640))
    );
    assert_eq!(page.frames.len(), 2);
    assert!(
        page.frames
            .iter()
            .all(|f| (f.width, f.height) == (320, 569)),
        "{:?}",
        page.frames
    );
    let contains = |r: &Region, (x, y): (f64, f64)| {
        r.x <= x && x <= r.x + r.width && r.y <= y && y <= r.y + r.height
    };
    let flash = (70.0 / 360.0, 550.0 / 640.0);
    let stored_flash = (550.0 / 640.0, 290.0 / 360.0);
    assert!(
        page.change_candidates
            .iter()
            .any(|c| (1.15..1.5).contains(&c.timestamp) && contains(&c.region, flash)),
        "{:?}",
        page.change_candidates
    );
    assert!(
        page.change_candidates
            .iter()
            .all(|c| !contains(&c.region, stored_flash)),
        "{:?}",
        page.change_candidates
    );
    // Right half, top half of the display: the red (stored top-left) quadrant with
    // the marker. In stored coordinates this rectangle would be the green quadrant.
    let detail = service
        .details(DetailRequest {
            source: None,
            video_id: Some(page.video_id.clone()),
            timestamps: vec![0.5],
            crop: Some(Region {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            }),
            track_region: None,
            max_width: 2048,
            max_height: 2048,
            include_ocr: false,
        })
        .await
        .unwrap();
    let frame = &detail.frames[0];
    assert_eq!((frame.source_width, frame.source_height), (360, 640));
    assert_eq!(frame.source_pixels, [180, 0, 180, 320]);
    assert_eq!((frame.width, frame.height), (180, 320));
    let image = image::load_from_memory(&frame.image.bytes)
        .unwrap()
        .to_rgb8();
    let marker = image.get_pixel(120, 70);
    assert!(marker.0.iter().all(|c| *c > 200), "{marker:?}");
    let red = image.get_pixel(20, 250);
    assert!(red[0] > 150 && red[1] < 100 && red[2] < 100, "{red:?}");
    service.close_video(page.video_id).await.unwrap();
    // extract_keyframes shares the convention: a tight display crop is the marker.
    let frames = media::extract(
        &path,
        &FrameOptions {
            window: Window {
                start: 0.0,
                end: 0.5,
            },
            count: 1,
            crop: Some(Region {
                x: 280.0 / 360.0,
                y: 50.0 / 640.0,
                width: 40.0 / 360.0,
                height: 40.0 / 640.0,
            }),
            ..Default::default()
        },
        &control(),
    )
    .unwrap();
    let image = image::load_from_memory(&frames[0].bytes).unwrap().to_rgb8();
    assert!(image.pixels().all(|p| p.0.iter().all(|c| *c > 200)));
}
#[test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
fn probe_decode_scale_and_audio() {
    let path = fixture("av.mp4");
    let m = media::inspect(&path, &control()).unwrap();
    assert!((m.duration_seconds.unwrap() - 3.0).abs() < 0.2);
    assert!(
        m.streams
            .iter()
            .any(|s| s.width == Some(320) && s.height == Some(240))
    );
    assert!(m.streams.iter().any(|s| s.sample_rate == Some(48000)));
    for format in [ImageFormat::Jpeg, ImageFormat::Webp] {
        let frames = media::extract(
            &path,
            &FrameOptions {
                window: Window {
                    start: 0.5,
                    end: 2.5,
                },
                count: 4,
                max_width: 160,
                max_height: 120,
                format,
                ..Default::default()
            },
            &control(),
        )
        .unwrap();
        assert_eq!(frames.len(), 4);
        for (i, f) in frames.iter().enumerate() {
            assert!(
                f.timestamp >= 0.5 + i as f64 * 0.5 - 0.001 && f.timestamp < 1.0 + i as f64 * 0.5
            );
            assert_eq!((f.width, f.height), (160, 120));
            let image = image::load_from_memory(&f.bytes).unwrap();
            assert_eq!((image.width(), image.height()), (160, 120));
        }
    }
    let pcm = media::audio(
        &path,
        &Window {
            start: 0.5,
            end: 2.5,
        },
        &control(),
    )
    .unwrap();
    assert!(
        (pcm.len() as i64 - 32000).abs() < 64,
        "samples={}",
        pcm.len()
    );
    assert!(pcm.iter().all(|x| x.is_finite()));
    assert!(pcm.iter().any(|x| x.abs() > 0.01));
}
#[test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
fn missing_streams_invalid_data_and_flushing() {
    assert!(media::audio(&fixture("silent.mp4"), &Window::default(), &control()).is_err());
    assert!(media::extract(&fixture("tone.wav"), &FrameOptions::default(), &control()).is_err());
    assert!(media::inspect(&fixture("invalid.bin"), &control()).is_err());
    // Upsampling 8 kHz and final resampler/decoder flush must preserve the tail.
    let pcm = media::audio(
        &fixture("tone.wav"),
        &Window {
            start: 0.0,
            end: 3.0,
        },
        &control(),
    )
    .unwrap();
    assert!(
        (pcm.len() as i64 - 48000).abs() < 32,
        "samples={}",
        pcm.len()
    );
    let frames = media::extract(
        &fixture("av.mp4"),
        &FrameOptions {
            window: Window {
                start: 2.8,
                end: 3.5,
            },
            count: 8,
            interval: Some(0.04),
            ..Default::default()
        },
        &control(),
    )
    .unwrap();
    assert!(!frames.is_empty());
    assert!(frames.last().unwrap().timestamp > 2.9);
}
#[test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
fn nonzero_origin_and_vfr() {
    for name in ["offset.mkv", "vfr.mkv"] {
        let frames = media::extract(
            &fixture(name),
            &FrameOptions {
                window: Window {
                    start: 0.5,
                    end: 2.5,
                },
                count: 4,
                ..Default::default()
            },
            &control(),
        )
        .unwrap();
        assert!(!frames.is_empty());
        assert!(frames.windows(2).all(|w| w[1].timestamp > w[0].timestamp));
        assert!(
            frames
                .iter()
                .all(|f| f.timestamp >= 0.5 && f.timestamp < 2.5)
        );
    }
}
#[cfg(feature = "whisper")]
#[test]
#[ignore = "requires VIDEO_WHISPER_MODEL and jfk.wav fixture"]
fn speech_model_smoke() {
    let model = std::env::var_os("VIDEO_WHISPER_MODEL")
        .expect("set VIDEO_WHISPER_MODEL")
        .into();
    let transcriber = video_mcp::speech::Transcriber::new(model, 4).unwrap();
    let t = transcriber
        .transcribe(
            &fixture("jfk.wav"),
            &Window {
                start: 0.0,
                end: 15.0,
            },
            Some("en"),
            &Control::new(Duration::from_secs(180)),
        )
        .unwrap();
    assert!(t.speech_detected);
    assert!(
        t.segments
            .iter()
            .any(|s| s.text.to_lowercase().contains("country"))
    );
    assert!(
        t.segments
            .iter()
            .all(|s| s.start >= 0.0 && s.end >= s.start && s.end <= 15.0)
    );
    let silent = transcriber
        .transcribe(
            &fixture("silence.wav"),
            &Window {
                start: 0.0,
                end: 1.0,
            },
            Some("en"),
            &control(),
        )
        .unwrap();
    assert!(!silent.speech_detected && silent.segments.is_empty());
}

#[test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
fn missing_pts_and_audio_gap() {
    let frames = media::extract(
        &fixture("missing-pts.h264"),
        &FrameOptions {
            window: Window {
                start: 0.0,
                end: 3.0,
            },
            count: 3,
            ..Default::default()
        },
        &control(),
    )
    .unwrap();
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|f| f.estimated_timestamp));
    let pcm = media::audio(
        &fixture("audio-gap.mkv"),
        &Window {
            start: 0.0,
            end: 3.0,
        },
        &control(),
    )
    .unwrap();
    assert!((pcm.len() as i64 - 48000).abs() < 64);
    assert!(pcm[20000..28000].iter().all(|s| s.abs() < 1e-6));
    assert!(pcm[35000..40000].iter().any(|s| s.abs() > 0.01));
}

#[tokio::test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
async fn viewing_sessions_advance_rewind_replay_and_release() {
    use video_mcp::{service::Service, watch::WatchRequest};
    let service = Service::new(2, Default::default(), None, 4).unwrap();
    let first = service
        .watch(WatchRequest {
            source: Some(fixture("av.mp4").to_string_lossy().into_owned()),
            seconds: 1.0,
            fps: 2.0,
            include_speech: false,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(first.frames.len(), 2);
    assert!(first.metadata.is_some());
    assert!(!first.cache_hit);
    assert!(!first.seek_performed);
    assert_eq!(first.frames[0].timestamp, 0.0);
    assert_eq!(first.next_start, Some(1.0));
    let image = image::load_from_memory(&first.jpeg).unwrap();
    assert_eq!((image.width(), image.height()), (first.width, first.height));
    let request = WatchRequest {
        video_id: Some(first.video_id.clone()),
        seconds: 1.0,
        fps: 2.0,
        include_speech: false,
        ..Default::default()
    };
    let next = service.watch(request.clone()).await.unwrap();
    assert_eq!(next.window.start, 1.0);
    assert!(next.frames[0].timestamp >= 1.0);
    assert!(!next.seek_performed);
    let mut rewind = request.clone();
    rewind.start = Some(0.0);
    let back = service.watch(rewind.clone()).await.unwrap();
    assert!(back.seek_performed);
    assert_eq!(back.frames[0].timestamp, 0.0);
    let replay = service.watch(rewind).await.unwrap();
    assert!(replay.cache_hit);
    assert_eq!(replay.decoded_frames, 0);
    assert_eq!(back.jpeg, replay.jpeg);
    let audio = service
        .watch(WatchRequest {
            video_id: Some(first.video_id.clone()),
            start: Some(0.0),
            seconds: 1.0,
            fps: 2.0,
            include_speech: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(audio.cache_hit);
    assert_eq!(audio.decoded_frames, 0);
    assert_eq!(audio.jpeg, replay.jpeg);
    assert!(matches!(
        audio.audio_status.as_str(),
        "model_unconfigured" | "whisper_not_built"
    ));
    assert!(service.close_video(first.video_id.clone()).await.unwrap());
    assert!(service.watch(request).await.is_err());
}

#[tokio::test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
async fn viewing_session_capacity_and_source_changes() {
    use video_mcp::{service::Service, watch::WatchRequest};
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("av.mp4");
    std::fs::copy(fixture("av.mp4"), &source).unwrap();
    let service = Service::new(2, Default::default(), None, 4).unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let page = service
            .watch(WatchRequest {
                source: Some(source.to_string_lossy().into_owned()),
                seconds: 0.5,
                include_speech: false,
                ..Default::default()
            })
            .await
            .unwrap();
        ids.push(page.video_id);
    }
    assert!(
        service
            .watch(WatchRequest {
                source: Some(source.to_string_lossy().into_owned()),
                seconds: 0.5,
                include_speech: false,
                ..Default::default()
            })
            .await
            .is_err()
    );
    for id in ids {
        assert!(service.close_video(id).await.unwrap());
    }
    let page = service
        .watch(WatchRequest {
            source: Some(source.to_string_lossy().into_owned()),
            seconds: 0.5,
            include_speech: false,
            ..Default::default()
        })
        .await
        .unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let error = service
        .watch(WatchRequest {
            video_id: Some(page.video_id.clone()),
            seconds: 0.5,
            include_speech: false,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("changed"));
    assert!(service.close_video(page.video_id).await.unwrap());
}

#[tokio::test]
#[ignore = "requires generated fixtures and FFmpeg development libraries"]
async fn change_scan_finds_a_flash_between_displayed_samples() {
    use video_mcp::{service::Service, watch::WatchRequest};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    let page = service
        .watch(WatchRequest {
            source: Some(fixture("brief-flash.mp4").to_string_lossy().into_owned()),
            seconds: 3.0,
            fps: 1.0,
            include_speech: false,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(page.frames.iter().all(|frame| frame.near_black));
    assert!(page.scanned_until.unwrap() > 2.9);
    assert!(
        page.change_candidates
            .iter()
            .any(|event| event.timestamp >= 1.1 && event.timestamp <= 1.4)
    );
    let focus = service
        .watch(WatchRequest {
            video_id: Some(page.video_id.clone()),
            start: Some(1.0),
            seconds: 0.5,
            fps: 8.0,
            include_speech: false,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(focus.frames.iter().any(|frame| frame.mean_luma > 200.0));
    assert!(service.close_video(page.video_id).await.unwrap());
}

#[tokio::test]
#[ignore = "requires scripts/make-visual-fixtures.py and native FFmpeg"]
async fn portrait_detail_crops_cache_and_preserve_playhead() {
    use video_mcp::{Region, detail::DetailRequest, service::Service, watch::WatchRequest};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    // The rotated twin stores this portrait drawing as 640x360 with an iPhone-style
    // display matrix; every view and coordinate must match the upright original.
    for name in ["portrait-regression.mp4", "portrait-regression-rotated.mp4"] {
        let page = service
            .watch(WatchRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                seconds: 0.5,
                event_budget: 0,
                ..Default::default()
            })
            .await
            .unwrap();
        let video = video_stream(page.metadata.as_ref().unwrap());
        assert_eq!(
            (video.display_width, video.display_height),
            (Some(360), Some(640)),
            "{name}"
        );
        assert_eq!(
            (page.frames[0].width, page.frames[0].height),
            (320, 569),
            "{name}"
        );
        let mut request = DetailRequest {
            source: None,
            video_id: Some(page.video_id.clone()),
            timestamps: vec![0.2, 0.3],
            crop: Some(Region {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 1.0,
            }),
            max_width: 1920,
            max_height: 1920,
            track_region: None,
            include_ocr: false,
        };
        let detail = service.details(request.clone()).await.unwrap();
        assert_eq!(detail.frames.len(), 2);
        assert_eq!(detail.frames[0].source_pixels, [180, 0, 180, 640], "{name}");
        assert_eq!(
            (detail.frames[0].width, detail.frames[0].height),
            (180, 640),
            "{name}"
        ); // no upscaling
        let image = image::load_from_memory(&detail.frames[0].image.bytes)
            .unwrap()
            .to_rgb8();
        let color = image.get_pixel(80, 400);
        assert!(color[0] > 180 && color[2] < 50, "{name} {color:?}");
        let replay = service.details(request.clone()).await.unwrap();
        assert!(replay.cache_hit);
        assert_eq!(replay.decoded_frames, 0);
        assert_eq!(replay.frames[0].image.bytes, detail.frames[0].image.bytes);
        request.crop.as_mut().unwrap().x = 0.0;
        let left = service.details(request).await.unwrap();
        assert!(!left.cache_hit);
        let image = image::load_from_memory(&left.frames[0].image.bytes)
            .unwrap()
            .to_rgb8();
        assert!(image.get_pixel(80, 400)[2] > 100, "{name}");
        let next = service
            .watch(WatchRequest {
                video_id: Some(page.video_id.clone()),
                seconds: 0.5,
                event_budget: 0,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(next.window.start, 0.5);
        assert!((next.frames[0].timestamp - 0.5).abs() < 0.001);
        service.close_video(page.video_id).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires scripts/make-visual-fixtures.py and native FFmpeg"]
async fn regional_scan_returns_actual_brief_event_images_and_gradual_hints() {
    use video_mcp::{service::Service, watch::WatchRequest};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    // The rotated twin is stored upside down with a 180-degree display matrix.
    for name in ["local-event.mp4", "local-event-rotated.mp4"] {
        let page = service
            .watch(WatchRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                seconds: 3.0,
                event_budget: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.frames.iter().all(|f| f.near_black));
        assert!(
            page.change_candidates
                .iter()
                .any(|c| (1.1..1.3).contains(&c.timestamp) && c.region.x >= 0.8),
            "{name} {:?}",
            page.change_candidates
        );
        assert!(!page.event_jpeg.is_empty());
        assert!(page.event_frames.len() <= 3);
        let image = image::load_from_memory(&page.event_jpeg).unwrap().to_rgb8();
        assert!(
            page.event_frames.iter().any(|f| {
                let p = image.get_pixel(
                    f.frame.image_rect[0] + f.frame.width * 601 / 640,
                    f.frame.image_rect[1] + f.frame.height * 291 / 360,
                );
                p[0] > 150
            }),
            "{name}"
        );
        service.close_video(page.video_id).await.unwrap();
    }
    let slow = service
        .watch(WatchRequest {
            source: Some(fixture("slow-change.mp4").to_string_lossy().into_owned()),
            seconds: 3.0,
            event_budget: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!slow.change_candidates.is_empty());
    assert!(slow.event_jpeg.is_empty());
    assert!(slow.event_frames.is_empty());
    service.close_video(slow.video_id).await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires visual fixtures, FFmpeg and a local Windows OCR language"]
async fn local_ocr_reads_changed_numbers_and_tracks_text() {
    use video_mcp::{detail::DetailRequest, service::Service};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    // The rotated twin stores the text sideways (700x1200) with a display matrix
    // that turns it upright; OCR must read it and report display-space boxes.
    for name in ["ocr-regression.mp4", "ocr-regression-rotated.mp4"] {
        let page = service
            .details(DetailRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                video_id: None,
                timestamps: vec![0.0, 1.0],
                crop: None,
                max_width: 1200,
                max_height: 700,
                track_region: None,
                include_ocr: true,
            })
            .await
            .unwrap();
        for (frame, number) in page.frames.iter().zip(["248", "249"]) {
            assert_eq!((frame.width, frame.height), (1200, 700), "{name}");
            let reading = frame.ocr.as_ref().unwrap();
            assert_eq!(reading.status, "recognized_unverified");
            assert!(
                reading.words.iter().any(|w| w.text.contains(number)),
                "{name} {reading:?}"
            );
            assert!(reading.words.iter().all(|w| w.region.validate().is_ok()));
            // "BALANCE ..." is drawn from (100,100) with 56px text in display pixels.
            let balance = reading
                .words
                .iter()
                .find(|w| w.text.contains(number))
                .unwrap();
            assert!(
                balance.region.y > 80.0 / 700.0 && balance.region.y < 200.0 / 700.0,
                "{name} {balance:?}"
            );
            assert!(
                balance.region.width > balance.region.height,
                "{name} {balance:?}"
            );
        }
        assert!(
            page.text_changes
                .iter()
                .any(|c| c.before.contains("248") && c.after.contains("249")),
            "{name}"
        );
        assert!(
            page.text_motion
                .iter()
                .any(|m| m.text == "MOVING" && m.to_region.x > m.from_region.x),
            "{name}"
        );
    }
}

#[tokio::test]
#[ignore = "requires visual fixtures and native FFmpeg"]
async fn opposite_motion_retains_temporal_order_at_dense_rates() {
    use video_mcp::{detail::DetailRequest, service::Service};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    for (name, times, rightward) in [
        ("visual-pilot.mp4", vec![1.0, 2.0], true),
        ("visual-pilot-reversed.mp4", vec![4.0, 5.0], false),
        // Stored 720x1280 turned counterclockwise, shown upright by its matrix.
        ("visual-pilot-rotated.mp4", vec![1.0, 2.0], true),
    ] {
        let page = service
            .details(DetailRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                video_id: None,
                timestamps: times,
                crop: None,
                max_width: 1280,
                max_height: 720,
                track_region: None,
                include_ocr: false,
            })
            .await
            .unwrap();
        let centers: Vec<f64> = page
            .frames
            .iter()
            .map(|f| {
                let image = image::load_from_memory(&f.image.bytes).unwrap().to_rgb8();
                let mut sum = 0u64;
                let mut count = 0u64;
                for (x, _, p) in image.enumerate_pixels() {
                    if p[0] < 80 && p[1] > 150 && p[2] > 150 {
                        sum += u64::from(x);
                        count += 1;
                    }
                }
                assert!(count > 100);
                sum as f64 / count as f64
            })
            .collect();
        assert_eq!(centers[1] > centers[0], rightward);
    }
}

#[tokio::test]
#[ignore = "requires visual fixtures and native FFmpeg"]
async fn colored_badge_is_a_local_event_even_between_overview_samples() {
    use video_mcp::{service::Service, watch::WatchRequest};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    for name in ["visual-pilot.mp4", "visual-pilot-rotated.mp4"] {
        let page = service
            .watch(WatchRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                seconds: 6.0,
                event_budget: 0,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            page.frames
                .iter()
                .all(|f| (f.width, f.height) == (320, 180)),
            "{name}"
        );
        assert!(
            page.change_candidates
                .iter()
                .any(|e| (2.19..2.31).contains(&e.timestamp) && e.region.x >= 0.7),
            "{name} {:?}",
            page.change_candidates
        );
        service.close_video(page.video_id).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires visual fixtures and native FFmpeg"]
async fn appearance_tracking_follows_the_displayed_motion() {
    use video_mcp::{Region, detail::DetailRequest, service::Service};
    let service = Service::new(1, Default::default(), None, 4).unwrap();
    // The cyan circle moves left to right on display; in the rotated twin's stored
    // frames it moves bottom to top.
    for name in ["visual-pilot.mp4", "visual-pilot-rotated.mp4"] {
        let page = service
            .details(DetailRequest {
                source: Some(fixture(name).to_string_lossy().into_owned()),
                video_id: None,
                timestamps: vec![1.0, 1.1, 1.2, 1.3, 1.4, 1.5],
                crop: None,
                track_region: Some(Region {
                    x: 0.1,
                    y: 0.43,
                    width: 0.08,
                    height: 0.14,
                }),
                max_width: 1280,
                max_height: 1280,
                include_ocr: false,
            })
            .await
            .unwrap();
        let regions: Vec<Region> = page.motion.iter().filter_map(|m| m.region).collect();
        assert!(regions.len() >= 4, "{name} {:?}", page.motion);
        let (first, last) = (regions[0], regions[regions.len() - 1]);
        assert!(last.x > first.x + 0.1, "{name} {:?}", page.motion);
        assert!((last.y - first.y).abs() < 0.05, "{name} {:?}", page.motion);
    }
}
