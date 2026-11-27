# Local video understanding MCP

A local Rust MCP server that lets a coding agent inspect video. It provides metadata, timestamped JPEG/WebP frames, filmstrip-style temporal viewing with change hints, full-resolution crops with optional local OCR and patch tracking, and local Whisper transcription. The connected agent decides what to inspect and interprets the evidence; the server never calls a model or launches an agent. Python is used only for developer checks, not at runtime.

It is a single Rust crate, `video-mcp`. The media engine lives in `src/`, the MCP tool layer in `src/mcp.rs`, and the stdio entry point in `src/main.rs`. [AGENTS.md](AGENTS.md) covers the layout, invariants and contributor workflow. [SKILL.md](SKILL.md) is the tool-usage workflow to give agents.

## Install

Each installer builds the server from source, downloads the Whisper speech model, checks that the server starts, and prints the registration commands for your agent with your paths filled in. The first build takes 5–20 minutes. Rerunning an installer updates the server, and finished steps are skipped.

### macOS (Apple silicon)

Install Xcode's command line tools (`xcode-select --install`), [Homebrew](https://brew.sh) and [Rust](https://rustup.rs), then:

```sh
curl -fsSL https://raw.githubusercontent.com/dovidmoishe/video-mcp/main/scripts/install.sh | sh
```

- **Dependencies:** installs `ffmpeg@8`, `pkg-config` and `cmake` with Homebrew if needed. Homebrew's default `ffmpeg` is 9.x, which this server does not support yet; `ffmpeg@8` installs beside it.
- **Where things go:** the server is built with `cargo install` into `~/.cargo/bin/video-mcp`. The speech model and build cache go in `~/.local/share/video-mcp/`.
- **OCR:** local OCR is Windows-only for now. On macOS, `include_ocr` reports OCR as unavailable; everything else works.
- **Options:** set `VIDEO_MCP_NO_SPEECH_MODEL=1` to skip the 78 MB model, or `VIDEO_MCP_REF=<branch, tag or commit>` to install another version.

### Windows 10/11 (x64)

Install the build tools, skipping any you already have, then open a new terminal:

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install LLVM.LLVM
winget install Kitware.CMake
```

Then:

```powershell
irm https://raw.githubusercontent.com/dovidmoishe/video-mcp/main/scripts/install.ps1 | iex
```

This downloads the source into `%LOCALAPPDATA%\video-mcp` and runs `scripts\setup.ps1` there. Setup does the following:
- Checks the prerequisites and lists anything missing, with the install command for each.
- Downloads the FFmpeg 8 shared development files (about 90 MB) and the speech model, verifying pinned SHA-256 hashes.
- Builds the release server and checks that it starts.

To work from a clone instead, clone to a short path such as `C:\src\video-mcp` and run `powershell -ExecutionPolicy Bypass -File scripts\setup.ps1`. whisper.cpp's MSBuild build fails when the build folder's path is long (Windows' 260-character limit), and setup stops early if it would. Setup options:
- `-NoSpeechModel` skips the speech model.
- `-TargetDirectory target/<name>` builds into a new folder while an agent still runs the old executable, which Windows keeps locked.

### With cargo

On macOS:

```sh
brew install ffmpeg@8 pkg-config cmake
PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig" cargo install --locked --git https://github.com/dovidmoishe/video-mcp
```

Then download [ggml-tiny.en.bin](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin) and point `VIDEO_WHISPER_MODEL` at it if you want speech.
- **Windows:** `cargo install` needs `FFMPEG_DIR` set to FFmpeg 8 shared development files, `LIBCLANG_PATH`, CMake on PATH, and FFmpeg's `bin` on the server's PATH at runtime. The Windows installer above sets all of that up, so prefer it.
- **Linux:** untested. It needs FFmpeg 8 development packages that pkg-config can find, CMake and clang.

### Register with your agent

Paste the command the installer printed, then restart the agent. They have this shape:
- Codex: `codex mcp add video-local --env "VIDEO_WHISPER_MODEL=<model>" -- "<path to video-mcp>"`
- Claude Code: `claude mcp add video-local -s user -e "VIDEO_WHISPER_MODEL=<model>" -- "<path to video-mcp>"`
- Claude Desktop, Cursor (`~/.cursor/mcp.json`) and other JSON clients: add the printed `video-local` entry under `mcpServers`. Claude Desktop's config file is `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS and `%APPDATA%\Claude\claude_desktop_config.json` on Windows.

