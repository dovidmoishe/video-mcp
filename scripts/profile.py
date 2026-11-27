"""Profile MCP frame extraction, including server startup and Windows peak RSS."""

import argparse
import base64
import concurrent.futures
import ctypes
import ctypes.wintypes as wt
import json
import os
import pathlib
import time
from mcp_client import Client


class MemoryCounters(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("PageFaultCount", wt.DWORD)] + [
        (name, ctypes.c_size_t)
        for name in [
            "PeakWorkingSetSize",
            "WorkingSetSize",
            "QuotaPeakPagedPoolUsage",
            "QuotaPagedPoolUsage",
            "QuotaPeakNonPagedPoolUsage",
            "QuotaNonPagedPoolUsage",
            "PagefileUsage",
            "PeakPagefileUsage",
        ]
    ]


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("media")
parser.add_argument("--server", default="target/debug/video-mcp.exe")
args = parser.parse_args()
peak = 0
started = time.perf_counter()
with open(os.devnull, "wb") as log:
    client = Client(pathlib.Path(args.server).resolve(), os.environ.copy(), log)
    try:
        if os.name == "nt":
            get_memory = ctypes.WinDLL("psapi").GetProcessMemoryInfo
            get_memory.argtypes = [wt.HANDLE, ctypes.POINTER(MemoryCounters), wt.DWORD]
            get_memory.restype = wt.BOOL
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(
                client.call,
                "extract_keyframes",
                {
                    "source": str(pathlib.Path(args.media).resolve()),
                    "options": {
                        "window": {"start": 0, "end": 3},
                        "count": 8,
                        "max_width": 960,
                        "max_height": 540,
                    },
                },
            )
            while not pending.done():
                if os.name == "nt":
                    memory = MemoryCounters()
                    memory.cb = ctypes.sizeof(memory)
                    if get_memory(
                        int(client.process._handle), ctypes.byref(memory), memory.cb
                    ):
                        peak = max(peak, memory.PeakWorkingSetSize)
                time.sleep(0.01)
            response = pending.result()
        elapsed = time.perf_counter() - started
    finally:
        client.close()
images = [c for c in response["content"] if c["type"] == "image"]
print(
    json.dumps(
        {
            "server": str(pathlib.Path(args.server).resolve()),
            "wall_seconds": elapsed,
            "sampled_peak_working_set_bytes": peak or None,
            "images": len(images),
            "encoded_bytes": sum(
                len(base64.b64decode(c["data"], validate=True)) for c in images
            ),
            "note": "Single startup-inclusive MCP sample; shutdown excluded. Peak sampled during extraction; no child-process or hard memory limit measurement.",
        },
        indent=2,
    )
)
