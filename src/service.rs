use crate::{
    control::{CancelOnDrop, Control},
    source::{Source, SourceLimits},
    *,
};
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub struct Service {
    permits: Arc<Semaphore>,
    pub limits: SourceLimits,
    #[cfg(feature = "whisper")]
    transcriber: Option<Arc<speech::Transcriber>>,
    #[cfg(feature = "native")]
    watches: Arc<crate::watch::Sessions>,
}
impl Service {
    pub fn new(
        jobs: usize,
        limits: SourceLimits,
        model: Option<PathBuf>,
        threads: usize,
    ) -> Result<Self> {
        ensure!((1..=4).contains(&jobs), "jobs must be 1..4");
        ensure!((1..=16).contains(&threads), "Whisper threads must be 1..16");
        #[cfg(not(feature = "whisper"))]
        let _ = (&model, threads);
        #[cfg(feature = "whisper")]
        let transcriber = model
            .map(|p| speech::Transcriber::new(p, threads).map(Arc::new))
            .transpose()?;
        #[cfg(all(feature = "native", feature = "whisper"))]
        let watches = crate::watch::Sessions::with_speech(transcriber.clone());
        #[cfg(all(feature = "native", not(feature = "whisper")))]
        let watches = crate::watch::Sessions::default();
        Ok(Self {
            permits: Arc::new(Semaphore::new(jobs)),
            limits,
            #[cfg(feature = "whisper")]
            transcriber,
            #[cfg(feature = "native")]
            watches: Arc::new(watches),
        })
    }
    /// Fail fast when busy: do not create an unbounded request or download queue.
    pub async fn run<T, F>(&self, source: String, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(PathBuf, Control) -> Result<T> + Send + 'static,
    {
        self.run_source(source, move |source, control| {
            work(source.path.clone(), control)
        })
        .await
    }
    async fn run_source<T, F>(&self, source: String, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Source, Control) -> Result<T> + Send + 'static,
    {
        let started = Instant::now();
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .context("server busy; retry after the current operation")?;
        let control = Control::new(Duration::from_secs(300));
        let _cancel = CancelOnDrop(control.clone());
        let limits = self.limits.clone();
        let resolve_control = control.clone();
        // A dropped waiter signals cancellation but leaves the resolver alive long
        // enough to kill/reap a downloader while still holding its admission slot.
        let (source, permit) = tokio::spawn(async move {
            let resolved = Source::resolve(&source, &limits, &resolve_control).await?;
            Ok::<_, anyhow::Error>((resolved, permit))
        })
        .await
        .context("source resolver panicked")??;
        tokio::task::spawn_blocking(move || {
            // Keep both permit and temporary directory alive if the awaiting future
            // disappears. Tokio cannot forcibly stop spawn_blocking work.
            let _permit = permit;
            let result = work(source, control);
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                success = result.is_ok(),
                "media operation complete"
            );
            result
        })
        .await
        .context("media worker panicked")?
    }
    pub async fn inspect(&self, source: String) -> Result<Metadata> {
        self.run(source, |path, control| {
            #[cfg(feature = "native")]
            {
                media::inspect(&path, &control)
            }
            #[cfg(not(feature = "native"))]
            {
                let _ = (path, control);
                anyhow::bail!("built without native media support")
            }
        })
        .await
    }
    pub async fn frames(&self, source: String, options: FrameOptions) -> Result<Vec<EncodedFrame>> {
        options.validate()?;
        self.run(source, move |path, control| {
            #[cfg(feature = "native")]
            {
                media::extract(&path, &options, &control)
            }
            #[cfg(not(feature = "native"))]
            {
                let _ = (path, control, options);
                anyhow::bail!("built without native media support")
            }
        })
        .await
    }
    pub async fn transcript(
        &self,
        source: String,
        window: Window,
        language: Option<String>,
    ) -> Result<Transcript> {
        window.validate()?;
        #[cfg(feature = "whisper")]
        let transcriber = self
            .transcriber
            .clone()
            .context("set VIDEO_WHISPER_MODEL to a local ggml model")?;
        self.run(source, move |path, control| {
            #[cfg(feature = "whisper")]
            {
                transcriber.transcribe(&path, &window, language.as_deref(), &control)
            }
            #[cfg(not(feature = "whisper"))]
            {
                let _ = (path, control, window, language);
                anyhow::bail!("built without Whisper support")
            }
        })
        .await
    }
    pub fn from_env() -> Result<Self> {
        let mut limits = SourceLimits {
            allow_urls: std::env::var("VIDEO_ALLOW_URLS").as_deref() == Ok("1"),
            ..Default::default()
        };
        if let Some(p) = std::env::var_os("VIDEO_YT_DLP") {
            limits.yt_dlp = p.into();
        }
        limits.max_bytes = env_number("VIDEO_MAX_BYTES", limits.max_bytes)?;
        limits.timeout = Duration::from_secs(env_number("VIDEO_DOWNLOAD_TIMEOUT", 120)?);
        ensure!(
            limits.max_bytes > 0 && limits.max_bytes <= 4 * 1024 * 1024 * 1024,
            "VIDEO_MAX_BYTES must be 1..4 GiB"
        );
        ensure!(
            (1..=300).contains(&limits.timeout.as_secs()),
            "download timeout must be 1..300 seconds"
        );
        Self::new(
            env_number("VIDEO_JOBS", 2)?,
            limits,
            std::env::var_os("VIDEO_WHISPER_MODEL").map(PathBuf::from),
            env_number("VIDEO_WHISPER_THREADS", 4)?,
        )
    }
    pub async fn watch(
        &self,
        mut request: crate::watch::WatchRequest,
    ) -> Result<crate::watch::WatchPage> {
        request.validate()?;
        #[cfg(not(feature = "native"))]
        {
            let _ = &mut request;
            anyhow::bail!("built without native media support");
        }
        #[cfg(feature = "native")]
        {
            let sessions = self.watches.clone();
            if let Some(source) = request.source.take() {
                return self
                    .run_source(source, move |source, control| {
                        sessions.open(source, &request, &control)
                    })
                    .await;
            }
            let id = request.video_id.clone().context("missing video_id")?;
            let permit = self
                .permits
                .clone()
                .try_acquire_owned()
                .context("server busy; retry after current work")?;
            let control = Control::new(Duration::from_secs(300));
            let _cancel = CancelOnDrop(control.clone());
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                sessions.watch(&id, &request, &control)
            })
            .await
            .context("viewing worker panicked")?
        }
    }
    pub async fn details(
        &self,
        mut request: crate::detail::DetailRequest,
    ) -> Result<crate::detail::DetailPage> {
        request.validate()?;
        #[cfg(not(feature = "native"))]
        {
            let _ = &mut request;
            anyhow::bail!("built without native media support");
        }
        #[cfg(feature = "native")]
        {
            if let Some(source) = request.source.take() {
                return self
                    .run_source(source, move |source, control| {
                        let metadata = media::inspect(&source.path, &control)?;
                        let mut reader = crate::video::VideoReader::open(
                            &source.path,
                            request.timestamps[0],
                            &control,
                        )?;
                        crate::detail::extract(&mut reader, &metadata, &request, &control)
                    })
                    .await;
            }
            let sessions = self.watches.clone();
            let id = request.video_id.clone().context("missing video_id")?;
            let permit = self
                .permits
                .clone()
                .try_acquire_owned()
                .context("server busy; retry after current work")?;
            let control = Control::new(Duration::from_secs(300));
            let _cancel = CancelOnDrop(control.clone());
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                sessions.details(&id, &request, &control)
            })
            .await
            .context("detail worker panicked")?
        }
    }
    pub async fn close_video(&self, id: String) -> Result<bool> {
        #[cfg(not(feature = "native"))]
        {
            let _ = id;
            anyhow::bail!("built without native media support");
        }
        #[cfg(feature = "native")]
        {
            let sessions = self.watches.clone();
            tokio::task::spawn_blocking(move || sessions.close(&id))
                .await
                .context("viewing cleanup panicked")?
        }
    }
}
fn env_number<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match std::env::var(name) {
        Ok(value) => value.parse().map_err(|_| anyhow::anyhow!("invalid {name}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn busy_does_not_queue() {
        let service = Service::new(1, SourceLimits::default(), None, 4).unwrap();
        let _permit = service.permits.clone().acquire_owned().await.unwrap();
        let e = service
            .run("missing".into(), |_, _| Ok(()))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("busy"));
    }
    #[tokio::test]
    async fn dropped_waiter_keeps_worker_permit() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"fixture").unwrap();
        let source = file.path().to_str().unwrap().to_owned();
        let service = Arc::new(Service::new(1, SourceLimits::default(), None, 4).unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let service_for_task = service.clone();
        let task = tokio::spawn(async move {
            service_for_task
                .run(source, move |_, control| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    assert!(control.check().is_err());
                    finished_tx.send(()).unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_eq!(service.permits.available_permits(), 0);
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
        // Acquiring rather than sleeping establishes that the worker released it.
        let _permit = service.permits.acquire().await.unwrap();
    }
}
