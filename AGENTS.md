# AGENTS.md

Local Rust MCP server for video inspection. [README.md](README.md) covers setup, client registration and the tool contract. This file is the working guide for changing the code.

## Layout

One crate, `video-mcp`, with a library and a binary.

- `src/main.rs`: the stdio entry point. It sets up stderr logging and serves `mcp::VideoServer`.
- `src/mcp.rs`: the MCP tool layer (tool schemas, descriptions and result formatting).
- `src/lib.rs`: the module list. Every other module is the media engine:
  - `types.rs`: request and metadata types and their validation (`Window`, `FrameOptions`, `Region`, `StreamMetadata`).
  - `media.rs`: FFmpeg init and probing (including the display matrix), the stateless frame-extraction entry point, and audio decoding/resampling to 16 kHz mono.
  - `video.rs`: `VideoReader`, a persistent demuxer/decoder with seek, flush, EOF handling and the every-frame change scan. `FrameRenderer` handles RGB conversion, crop, scale and the display transform.
  - `orientation.rs`: `DisplayTransform`, the eight right-angle display-matrix transforms, chosen as the ffmpeg CLI's autorotation chooses them. Pure Rust.
  - `watch.rs`: viewing sessions and handles, filmstrips, the replay cache and speech context.
  - `detail.rs`: targeted crops, OCR and tracking requests.
  - Other modules:

    | Module | Purpose |
    |---|---|
    | `visual.rs` | change detector |
    | `ocr.rs` | Windows WinRT OCR |
    | `tracking.rs` | patch tracker |
    | `speech.rs` | Whisper |
    | `source.rs` | local files and opt-in yt-dlp downloads |
    | `service.rs` | admission, cancellation and the blocking-worker boundary |
    | `control.rs`, `timestamp.rs` | helpers |
- `tests/`: `native.rs` holds the fixture-based native tests, `#[ignore]` by default. `test_mcp_client.py` holds the Python unit tests for `scripts/mcp_client.py`. Cargo only reads the `.rs` files there.
- `scripts/`:

  | Purpose | Scripts |
  |---|---|
  | One-line installers (`curl \| sh` on macOS, `irm \| iex` on Windows) | `install.sh`, `install.ps1` |
  | Windows setup (prerequisite check, pinned downloads, build, self-test) | `setup.ps1` |
  | Release build | `build-release.ps1` |
  | Fixtures | `make-fixtures.ps1`, `make-visual-fixtures.py` |
  | MCP smoke checks | `mcp-smoke.py`, `visual-smoke.py`, `watch-smoke.py`, `url-smoke.py` |
  | Shared client | `mcp_client.py` |
  | Profiling | `profile.py`, `benchmark-media.py` |
  | Agent benchmark | `benchmark-agent.py`, `report-agent-benchmark.py` |
- `SKILL.md`: the agent-facing workflow for the tools. Keep it in step with tool behavior and limits.
- Ignored: `.local/` holds the FFmpeg dev build, CMake, models, fixtures and run evidence; `target/` holds build output.

## Environment and checks

`powershell -ExecutionPolicy Bypass -File scripts\setup.ps1` fetches the pinned FFmpeg and Whisper files into `.local/` and builds once. For a development shell, also put CMake on PATH:

```powershell
$env:FFMPEG_DIR = (Resolve-Path '.local/ffmpeg-8.0-full_build-shared').Path
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
$env:VIDEO_WHISPER_MODEL = (Resolve-Path '.local/ggml-tiny.en.bin').Path
$env:VIDEO_FIXTURES = (Resolve-Path '.local/fixtures').Path   # after generating fixtures
```

The native tests also need `.local/fixtures/jfk.wav`, from whisper.cpp's `samples/` folder.

On macOS, install `brew install ffmpeg@8 pkg-config cmake powershell` and export `PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig"`. Pass `$(brew --prefix ffmpeg@8)/bin/ffmpeg` to both fixture scripts. CI (`.github/workflows/ci.yml`) runs the one-line installers on macOS and Windows, plus the full test suite on macOS, for every pull request.

Before finishing a change, run:
- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test -- --include-ignored`
- `cargo test --no-default-features`

Rerun `./scripts/make-fixtures.ps1` and `python scripts/make-visual-fixtures.py` after changing them. For server-visible changes, also build a release and run the smoke scripts against it (see README.md).

## Invariants

- **Engine/MCP boundary:** only `src/mcp.rs` and `src/main.rs` use rmcp. Engine modules must not import `mcp`; they expose plain Rust types that `mcp.rs` formats. This boundary used to be a separate crate, so nothing but review enforces it now.
- **Coordinates:** every normalized region and pixel rectangle in requests and results is full-frame and in display orientation (`crop`, `track_region`, change `region`, OCR boxes, motion regions, `source_pixels`). Only `FrameRenderer` maps to stored pixels, through `DisplayTransform`; never return stored-space coordinates. Metadata `width`/`height` are the coded size; size frames from `display_width`/`display_height`.
- **Bounds:** validated in `types.rs`, `watch.rs` and `detail.rs`. Keep the schemars ranges, tool descriptions, README.md and SKILL.md consistent with them.
- **Stdout:** reserved for JSON-RPC; log to stderr.
- **Native work:** runs on admitted `spawn_blocking` workers owned by `Service`. Check `Control` between packets, frames and chunks; cancellation is cooperative.
- **Scalers:** `SwsContext` is not `Send`, so create scalers per call. Never add `unsafe impl Send`.
- **Unsafe:** outside tests, unsafe is limited to `media::origin` and `media::display_orientation`. Any new block needs a `SAFETY:` comment.
- **Session drop order:** session field order is deliberate. The reader drops before the source, which may own a temporary download directory.

## Gotchas

- **Platforms:** Windows and macOS (Apple silicon) are supported; Linux is untested. OCR uses Windows WinRT, and other platforms report it unavailable. The bindings target FFmpeg 8 (libavcodec 62). Homebrew's default `ffmpeg` is 9.x and unsupported, so use the side-by-side `ffmpeg@8`.
- **Short paths:** keep the Cargo target folder's full path at or under 99 characters, including in worktrees and temp clones. whisper.cpp builds through MSBuild, whose file tracker fails past 260 characters (FTK1011, then CMake's "No CMAKE_C_COMPILER could be found"). `setup.ps1` checks this up front.
- **Locked executables:** an MCP client locks the executable it runs. Build into `./scripts/build-release.ps1 -TargetDirectory target/<new>` instead of stopping the client. Don't edit client configs such as `~/.codex/config.toml` unless asked.
- **PowerShell redirection:** in Windows PowerShell 5.1, `$ErrorActionPreference = 'Stop'` turns a native command's redirected stderr into a fatal error. Scripts here run cargo under `'Continue'` and check `$LASTEXITCODE`; keep that pattern for any native command that writes to stderr.
- **Pinned downloads:** `setup.ps1` pins the FFmpeg archive and Whisper model by SHA-256. Update the URL and hash together, and rerun the native tests, when changing either.
- **FFmpeg display options:** in FFmpeg 8, `-display_rotation`, `-display_hflip` and `-display_vflip` are input options: `ffmpeg -display_rotation 90 -i in.mp4 -c copy out.mp4`.
- **Fixture scripts:** they overwrite the generated files in `.local/fixtures` and leave hand-added files such as `jfk.wav` alone.
- **Git:** don't run `git clean`, `git reset --hard`, `git checkout --` or `git stash`; untracked work may be unrecoverable.
