"""Ordinary Codex tools versus video MCP: outcome-based, fresh-context trials.

The baseline has standalone third-party FFmpeg, Python/Pillow and image viewing;
it never receives this project's CLI, source, skill or generated analysis. This
is a local installed-tools benchmark, not a native audiovisual-model benchmark.
Full-access Codex is instruction-isolated, not OS-isolated: review the tool log.
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time
import tomllib


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def parse_events(path):
    events = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    completed = [e["item"] for e in events if e.get("type") == "item.completed"]
    messages = [e.get("text", "") for e in completed if e.get("type") == "agent_message"]
    usage = next((e.get("usage") for e in reversed(events) if e.get("type") == "turn.completed"), None)
    actions = [e for e in completed if e.get("type") not in ("agent_message", "reasoning")]
    return messages[-1] if messages else None, usage, actions


def normalized_command(command):
    # Codex event strings can contain doubled Windows separators.
    return re.sub(r"\\+", lambda _: "/", command).lower()


def run(args):
    root = pathlib.Path(__file__).resolve().parents[1]
    out = pathlib.Path(args.output).resolve()
    # Never mix a partially completed study or earlier answers into a new study.
    out.mkdir(parents=True, exist_ok=False)
    config = tomllib.loads((pathlib.Path.home() / ".codex/config.toml").read_text(encoding="utf-8"))
    model = args.model or config["model"]
    effort = args.effort or config.get("model_reasoning_effort", "high")
    registration = json.loads(subprocess.check_output(["codex", "mcp", "get", "video-local", "--json"], text=True))
    ffmpeg_bin = pathlib.Path(args.ffmpeg_bin).resolve()
    original = pathlib.Path(args.media).resolve()
    server = pathlib.Path(args.server or registration["transport"]["command"]).resolve()
    speech_model = pathlib.Path(args.speech_model).resolve() if args.speech_model else None
    order = args.order or ["baseline", "mcp", "mcp", "baseline"] * args.blocks
    common = """Analyze the local video clip.mp4 in your current working directory.
Inspect the video using the permitted local tools and choose your own method,
sampling, resolution and sequence of operations. Cover the whole clip and inspect
fine details when needed. Do not rely on the filename or knowledge of other runs.
In at most 300 words report:
1. Duration, dimensions and frame rate.
2. The main visible objects and the exact text shown on screen.
3. How the scene changes over time, with approximate timestamps.
4. Any intelligible speech and what is said, only if you can verify it. Distinguish
   direct audio evidence, speech-model captions and unverified audio.
5. Important uncertainty or events your method could miss.
Ground your answer in tool results. Do not invent sound characteristics. Do not
claim continuous playback if you only inspected sampled images.

