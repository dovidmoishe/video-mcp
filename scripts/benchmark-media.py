"""No agent API calls: MCP startup, repeated operations and retained viewing."""

import argparse
import base64
import json
import os
import pathlib
import statistics
import time
from mcp_client import Client

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("media")
parser.add_argument("--server", default="target/debug/video-mcp.exe")
parser.add_argument("--output", default=".local/media-benchmark.json")
parser.add_argument("--watch", action="store_true")
parser.add_argument("--skip-speech", action="store_true")
parser.add_argument("--repeats", type=int, default=5)
args = parser.parse_args()
if not 1 <= args.repeats <= 20:
    parser.error("repeats must be 1..20")
source = str(pathlib.Path(args.media).resolve())
results = {
    "source": source,
    "server": str(pathlib.Path(args.server).resolve()),
    "rows": [],
}


def record(arm, operation, repeat, started, response):
    images = [c for c in response.get("content", []) if c["type"] == "image"]
    row = {
        "arm": arm,
        "operation": operation,
        "repeat": repeat,
        "seconds": time.perf_counter() - started,
        "images": len(images),
        "encoded_bytes": sum(
            len(base64.b64decode(c["data"], validate=True)) for c in images
        ),
    }
    results["rows"].append(row)


with open(os.devnull, "wb") as log:
    started = time.perf_counter()
    client = Client(pathlib.Path(args.server).resolve(), os.environ.copy(), log)
    results["mcp_startup_seconds"] = time.perf_counter() - started
    try:
        metadata = json.loads(
            client.call("inspect_video_metadata", {"source": source})["content"][0][
                "text"
            ]
        )
        end = min(15, metadata.get("duration_seconds") or 15)
        has_audio = any(s.get("kind") == "audio" for s in metadata["streams"])
        for repeat in range(args.repeats):
            operations = [
                ("inspect_video_metadata", {"source": source}),
                (
                    "extract_keyframes",
                    {
                        "source": source,
                        "options": {
                            "window": {"start": 0, "end": end},
                            "count": 8,
                            "max_width": 960,
                            "max_height": 540,
                        },
                    },
                ),
            ]
            if has_audio and not args.skip_speech:
                operations.insert(
                    1,
                    (
                        "get_timestamped_transcript",
                        {
                            "source": source,
                            "window": {"start": 0, "end": end},
                            "language": "en",
                        },
                    ),
                )
            for tool, arguments in operations:
                started = time.perf_counter()
                record("mcp", tool, repeat, started, client.call(tool, arguments))
            if args.watch:
                for arm in (
                    ["stateless", "watch"]
                    if repeat % 2 == 0
                    else ["watch", "stateless"]
                ):
                    started = time.perf_counter()
                    size = 0
                    handle = None
                    try:
                        for start in range(0, int(end + 0.999), 5):
                            stop = min(start + 5, end)
                            if stop - start < 0.25:
                                continue
                            if arm == "stateless":
                                response = client.call(
                                    "extract_keyframes",
                                    {
                                        "source": source,
                                        "options": {
                                            "window": {"start": start, "end": stop},
                                            "count": 5,
                                            "max_width": 320,
                                            "max_height": 180,
                                        },
                                    },
                                )
                            else:
                                arguments = {
                                    "start": start,
                                    "seconds": stop - start,
                                    "fps": 1,
                                    "width": 320,
                                    "include_speech": False,
                                }
                                arguments.update(
                                    {"video_id": handle}
                                    if handle
                                    else {"source": source}
                                )
                                response = client.call("watch_video", arguments)
                                handle = json.loads(response["content"][0]["text"])[
                                    "video_id"
                                ]
                            size += sum(
                                len(base64.b64decode(c["data"], validate=True))
                                for c in response["content"]
                                if c["type"] == "image"
                            )
                    finally:
                        if handle:
                            client.call("close_video", {"video_id": handle})
                    record(arm, "sequential_windows", repeat, started, {})
                    results["rows"][-1]["encoded_bytes"] = size
    finally:
        client.close()
pathlib.Path(args.output).write_text(json.dumps(results, indent=2), encoding="utf-8")
for arm, operation in sorted({(r["arm"], r["operation"]) for r in results["rows"]}):
    values = [
        r["seconds"]
        for r in results["rows"]
        if r["arm"] == arm and r["operation"] == operation
    ]
    print(
        json.dumps(
            {
                "arm": arm,
                "operation": operation,
                "median_seconds": statistics.median(values),
                "samples": len(values),
            }
        )
    )
