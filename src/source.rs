use crate::control::Control;
use anyhow::{Context, Result, bail, ensure};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    process::Command,
    time::{Instant, sleep},
};

#[derive(Clone)]
pub struct SourceLimits {
    pub max_bytes: u64,
    pub timeout: Duration,
    pub yt_dlp: PathBuf,
    pub allow_urls: bool,
}
impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024 * 1024,
            timeout: Duration::from_secs(120),
            yt_dlp: "yt-dlp".into(),
            allow_urls: false,
        }
    }
}
pub struct Source {
    pub path: PathBuf,
    _temporary: Option<TempDir>,
}
// Reap the downloader before removing files, including when its caller is dropped.
struct Download {
    child: Option<Box<dyn ChildWrapper>>,
    dir: Option<TempDir>,
}
impl Drop for Download {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let dir = self.dir.take();
            tokio::spawn(async move {
                let _ = Box::into_pin(child.kill()).await;
                let _ = child.wait().await;
                drop(dir);
            });
        }
    }
}
impl Source {
    pub async fn resolve(input: &str, limits: &SourceLimits, control: &Control) -> Result<Self> {
        control.check()?;
        if input.starts_with("http://") || input.starts_with("https://") {
            ensure!(
                limits.allow_urls,
                "URL ingestion is disabled; set VIDEO_ALLOW_URLS=1"
            );
            let url = url::Url::parse(input)?;
            ensure!(
                url.username().is_empty() && url.password().is_none() && url.host_str().is_some(),
                "invalid URL credentials or host"
            );
            let dir = tempfile::tempdir()?;
            let media_dir = dir.path().join("download");
            let runtime_dir = dir.path().join("runtime");
            std::fs::create_dir(&media_dir)?;
            std::fs::create_dir(&runtime_dir)?;
            let output = media_dir.join("media.%(ext)s");
            let mut cmd = Command::new(&limits.yt_dlp);
            cmd.args([
                "--ignore-config",
                "--no-playlist",
                "--downloader",
                "native",
                "--max-downloads",
                "1",
                "--no-progress",
                "--no-warnings",
                "--no-part",
                "--no-cache-dir",
                "--no-write-info-json",
                "--socket-timeout",
                "15",
                "--retries",
                "1",
                "--fragment-retries",
                "1",
                "--concurrent-fragments",
                "1",
                "--max-filesize",
            ])
            .arg(limits.max_bytes.to_string())
            .args(["--format", "best[ext=mp4]/best", "--output"])
            .arg(output)
            .arg("--")
            .arg(input)
            .env("TMP", &runtime_dir)
            .env("TEMP", &runtime_dir)
            .env("TMPDIR", &runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
            // Do not launch an external downloader/merger: use a single combined format.
            let mut wrapped = CommandWrap::from(cmd);
            wrapped.wrap(KillOnDrop);
            #[cfg(windows)]
            wrapped.wrap(process_wrap::tokio::JobObject);
            #[cfg(unix)]
            wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
            let child = wrapped
                .spawn()
                .context("start yt-dlp (install it or configure VIDEO_YT_DLP)")?;
            let mut download = Download {
                child: Some(child),
                dir: Some(dir),
            };
            let deadline = Instant::now() + limits.timeout;
            loop {
                let child = download.child.as_mut().unwrap();
                if let Err(error) = control.check() {
                    Box::into_pin(child.kill()).await?;
                    child.wait().await?;
                    download.child.take();
                    return Err(error);
                }
                if Instant::now() >= deadline || directory_bytes(&media_dir)? > limits.max_bytes {
                    Box::into_pin(child.kill()).await?;
                    child.wait().await?;
                    download.child.take();
                    bail!("download exceeded size or execution limit");
                }
                if let Some(status) = child.try_wait()? {
                    download.child.take();
                    // yt-dlp exits 101 after completing --max-downloads=1.
                    // Still require exactly one nonempty file within limits below.
                    ensure!(
                        status.success() || status.code() == Some(101),
                        "yt-dlp failed: {status}"
                    );
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
            download.child.take(); // Already reaped by try_wait.
            let dir = download.dir.take().unwrap();
            let mut files = std::fs::read_dir(&media_dir)?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            ensure!(
                files.len() == 1,
                "download must produce exactly one media file"
            );
            let path = files.pop().unwrap();
            validate_local(&path, limits.max_bytes)?;
            Ok(Self {
                path,
                _temporary: Some(dir),
            })
        } else {
            let path = std::fs::canonicalize(input).context("resolve local media path")?;
            validate_local(&path, limits.max_bytes)?;
            Ok(Self {
                path,
                _temporary: None,
            })
        }
    }
}
fn validate_local(path: &Path, max_bytes: u64) -> Result<()> {
    let m = std::fs::metadata(path)?;
    ensure!(
        m.is_file() && m.len() > 0 && m.len() <= max_bytes,
        "media must be a nonempty regular file within the size limit"
    );
    Ok(())
}
fn directory_bytes(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    for e in std::fs::read_dir(path)? {
        total = total.saturating_add(e?.metadata()?.len());
    }
    Ok(total)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn local_and_limits() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"media").unwrap();
        let c = Control::new(Duration::from_secs(5));
        let mut l = SourceLimits::default();
        assert!(
            Source::resolve(f.path().to_str().unwrap(), &l, &c)
                .await
                .is_ok()
        );
        l.max_bytes = 2;
        assert!(
            Source::resolve(f.path().to_str().unwrap(), &l, &c)
                .await
                .is_err()
        );
        assert!(
            Source::resolve("https://example.com/v", &l, &c)
                .await
                .is_err()
        );
    }
}