On Windows the entry also sets `PATH` to include FFmpeg's `bin` folder, so the server can load the FFmpeg DLLs. The server speaks MCP over stdio: standard output carries only JSON-RPC, and logs go to stderr. Give it absolute local video paths.

### Optional: install the skill

[SKILL.md](SKILL.md) teaches the agent the recommended video workflow. Put it in `~/.codex/skills/understand-video/SKILL.md` for Codex or `~/.claude/skills/understand-video/SKILL.md` for Claude Code; the installers print the exact command.

## Manual build (Windows)

The setup script wraps these steps. On macOS, the cargo commands above are the whole manual build. Versions are pinned to ffmpeg-next 8.0.0, whisper-rs 0.16.0 and rmcp 0.8.5; upgrading a native ABI or wrapper major version requires repeating the native validation. The build needs:
- FFmpeg 8 **shared development** files: headers, `.lib` import libraries and DLLs. An ffmpeg.exe-only installation is insufficient.
- LLVM/libclang for the bindings.
- CMake for whisper.cpp.

In PowerShell from the project root:

```powershell
$env:FFMPEG_DIR = (Resolve-Path '.local/ffmpeg-8.0-full_build-shared').Path
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"   # CMake must also be on PATH
$env:VIDEO_WHISPER_MODEL = (Resolve-Path '.local/ggml-tiny.en.bin').Path
cargo build
```

