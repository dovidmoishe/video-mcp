use crate::{Segment, Transcript, Window, control::Control, media};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// One model per service, lazy loaded and serialized; inference state is per call.
pub struct Transcriber {
    model: PathBuf,
    threads: i32,
    context: Mutex<Option<WhisperContext>>,
}
impl Transcriber {
    pub fn new(model: PathBuf, threads: usize) -> Result<Self> {
        ensure!((1..=16).contains(&threads), "Whisper threads must be 1..16");
        Ok(Self {
            model,
            threads: threads as i32,
            context: Mutex::new(None),
        })
    }
    pub fn transcribe(
        &self,
        path: &Path,
        window: &Window,
        language: Option<&str>,
        control: &Control,
    ) -> Result<Transcript> {
        static LOGGING: std::sync::Once = std::sync::Once::new();
        LOGGING.call_once(whisper_rs::install_logging_hooks);
        let started = Instant::now();
        window.validate()?;
        if let Some(l) = language {
            ensure!(
                whisper_rs::get_lang_id(l).is_some(),
                "unknown Whisper language"
            );
        }
        let pcm = media::audio(path, window, control)?;
        control.check()?;
        // Empty/near-silent PCM is not evidence that the video has no events.
        if pcm.is_empty()
            || pcm.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / (pcm.len() as f64) < 1e-10
        {
            return Ok(Transcript {
                segments: vec![],
                truncated: false,
                speech_detected: false,
            });
        }
        let mut cached = self
            .context
            .lock()
            .map_err(|_| anyhow::anyhow!("Whisper model mutex poisoned"))?;
        control.check()?;
        if cached.is_none() {
            ensure!(
                self.model.is_file(),
                "Whisper model is missing: {}",
                self.model.display()
            );
            let mut params = WhisperContextParameters::default();
            params.use_gpu(false);
            *cached = Some(
                WhisperContext::new_with_params(&self.model, params)
                    .context("load local Whisper model")?,
            );
        }
        let mut state = cached.as_ref().unwrap().create_state()?;
        let mut segments = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        // Cancellation is checked between bounded 30-second inferences. No callback
        // captures native pointers or outlives the request.
        'chunks: for (chunk_index, chunk) in pcm.chunks(30 * 16000).enumerate() {
            control.check()?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(self.threads);
            params.set_language(language);
            params.set_translate(false);
            params.set_no_context(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            state.full(params, chunk)?;
            control.check()?;
            let offset = window.start + chunk_index as f64 * 30.0;
            for s in state.as_iter() {
                let text = s.to_str()?.trim().to_owned();
                if text.is_empty() {
                    continue;
                }
                if segments.len() >= 512 || bytes + text.len() > 64 * 1024 {
                    truncated = true;
                    break 'chunks;
                }
                let start =
                    (offset + s.start_timestamp() as f64 / 100.0).clamp(window.start, window.end);
                let end = (offset + s.end_timestamp() as f64 / 100.0).clamp(start, window.end);
                bytes += text.len();
                segments.push(Segment { start, end, text });
            }
        }
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            samples = pcm.len(),
            segments = segments.len(),
            "transcription complete"
        );
        Ok(Transcript {
            speech_detected: has_transcribed_speech(&segments),
            segments,
            truncated,
        })
    }
}

// Whisper can emit [MUSIC] or [MUSIC PLAYING] instead of dialogue. Retain those
// captions as model output, but do not use them as evidence of speech. This
// narrow caption check is not audio VAD or a general sound classifier.
pub(crate) fn has_transcribed_speech(segments: &[Segment]) -> bool {
    segments.iter().any(|segment| {
        let text = segment.text.to_ascii_uppercase();
        !text
            .replace("[MUSIC PLAYING]", "")
            .replace("[MUSIC]", "")
            .trim()
            .is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn segment(text: &str) -> Segment {
        Segment {
            start: 0.0,
            end: 1.0,
            text: text.into(),
        }
    }
    #[test]
    fn music_annotation_is_not_speech_but_dialogue_is() {
        assert!(!has_transcribed_speech(&[]));
        assert!(!has_transcribed_speech(&[segment("  [MUSIC]  ")]));
        assert!(!has_transcribed_speech(&[segment("[music]\n[MUSIC]")]));
        assert!(!has_transcribed_speech(&[segment("[MUSIC PLAYING]")]));
        assert!(!has_transcribed_speech(&[segment(
            " [Music playing] [MUSIC] "
        )]));
        assert!(has_transcribed_speech(&[segment("Hello, welcome.")]));
        assert!(has_transcribed_speech(&[
            segment("[MUSIC]"),
            segment("Hello.")
        ]));
        assert!(has_transcribed_speech(&[segment("[MUSIC] Welcome.")]));
        assert!(has_transcribed_speech(&[segment(
            "[MUSIC PLAYING] Welcome."
        )]));
        assert!(has_transcribed_speech(&[segment("music playing")]));
    }
}
