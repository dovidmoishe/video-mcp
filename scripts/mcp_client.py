"""Small stdio client shared by local integration and performance checks."""

import json
import queue
import subprocess
import threading
import time


class Client:
    def __init__(self, server, environment, stderr, startup_timeout=330):
        self.process = subprocess.Popen(
            [str(server)],
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
        )
        self.responses = queue.Queue()
        self.identifier = 0
        threading.Thread(target=self._read, daemon=True).start()
        try:
            self.request(
                "initialize",
                {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "media-benchmark", "version": "1"},
                },
                timeout=startup_timeout,
            )
            self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        except BaseException:
            self.close()
            raise

    def _read(self):
        for line in self.process.stdout:
            self.responses.put(line)
        self.responses.put(None)

    def send(self, message):
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def request(self, method, params, timeout=330):
        self.identifier += 1
        self.send(
            {
                "jsonrpc": "2.0",
                "id": self.identifier,
                "method": method,
                "params": params,
            }
        )
        deadline = time.monotonic() + timeout
        while True:
            try:
                line = self.responses.get(timeout=max(0, deadline - time.monotonic()))
            except queue.Empty:
                self.send(
                    {
                        "jsonrpc": "2.0",
                        "method": "notifications/cancelled",
                        "params": {
                            "requestId": self.identifier,
                            "reason": "controller deadline",
                        },
                    }
                )
                raise TimeoutError(
                    f"MCP {method} exceeded its request budget"
                ) from None
            if line is None:
                raise RuntimeError("MCP server exited")
            response = json.loads(line)
            if response.get("id") == self.identifier:
                if "error" in response:
                    raise RuntimeError(response["error"])
                result = response["result"]
                if result.get("isError"):
                    raise RuntimeError(result)
                return result

    def call(self, name, arguments, timeout=330):
        return self.request(
            "tools/call", {"name": name, "arguments": arguments}, timeout=timeout
        )

    def close(self):
        try:
            self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=10)
