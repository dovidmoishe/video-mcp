"""Exercise a real stdio server, including image content. No third-party Python deps."""
import argparse
import base64
import json
import pathlib
import os
import subprocess
import queue
import threading

parser = argparse.ArgumentParser()
parser.add_argument("--server", default="target/debug/video-mcp.exe")
parser.add_argument("--media", default=".local/fixtures/av.mp4")
args = parser.parse_args()
process = subprocess.Popen([str(pathlib.Path(args.server).resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
responses = queue.Queue()
def read_responses():
    for line in process.stdout:
        responses.put(line)
    responses.put(None)
threading.Thread(target=read_responses, daemon=True).start()

def send(message):
    process.stdin.write((json.dumps(message) + "\n").encode())
    process.stdin.flush()

def request(i, method, params):
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    while True:
        line = responses.get(timeout=60)
        if not line:
            raise RuntimeError(f"server exited {process.poll()}")
        response = json.loads(line)
        if response.get("id") == i:
            assert "error" not in response, response
            return response["result"]

try:
    request(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "video-smoke", "version": "1"}})
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    tools = request(2, "tools/list", {})["tools"]
    assert {t["name"] for t in tools} == {"inspect_video_metadata", "extract_keyframes", "get_timestamped_transcript", "watch_video", "close_video", "inspect_video_frames"}
    source = str(pathlib.Path(args.media).resolve())
    metadata = request(3, "tools/call", {"name": "inspect_video_metadata", "arguments": {"source": source}})
    assert not metadata.get("isError"), metadata
    assert json.loads(metadata["content"][0]["text"])["streams"]
    frames = request(4, "tools/call", {"name": "extract_keyframes", "arguments": {"source": source, "options": {"count": 2, "window": {"start": 0.5, "end": 1.5}}}})
    assert not frames.get("isError"), frames
    images = [c for c in frames["content"] if c["type"] == "image"]
    assert len(images) == 2
    assert all(i["mimeType"] == "image/jpeg" and base64.b64decode(i["data"]).startswith(b"\xff\xd8") for i in images)
    bad = request(5, "tools/call", {"name": "extract_keyframes", "arguments": {"source": source, "options": {"count": 33}}})
    assert bad["isError"]
    if os.environ.get("VIDEO_WHISPER_MODEL"):
        speech = request(6, "tools/call", {"name": "get_timestamped_transcript", "arguments": {"source": str(pathlib.Path(".local/fixtures/jfk.wav").resolve()), "window": {"start": 0, "end": 15}, "language": "en"}})
        assert not speech.get("isError"), speech
        transcript = json.loads(speech["content"][0]["text"])
        assert any("country" in s["text"].lower() for s in transcript["segments"])
        print("PASS: timestamped speech over MCP stdio without protocol contamination")
    print("PASS: initialize, exact tools, metadata, two JPEG images, bounded tool error")
finally:
    process.stdin.close()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