Use only this run's clip, workspace and permitted tools. Do not read project
source, skills, earlier results, other runs, chat histories or unrelated files.
Do not use network services, download/install packages, start other agents or
change global settings. You may create local scripts and outputs in this workspace.
"""
    if args.prompt_file:
        common = pathlib.Path(args.prompt_file).read_text(encoding="utf-8")
    study = {
        "version": 1, "model": model, "reasoning_effort": effort, "order": order,
        "source_sha256": sha256(original), "source_bytes": original.stat().st_size,
        "output_schema_sha256": sha256(pathlib.Path(args.output_schema)) if args.output_schema else None,
        "codex_version": subprocess.check_output(["codex", "--version"], text=True).strip(),
        "prompt": common, "timeout_seconds": args.timeout,
        "baseline": "Built-in shell/image viewing, ordinary third-party FFmpeg/ffprobe and existing Python/Pillow; no project tools or skill; agent selects its own pipeline.",
        "mcp": "Same built-ins, environment and task, plus the release video-local MCP catalog; agent selects its own calls/settings. Local Whisper model is preinstalled as part of MCP setup.",
        "controls": "Fresh ephemeral contexts, neutral clip name, sequential order recorded above, no supplied sampling plan, startup/network/model latency included, no installs or external media services. Preinstalled tool provisioning excluded from timings.",
        "isolation": "Separate TEMP workspaces and --ignore-user-config; no project cwd. Full-access permissions match both arms. Prompt restrictions and post-run command audit, not hard OS containment.",
        "audit_limit": "Codex exec JSONL does not expose every built-in image-viewing invocation. Shell/MCP counts are not total tool-call counts; retained image outputs and answers also need review.",
        "server_sha256": sha256(server),
        "ffmpeg_sha256": sha256(ffmpeg_bin / "ffmpeg.exe"),
        "ffprobe_sha256": sha256(ffmpeg_bin / "ffprobe.exe"),
        "speech_model_sha256": sha256(speech_model) if speech_model else None,
        "results_interpretation": "Trial counts follow the explicit order; default is two observations per arm per ABBA block. One clip is not a general performance or reasoning-accuracy estimate. Failed/timeout runs retained, never silently excluded.",
    }
    if speech_model:
        study['baseline_audio'] = 'Same upstream ggml Whisper model staged as speech-model.bin in both workspaces; ordinary FFmpeg has its own Whisper filter. Baseline chooses whether/how to use it; no project speech code is available.'
    else:
        study['baseline_audio'] = 'No standalone Python Whisper installation and no model exposed to the baseline. This FFmpeg build includes a Whisper filter but requires a model. Report this setup difference separately from speed.'
    (out / "methodology.json").write_text(json.dumps(study, indent=2), encoding="utf-8")
    results = []
    with tempfile.TemporaryDirectory(prefix="ordinary-video-tools-") as tools_dir:
        neutral_tools = pathlib.Path(tools_dir)
        for path in ffmpeg_bin.iterdir():
            if path.suffix.lower() == ".dll" or path.name in ("ffmpeg.exe", "ffprobe.exe"):
                shutil.copy2(path, neutral_tools / path.name)
        assert sha256(neutral_tools / "ffmpeg.exe") == study["ffmpeg_sha256"]
        # Baseline receives no project paths via media-related environment variables.
        environment = {k: v for k, v in os.environ.items() if not k.startswith("VIDEO_") and k not in ("FFMPEG_DIR", "PYTHONPATH")}
        environment["PATH"] = str(neutral_tools) + os.pathsep + os.pathsep.join(
            p for p in environment["PATH"].split(os.pathsep) if str(root).lower() not in p.lower()
        )
        environment["PYTHONIOENCODING"] = "utf-8"
        for index, arm in enumerate(order, 1):
            with tempfile.TemporaryDirectory(prefix="ordinary-video-case-") as workspace:
                shutil.copy2(original, pathlib.Path(workspace) / "clip.mp4")
                model_path = pathlib.Path(workspace) / 'speech-model.bin'
                if speech_model:
                    shutil.copy2(speech_model, model_path)
                command = ["codex", "exec", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--skip-git-repo-check", "--json", "--color", "never", "--sandbox", "danger-full-access", "-C", workspace, "--model", model, "-c", f"model_reasoning_effort={json.dumps(effort)}", "-c", 'approval_policy="never"']
                if args.output_schema:
                    command += ["--output-schema", str(pathlib.Path(args.output_schema).resolve())]
                instructions = "You have ordinary shell, Python/Pillow, ffmpeg/ffprobe on PATH and local image viewing. Use your normal local video-inspection approach. No project-specific executable, library, helper, skill or MCP tool is permitted."
                if arm == "mcp":
                    command += ["-c", f"mcp_servers.video-local.command={json.dumps(str(server))}", "-c", "mcp_servers.video-local.tool_timeout_sec=300"]
                    mcp_environment = dict(registration["transport"]["env"])
                    if speech_model:
                        mcp_environment['VIDEO_WHISPER_MODEL'] = str(model_path)
                    for key, value in mcp_environment.items():
                        command += ["-c", f"mcp_servers.video-local.env.{key}={json.dumps(value)}"]
                    instructions = "The video-local MCP tools are available. Use them for media inspection; select whichever calls and settings suit the task. Use the returned images directly. Close any viewing handle when finished. Do not read the MCP implementation or skill."
                if speech_model:
                    instructions += '\nThe standard upstream ggml Whisper speech model speech-model.bin is available in this workspace, independent of project code. Use it with your permitted tools if needed.'
                prompt = common + "\nPermitted tools for this run:\n" + instructions
                (out / f"{index}-{arm}.prompt.txt").write_text(prompt, encoding="utf-8")
                stdout_path = out / f"{index}-{arm}.jsonl"
                stderr_path = out / f"{index}-{arm}.stderr.txt"
                print(json.dumps({"event": "run_started", "run": index, "arm": arm}), flush=True)
                started = time.perf_counter()
                started_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
                with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
                    process = subprocess.Popen(command + ["-"], env=environment, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr)
                    try:
                        process.communicate(prompt.encode(), timeout=args.timeout)
                        status = "completed" if process.returncode == 0 else "failed"
                    except subprocess.TimeoutExpired:
                        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
                        process.wait(timeout=15)
                        status = "timeout"
                wall = time.perf_counter() - started
                # Suspended Windows hosts can resume an I/O wait after its
                # deadline. Such an answer is not an on-time completion.
                if wall > args.timeout + 5 and status == "completed":
                    status = "deadline_exceeded"
                response, usage, actions = parse_events(stdout_path)
                if status == "completed" and (response is None or usage is None):
                    status = "incomplete"
                mcp_calls = [a for a in actions if a.get("type") == "mcp_tool_call"]
                commands = [a for a in actions if a.get("type") == "command_execution"]
                audit = {
                    "baseline_has_no_mcp_calls": not mcp_calls if arm == "baseline" else None,
                    "project_path_in_shell_commands": [a.get("command", "") for a in commands if normalized_command(str(root)) in normalized_command(a.get("command", "")) or "video-cli" in a.get("command", "").lower()],
                    "requires_manual_tool_and_answer_review": True,
                }
                if arm == "baseline" and (mcp_calls or audit["project_path_in_shell_commands"]):
                    status = "contaminated"
                result = {"run": index, "arm": arm, "status": status, "exit_code": process.returncode, "wall_seconds": round(wall, 3), "usage": usage, "action_types": [a.get("type") for a in actions], "mcp_calls": len(mcp_calls), "shell_commands": len(commands), "audit": audit, "response": response}
                # Keep scripts/images for reproduction without exposing earlier runs.
                result["started_utc"] = started_utc
                result["ended_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
                shutil.copytree(workspace, out / f"{index}-{arm}-workspace", ignore=shutil.ignore_patterns('speech-model.bin'))
                results.append(result)
                (out / "agent-results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
                (out / f"{index}-{arm}.actions.json").write_text(json.dumps(actions, indent=2), encoding="utf-8")
                print(json.dumps({k: v for k, v in result.items() if k not in ("response", "audit", "action_types")}), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("media")
    parser.add_argument("--ffmpeg-bin", required=True, help="Standalone third-party FFmpeg/ffprobe and runtime DLLs")
    parser.add_argument("--output", default=".local/ordinary-agent-benchmark")
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--blocks", type=int, default=1, help="Number of sequential ABBA blocks (four trials each)")
    parser.add_argument("--speech-model", help="Optional upstream ggml model staged for both arms; does not expose project speech code")
    parser.add_argument("--prompt-file", help="Outcome-based task/questions without reference answers")
    parser.add_argument("--output-schema", help="JSON Schema for structured final answers")
    parser.add_argument("--server", help="Override the tested MCP executable without changing client registration")
    parser.add_argument("--model", help="Explicit model override for both arms")
    parser.add_argument("--effort", help="Explicit reasoning effort for both arms")
    parser.add_argument("--order", nargs="+", choices=["baseline", "mcp"], help="Explicit trial order; default remains ABBA")
    args = parser.parse_args()
    if not 1 <= args.blocks <= 5 or not 30 <= args.timeout <= 900:
        parser.error("blocks must be 1..5 and timeout 30..900 seconds")
    run(args)
