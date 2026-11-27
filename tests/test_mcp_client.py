"""Contracts for the stdio client used only by developer checks."""

import json
import queue
import unittest
from unittest.mock import Mock, patch
from scripts.mcp_client import Client


class ClientTests(unittest.TestCase):
    def test_timeout_cancels_and_late_reply_cannot_satisfy_next_request(self):
        client = Client.__new__(Client)
        client.responses = queue.Queue()
        client.identifier = 0
        sent = []
        client.send = sent.append
        with self.assertRaises(TimeoutError):
            client.request("tools/call", {}, timeout=0)
        self.assertEqual(sent[-1]["method"], "notifications/cancelled")
        self.assertEqual(sent[-1]["params"]["requestId"], 1)
        client.responses.put(json.dumps({"id": 1, "result": {"late": True}}).encode())
        client.responses.put(
            json.dumps({"id": 2, "result": {"current": True}}).encode()
        )
        self.assertEqual(client.request("tools/list", {}), {"current": True})

    def test_failed_initialization_closes_the_owned_process(self):
        process = Mock()
        process.stdout = []
        process.wait.return_value = 0
        with patch("scripts.mcp_client.subprocess.Popen", return_value=process):
            with self.assertRaisesRegex(RuntimeError, "server exited"):
                Client("test-server", {}, None, startup_timeout=1)
        process.stdin.close.assert_called_once()
        process.wait.assert_called_once()


if __name__ == "__main__":
    unittest.main()
