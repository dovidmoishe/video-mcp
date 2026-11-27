"""Exercise real temporal viewing, replay, focus, and release over MCP stdio."""
import argparse
import base64
import json
import os
import pathlib
import subprocess
from mcp_client import Client

parser = argparse.ArgumentParser()
parser.add_argument("media")
parser.add_argument("--server", default="target/release/video-mcp.exe")
parser.add_argument("--output", default=".local/watch")
args = parser.parse_args()
output = pathlib.Path(args.output).resolve(); output.mkdir(parents=True, exist_ok=True)
registration = json.loads(subprocess.check_output(["codex", "mcp", "get", "video-local", "--json"], text=True))
environment = os.environ.copy(); environment.update(registration["transport"]["env"])
with (output/"server.log").open("w", encoding="utf-8") as log:
    client = Client(pathlib.Path(args.server).resolve(), environment, log)
    try:
        request = {"source": str(pathlib.Path(args.media).resolve()), "seconds": 15, "fps": 1, "language": "en", "include_speech": True}
        response = client.call("watch_video", request)
        page = json.loads(response["content"][0]["text"])
        assert page["metadata"] and page["frames"] and not page["cache_hit"]
        images = [c for c in response["content"] if c["type"] == "image"]
        assert len(images) == 1 + bool(page.get("event_frames")) and images[0]["mimeType"] == "image/jpeg"
        (output/"overview.jpg").write_bytes(base64.b64decode(images[0]["data"]))
        (output/"overview.json").write_text(json.dumps(page, indent=2), encoding="utf-8")
        replay = client.call("watch_video", {"video_id": page["video_id"], "start": 0, "seconds": 15, "fps": 1, "language": "en", "include_speech": True})
        replay_page = json.loads(replay["content"][0]["text"])
        assert replay_page["cache_hit"] and replay_page["decoded_frames"] == 0
        assert replay["content"][1]["data"] == images[0]["data"]
        focus_start = min(1.0, page["window"]["end"]/4)
        focus = client.call("watch_video", {"video_id": page["video_id"], "start": focus_start, "seconds": 0.5, "fps": 8, "include_speech": False})
        focus_page = json.loads(focus["content"][0]["text"])
        assert focus_page["seek_performed"] and len(focus_page["frames"]) == 4
        closed = client.call("close_video", {"video_id": page["video_id"]})
        assert json.loads(closed["content"][0]["text"])["closed"]
        print(json.dumps({"frames": len(page["frames"]), "decoded_frames": page["decoded_frames"], "scanned_until": page["scanned_until"], "change_candidates": len(page["change_candidates"]), "filmstrip_bytes": len(base64.b64decode(images[0]["data"])), "audio_status": page["audio_status"], "speech_detected": (page["transcript"] or {}).get("speech_detected"), "replay_decoded_frames": replay_page["decoded_frames"]}, indent=2))
        print("PASS: temporal view, labeled JPEG, replay without decoding, higher-rate rewind, release")
    finally:
        client.close()
