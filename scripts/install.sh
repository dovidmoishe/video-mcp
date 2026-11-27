#!/bin/sh
# One-line installer for macOS (Linux is best-effort and untested):
#
#   curl -fsSL https://raw.githubusercontent.com/dovidmoishe/video-mcp/main/scripts/install.sh | sh
#
# Installs Homebrew packages if needed, builds video-mcp with cargo, downloads the
# Whisper speech model, checks that the server starts, and prints agent registration
# commands. Safe to rerun; finished steps are skipped.
#
# Settings (environment variables):
#   VIDEO_MCP_REF              branch, tag or commit to build (default: main)
#   VIDEO_MCP_SOURCE           build this local checkout instead of GitHub
#   VIDEO_MCP_HOME             data folder (default: ~/.local/share/video-mcp)
#   VIDEO_MCP_NO_SPEECH_MODEL  set to 1 to skip the 78 MB speech model
set -eu

REPO_URL="https://github.com/dovidmoishe/video-mcp"
RAW_URL="https://raw.githubusercontent.com/dovidmoishe/video-mcp"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"
MODEL_SHA256="921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"
TOOLS="watch_video inspect_video_frames close_video inspect_video_metadata get_timestamped_transcript extract_keyframes"

say() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        sha256sum "$1" | cut -d ' ' -f 1
    fi
}

main() {
    ref="${VIDEO_MCP_REF:-main}"
    data="${VIDEO_MCP_HOME:-$HOME/.local/share/video-mcp}"
    missing=""
    add_missing() {
        missing="$missing  - $1
"
    }

    # 1. Prerequisites. The server is built against FFmpeg 8 (libavcodec 62).
    say "Checking prerequisites"
    case "$(uname -s)" in
        Darwin)
            xcode-select -p >/dev/null 2>&1 || add_missing "Xcode command line tools: xcode-select --install"
            if command -v brew >/dev/null 2>&1; then
                # Homebrew's default ffmpeg is 9.x; ffmpeg@8 installs beside it.
                packages=""
                brew --prefix --installed ffmpeg@8 >/dev/null 2>&1 || packages="$packages ffmpeg@8"
                command -v pkg-config >/dev/null 2>&1 || packages="$packages pkg-config"
                command -v cmake >/dev/null 2>&1 || packages="$packages cmake"
                if [ -n "$packages" ]; then
                    say "Installing with Homebrew:$packages"
                    # shellcheck disable=SC2086 # Word splitting is intended.
                    brew install $packages </dev/null
                fi
                PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
                export PKG_CONFIG_PATH
            else
                add_missing "Homebrew: see https://brew.sh"
            fi
            ;;
        Linux)
            say "Linux is untested: you need FFmpeg 8 development packages, pkg-config, CMake and clang"
            command -v pkg-config >/dev/null 2>&1 || add_missing "pkg-config"
            command -v cmake >/dev/null 2>&1 || add_missing "CMake"
            ;;
        *)
            fail "unsupported system $(uname -s); on Windows run scripts/install.ps1"
            ;;
    esac
    command -v cargo >/dev/null 2>&1 || add_missing "Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    command -v curl >/dev/null 2>&1 || add_missing "curl"
    if [ -z "$missing" ] && command -v pkg-config >/dev/null 2>&1; then
        version="$(pkg-config --modversion libavcodec 2>/dev/null || true)"
        case "$version" in
            62.*) ;;
            "") add_missing "FFmpeg 8 development files (pkg-config cannot find libavcodec)" ;;
            *) add_missing "FFmpeg 8 development files (found libavcodec $version; FFmpeg 8 provides 62.x)" ;;
        esac
    fi
    if [ -n "$missing" ]; then
        printf 'Setup cannot continue yet. Install these, open a new terminal, then run the installer again:\n%s' "$missing" >&2
        exit 1
    fi

    # 2. Build into cargo's bin folder. A persistent target folder makes reruns fast.
    mkdir -p "$data"
    CARGO_TARGET_DIR="$data/build"
    export CARGO_TARGET_DIR
    if [ -n "${VIDEO_MCP_SOURCE:-}" ]; then
        say "Building $VIDEO_MCP_SOURCE (the first build takes 5-15 minutes)"
        cargo install --locked --path "$VIDEO_MCP_SOURCE"
    else
        case "$ref" in
            v[0-9]*) selector="--tag" ;;
            *[!0-9a-f]* | "") selector="--branch" ;;
            *) selector="--rev" ;; # A hexadecimal commit id.
        esac
        say "Building $REPO_URL at $ref (the first build takes 5-15 minutes)"
        cargo install --locked --git "$REPO_URL" "$selector" "$ref"
    fi
    bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin/video-mcp"
    [ -x "$bin" ] || fail "cargo did not install $bin"

    # 3. Speech model, pinned by SHA-256. curl resumes a partial download on rerun.
    model=""
    if [ "${VIDEO_MCP_NO_SPEECH_MODEL:-}" = 1 ]; then
        say "Skipping the Whisper model (VIDEO_MCP_NO_SPEECH_MODEL=1)"
    else
        model="$data/ggml-tiny.en.bin"
        if [ -f "$model" ] && [ "$(sha256_of "$model")" = "$MODEL_SHA256" ]; then
            say "Whisper model already present: $model"
        else
            say "Downloading the Whisper speech model (78 MB)"
            curl -fL --retry 5 --retry-delay 5 --connect-timeout 30 -C - -o "$model.partial" "$MODEL_URL" \
                || fail "the model download failed; run the installer again to resume it"
            if [ "$(sha256_of "$model.partial")" != "$MODEL_SHA256" ]; then
                rm -f "$model.partial"
                fail "checksum mismatch for the downloaded speech model"
            fi
            mv "$model.partial" "$model"
        fi
    fi

    # 4. Start the server the way an agent will and check that it lists its tools.
    say "Checking that the server starts and lists its tools"
    init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"video-mcp-install","version":"1"}}}'
    ready='{"jsonrpc":"2.0","method":"notifications/initialized"}'
    list='{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
    log="$data/self-test.log"
    if [ -n "$model" ]; then
        listed="$( (printf '%s\n%s\n%s\n' "$init" "$ready" "$list"; sleep 5) | VIDEO_WHISPER_MODEL="$model" "$bin" 2>"$log" || true)"
    else
        listed="$( (printf '%s\n%s\n%s\n' "$init" "$ready" "$list"; sleep 5) | env -u VIDEO_WHISPER_MODEL "$bin" 2>"$log" || true)"
    fi
    for tool in $TOOLS; do
        case "$listed" in
            *"\"$tool\""*) ;;
            *) fail "the server did not list its tool $tool.
