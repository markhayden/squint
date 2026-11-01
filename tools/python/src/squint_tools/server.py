"""A stdio MCP server, revision 2025-11-25, speaking newline-delimited JSON-RPC 2.0.

The squint harness is the only client: it initialises, lists, calls with ``_squint`` injected,
resets between runs with ``squint/reset``, and cancels with ``notifications/cancelled``.
"""

from __future__ import annotations

import json
import sys
from typing import Any, TextIO

from . import __version__, models, segmenter, tools
from .context import Context, ContextError
from .envelope import invalid_args, split_images
from .segmenter import SegmenterUnavailable

PROTOCOL_VERSION = "2025-11-25"


def mcp_tool(manifest: dict[str, Any]) -> dict[str, Any]:
    """A squint manifest as an MCP tool: native name, description and schemas; the rest annotated."""
    annotation = {
        k: v for k, v in manifest.items() if k not in ("name", "input_schema", "output_schema")
    }
    return {
        "name": manifest["name"],
        "description": manifest["purpose"],
        "inputSchema": manifest["input_schema"],
        "outputSchema": manifest["output_schema"],
        "annotations": {"squint": annotation},
    }


def select(names: list[str] | None) -> list[dict[str, Any]]:
    """The manifests of the named tools in the pack's order, every tool when none are named."""
    if not names:
        return list(tools.MANIFESTS)
    known = [m["name"] for m in tools.MANIFESTS]
    unknown = [n for n in names if n not in known]
    if unknown:
        raise ValueError(f"unknown tool {unknown[0]!r}; this pack serves {', '.join(known)}")
    return [m for m in tools.MANIFESTS if m["name"] in names]


class Server:
    def __init__(self, out: TextIO, served: list[dict[str, Any]] | None = None) -> None:
        self.out = out
        self.served = list(tools.MANIFESTS) if served is None else served
        self.calls_since_reset = 0

    def respond(self, message: dict[str, Any]) -> None:
        self.out.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.out.flush()

    def result(self, request_id: Any, result: Any) -> None:
        self.respond({"jsonrpc": "2.0", "id": request_id, "result": result})

    def error(self, request_id: Any, code: int, message: str) -> None:
        self.respond(
            {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}
        )

    def handle(self, message: dict[str, Any]) -> None:
        method = message.get("method", "")
        request_id = message.get("id")
        params = message.get("params") or {}
        if method == "initialize":
            problem = self.segmenter_problem()
            if problem is not None:
                # The harness ends the run at resolve with these words, before any spend.
                self.error(request_id, -32603, problem)
                return
            self.result(
                request_id,
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": False}},
                    "serverInfo": {"name": "squint-tools", "version": __version__},
                },
            )
        elif method in ("notifications/initialized", "notifications/cancelled"):
            return
        elif method == "tools/list":
            self.result(request_id, {"tools": [mcp_tool(m) for m in self.served]})
        elif method == "tools/call":
            self.call(request_id, params)
        elif method == "squint/reset":
            self.calls_since_reset = 0
            self.result(request_id, {})
        elif request_id is not None:
            self.error(request_id, -32601, "Method not found")

    def segmenter_problem(self) -> str | None:
        """Why a served tool that needs the segmenter could not run, or None.

        The artifact is checked (present, at the recorded size) and loaded once here, so the
        first `regions` call does not pay the load inside its own timeout; a server serving no
        such tool neither checks nor loads it.
        """
        needing = [m["name"] for m in self.served if m["name"] in tools.NEEDS_SEGMENTER]
        if not needing:
            return None
        problem = models.artifact_problem(models.Record.load())
        if problem is None:
            try:
                segmenter.shared()
            except (RuntimeError, OSError, ValueError) as e:
                # A file of the right size that onnxruntime cannot load (SegmenterUnavailable is
                # a RuntimeError too).
                problem = f"{type(e).__name__}: {e}"
        if problem is None:
            return None
        return f"{' and '.join(needing)} need the segmenter artifact: {problem}"

    def call(self, request_id: Any, params: dict[str, Any]) -> None:
        self.calls_since_reset += 1
        name = params.get("name", "")
        arguments = params.get("arguments") or {}
        handler = tools.HANDLERS.get(name) if any(m["name"] == name for m in self.served) else None
        if handler is None:
            self.error(request_id, -32602, f"unknown tool {name}")
            return
        if "_squint" not in arguments:
            envelope = invalid_args(name, "_squint is required")
        else:
            try:
                context = Context.parse(arguments["_squint"])
            except ContextError as e:
                # The harness's call is malformed: a JSON-RPC error, which it records as a
                # tool error, never an answer the model could act on.
                self.error(request_id, -32602, str(e))
                return
            model_args = {k: v for k, v in arguments.items() if k != "_squint"}
            try:
                envelope = handler(model_args, context)
            except (SegmenterUnavailable, OSError, ValueError) as e:
                # The artifact `fetch` places is not there, or a handle's pixels cannot be read
                # (a corrupt file, a decompression bomb, bad base64): the harness records the
                # call as a tool error with these words, and the server goes on serving.
                self.error(request_id, -32603, f"{name}: {type(e).__name__}: {e}")
                return
        # The images' bytes travel as content blocks, in the envelope's order (SPEC "Results").
        structured, blocks = split_images(envelope)
        self.result(
            request_id,
            {
                "content": blocks,
                "structuredContent": structured,
                "isError": envelope["status"] == "invalid_args",
            },
        )


def serve(
    stdin: TextIO = sys.stdin,
    stdout: TextIO = sys.stdout,
    served: list[dict[str, Any]] | None = None,
) -> None:
    server = Server(stdout, served)
    for line in stdin:
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            print("squint-tools: malformed line", file=sys.stderr)
            continue
        if isinstance(message, dict):
            server.handle(message)
