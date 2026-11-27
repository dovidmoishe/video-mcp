"""Real yt-dlp integration through MCP against a local HTTP fixture server."""

import argparse
import functools
import http.server
import json
import os
import pathlib
import tempfile
import threading
import time
from mcp_client import Client

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--server", default="target/debug/video-mcp.exe")
args = parser.parse_args()


class FixtureHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/hang.mp4":
            time.sleep(5)
            return
        super().do_GET()

    def log_message(self, *_):
        pass


server = http.server.ThreadingHTTPServer(
    ("127.0.0.1", 0),
    functools.partial(FixtureHandler, directory=str(root / ".local/fixtures")),
)
threading.Thread(target=server.serve_forever, daemon=True).start()
environment = os.environ.copy()
environment.update(VIDEO_ALLOW_URLS="1", VIDEO_YT_DLP=str(root / ".local/yt-dlp.exe"))
url = f"http://127.0.0.1:{server.server_port}/av.mp4"


def inspect(env, source):
    with open(os.devnull, "wb") as log:
        client = Client(pathlib.Path(args.server).resolve(), env, log)
        try:
            return json.loads(
                client.call("inspect_video_metadata", {"source": source}, timeout=45)[
                    "content"
                ][0]["text"]
            )
        finally:
            client.close()


try:
    assert inspect(environment, url)["streams"]
    try:
        inspect({**environment, "VIDEO_MAX_BYTES": "1024"}, url)
    except RuntimeError as error:
        assert any(
            message in str(error)
            for message in ["size", "exactly one media file", "yt-dlp failed"]
        ), error
    else:
        raise AssertionError("oversized download accepted")
    with tempfile.TemporaryDirectory(dir=root / ".local") as temp:
        env = {**environment, "VIDEO_DOWNLOAD_TIMEOUT": "1", "TMP": temp, "TEMP": temp}
        started = time.monotonic()
        try:
            inspect(env, url.replace("av.mp4", "hang.mp4"))
        except RuntimeError as error:
            assert "execution limit" in str(error), error
        else:
            raise AssertionError("download deadline ignored")
        assert time.monotonic() - started < 10, "deadline/cleanup took too long"
        assert not list(pathlib.Path(temp).iterdir()), "download left temporary files"
    print(
        "PASS: MCP URL download, size rejection, deadline and Windows temporary cleanup"
    )
finally:
    server.shutdown()
    server.server_close()