Sources and licensing:
- The tested FFmpeg is [Gyan FFmpeg 8.0 shared](https://github.com/GyanD/codexffmpeg/releases/tag/8.0); the speech model is [ggml-tiny.en](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin). Both stay in `.local/`, which Git ignores.
- Check distributor licenses before redistributing native binaries. The Gyan shared build is GPL licensed, and this crate's MIT license does not change dependency licenses.

For normal use and any performance measurement, build with `./scripts/build-release.ps1` and run `target/release/video-mcp.exe`. Debug builds encode images much more slowly. Plain Cargo release builds can lose the native Whisper `/O2` flags under the pinned build dependencies. The helper verifies those flags, repairs an unoptimized native cache, confines cleanup to the project's target directory and restores the calling shell environment.

The registration JSON has this shape, with `VIDEO_JOBS`/`VIDEO_WHISPER_THREADS` optional (see Configuration):

```json
{
  "mcpServers": {
    "video-local": {
      "command": "C:\\path\\to\\video-mcp\\target\\release\\video-mcp.exe",
      "args": [],
      "env": {
        "PATH": "C:\\path\\to\\video-mcp\\.local\\ffmpeg-8.0-full_build-shared\\bin;C:\\Windows\\System32;C:\\Windows",
        "VIDEO_WHISPER_MODEL": "C:\\path\\to\\video-mcp\\.local\\ggml-tiny.en.bin",
        "VIDEO_JOBS": "2",
        "VIDEO_WHISPER_THREADS": "4"
      }
    }
  }
}
```

## Tools

- `watch_video`: `{ "source": "C:/media/demo.mp4", "seconds": 15, "fps": 1, "width": 320 }`. Returns metadata, a periodic filmstrip plus bounded event evidence, actual sample times with estimated flags, and change candidates from every decoded frame.
  - Reuse `{ "video_id": "<returned handle>" }` to advance, or add `start` to seek/rewind. `next_start: null` signals the end.
  - Limits: at most 16 periodic samples (`seconds*fps <= 16`), 0.5–30 fps, 0.25–30 seconds, 160–640px width. Start at width 640 for text/detail questions.
  - Event evidence defaults to two candidates (up to six extra samples); `event_budget: 0` is cheapest and 4 is the maximum.
  - Speech is off by default; `include_speech: true` adds overlapping timestamped speech context. Cached speech segments keep their original timestamps and can extend beyond the visual window.
  - Captions and change candidates are evidence to inspect, not sound or event classifications.
- `inspect_video_frames`: `{ "video_id": "<handle>", "timestamps": [3.5, 3.6, 3.7], "crop": {"x": 0.2, "y": 0.3, "width": 0.5, "height": 0.3}, "include_ocr": true }`.
  - `crop` is a normalized full-frame rectangle in display orientation, applied at source resolution before resizing. Images, `source_pixels`, OCR words, motion and tentative text changes use actual timestamps and the same coordinates.
  - Accepts `source` instead of a handle; 1..8 times spanning <=30s; output sides <=2048.
  - Exact handle repeats reuse the detail cache, and inspection keeps the overview playhead.
  - Optional `track_region` inside the crop tracks a selected appearance patch with explicit uncertainty, not semantic identity.
- `close_video`: `{ "video_id": "<returned handle>" }`. Releases the decoder, cached data and any downloaded source. At most four handles are retained; handles expire after ten idle minutes and are pruned on the next viewing operation. Handles are process-local.
- `inspect_video_metadata`: `{ "source": "C:/media/demo.mp4" }`. Video streams report:
  - the coded `width`/`height`;
  - `rotation`: the display-matrix angle as ffprobe reports it, in counterclockwise degrees, so an iPhone portrait clip shows -90;
  - `display_transform`, for example `rotate_90_clockwise`;
  - the upright `display_width`/`display_height`.
- `get_timestamped_transcript`: `{ "source": "C:/media/demo.mp4", "window": { "start": 0, "end": 60 }, "language": "en" }`. Omit `language` for detection.
- `extract_keyframes`: `{ "source": "C:/media/demo.mp4", "options": { "window": { "start": 10, "end": 20 }, "count": 6, "max_width": 960, "max_height": 540, "format": "jpeg" } }`.
  - Optional `interval` >=0.001s sets the target sampling spacing; `count` remains the hard limit.
  - Both output sides accept 1..2048, including portrait layouts.
  - An optional display-orientation `crop` preserves small details before resizing.

Every frame-producing tool applies the video stream's display matrix the way players and the ffmpeg CLI's autorotation do. A portrait phone clip stored as 1920x1080 with rotation -90 is returned as upright 1080x1920 frames; 180-degree rotations and flips are applied too. All coordinates use that display orientation: `crop`, `track_region`, change-candidate `region`, OCR word boxes, motion regions and `source_pixels`. The server maps them to stored pixels internally, so normalized coordinates always describe what the returned images show. A matrix angle that is not a multiple of 90 degrees is reported in metadata but not applied.

Each returned image follows a text block giving its actual timestamp, its dimensions and whether timing was estimated. Tool failures use MCP `isError` results. Where a window applies, the defaults cover the first 30 seconds. This is a local trusted-agent tool: any accessible local file path can be requested, and it is not a multiuser filesystem security boundary.

### Viewing and evidence

`watch_video` returns a periodic row-major filmstrip and an optional event filmstrip, both with numeric timestamp headers and a machine-readable manifest.
- **Sessions:** a viewing session keeps the demuxer and decoder open and streams raw RGB samples into one composite. Forward viewing continues decoding; rewinds and large jumps seek. The last page is cached and replays with `cache_hit` and zero decoded frames.
- **Cells:** cell size follows the display aspect ratio, with the long edge at most 1280. Each composite is limited to 12 megapixels and a 2 MiB JPEG.
- **Change scan:** every decoded frame is scanned at 96x96 with a 16x16 grid of 6x6-pixel tiles, plus a half-second baseline for gradual change. Up to 64 nonsemantic candidates and at most four event triplets are returned. They are ranked by change strength rather than task relevance, so small or low-contrast events can be missed.

The agent receives sampled images and text, not native video or audio perception.

Local OCR uses the installed Windows user-profile language packs through WinRT, with no network service or model download; other platforms return an explicit OCR-unavailable result. Appearance tracking uses a <=192px analysis image and a bounded local search. Occlusion, similar objects, scale or shape changes and large jumps can produce loss or ambiguity. Treat OCR readings, tracks and text associations as evidence to check against the images, not as semantic recognition.

## Configuration and URLs

| Variable | Default | Range / notes |
|---|---|---|
| `VIDEO_JOBS` | 2 | 1–4 |
| `VIDEO_WHISPER_THREADS` | 4 | 1–16 |
| `VIDEO_MAX_BYTES` | 536870912 | up to 4 GiB; applies to local and downloaded files |
| `VIDEO_DOWNLOAD_TIMEOUT` | 120 seconds | 1–300 |

The overall cooperative operation deadline is 300 seconds. `RUST_LOG=info` enables timing diagnostics.

URLs are disabled by default. To enable them, install [yt-dlp](https://github.com/yt-dlp/yt-dlp), set `VIDEO_ALLOW_URLS=1`, and optionally set `VIDEO_YT_DLP` to its executable path. Downloads are temporary and not cached; for repeated analysis, use a local file.
- **Supervisor limits:** one download at a time, plus limits on file size, retries, execution time and directory size. These are monitored periodically and are not hard disk quotas.
- **Process cleanup:** a Windows Job Object terminates the downloader's whole process tree, and the downloader's temporary runtime files stay inside the owned source directory.
- **Formats:** a single combined video/audio format and the native downloader avoid a merger subprocess. Sites that offer only separate streams may fail or give lower quality.
- **Security:** URLs, extractors and redirects can reach the network, including local addresses. There is no network sandbox, so enable URLs only for trusted callers.

## Development

```powershell
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-default-features
./scripts/make-fixtures.ps1
python scripts/make-visual-fixtures.py
$env:VIDEO_FIXTURES = (Resolve-Path '.local/fixtures').Path
cargo test -- --include-ignored
python -m unittest discover -s tests -p 'test_*.py'
python scripts/mcp-smoke.py --server target/debug/video-mcp.exe
python scripts/visual-smoke.py --server target/release/video-mcp.exe
python scripts/url-smoke.py --server target/debug/video-mcp.exe
```

The ignored tests need the generated fixtures, FFmpeg, the Whisper model and `.local/fixtures/jfk.wav` (from whisper.cpp's `samples/` folder). The OCR test also needs an installed Windows OCR language and runs only on Windows.

On macOS, first export `PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig"`. Run the PowerShell fixture script with PowerShell 7 (`brew install powershell`), passing Homebrew's FFmpeg 8: `pwsh scripts/make-fixtures.ps1 -FFmpeg "$(brew --prefix ffmpeg@8)/bin/ffmpeg"`, and pass the same path to `make-visual-fixtures.py --ffmpeg`. CI (`.github/workflows/ci.yml`) runs the full suite on macOS and the one-line installer on both platforms. `visual-smoke.py` and `watch-smoke.py` take their server environment from the Codex `video-local` registration, so they need the `codex` CLI. Without default features, native processing is deliberately unavailable and returns explicit errors, but the MCP schemas and pure Rust contracts still test. With FFmpeg but no Whisper build, use `--no-default-features --features native`.

Profiling calls the server over MCP with the environment above:

```powershell
python scripts/profile.py .local/fixtures/av.mp4 --server target/release/video-mcp.exe
python scripts/benchmark-media.py .local/fixtures/av.mp4 --server target/release/video-mcp.exe --repeats 3 --watch
```

`scripts/benchmark-agent.py` compares real Codex sessions that use ordinary standalone FFmpeg/Python/image viewing against sessions that use this server, in sequential ABBA blocks:

```powershell
python scripts/benchmark-agent.py C:\media\demo.mp4 --ffmpeg-bin .local/ffmpeg-8.0-full_build-shared/bin --speech-model .local/ggml-tiny.en.bin --output .local/new-agent-benchmark
```

The output directory must be new.
- **Setup:** the runner reads the configured Codex model/effort and the `video-local` registration, stages standalone FFmpeg outside the project and strips project media variables from the baseline. `--speech-model` stages the same ggml model for both arms.
- **Output:** it saves prompts, raw JSONL, commands, final responses and workspace files.
- **Baseline isolation:** the baseline gets no project skill or source, but it runs with full-access permissions, so audit its commands.

`python scripts/report-agent-benchmark.py --data <run> --canvas <file>` renders a saved run.