Server output: $listed
Server log: $(cat "$log")" ;;
        esac
    done

    # 5. Registration commands with this machine's paths.
    printf '\nReady: %s (6 tools)\n\nRegister it with your agent, then restart the agent:\n\n' "$bin"
    if [ -n "$model" ]; then
        printf 'Codex:\n  codex mcp add video-local --env "VIDEO_WHISPER_MODEL=%s" -- "%s"\n\n' "$model" "$bin"
        printf 'Claude Code:\n  claude mcp add video-local -s user -e "VIDEO_WHISPER_MODEL=%s" -- "%s"\n\n' "$model" "$bin"
        env_json="{ \"VIDEO_WHISPER_MODEL\": \"$model\" }"
    else
        printf 'Codex:\n  codex mcp add video-local -- "%s"\n\n' "$bin"
        printf 'Claude Code:\n  claude mcp add video-local -s user -- "%s"\n\n' "$bin"
        env_json="{}"
    fi
    printf 'Claude Desktop (~/Library/Application Support/Claude/claude_desktop_config.json), Cursor (~/.cursor/mcp.json) and other JSON clients:\n'
    printf '{\n  "mcpServers": {\n    "video-local": {\n      "command": "%s",\n      "args": [],\n      "env": %s\n    }\n  }\n}\n\n' "$bin" "$env_json"
    printf 'Optional: give the agent the video workflow (use ~/.claude/skills for Claude Code):\n'
    printf '  mkdir -p ~/.codex/skills/understand-video && curl -fsSL %s/%s/SKILL.md -o ~/.codex/skills/understand-video/SKILL.md\n' "$RAW_URL" "$ref"
}

main "$@"
