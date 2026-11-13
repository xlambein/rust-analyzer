#!/usr/bin/env python3
"""Reproduce #18997 through LSP using the user's lsp_bench.Client.

Set PYTHONPATH=/path/to/lsp-bench/src. No third-party Python packages are used.
"""
import argparse
import asyncio
from collections import defaultdict, deque
import hashlib
import json
from pathlib import Path
import platform
import shutil
import statistics
import sys
import tempfile
import time

from lsp_bench import Client


class BenchClient(Client):
    """Drain stderr, answer server requests, and read complete LSP frames."""

    async def __aenter__(self):
        self.stderr_tail = deque(maxlen=100)
        self.failure = None
        await super().__aenter__()
        self.stderr_task = asyncio.create_task(self.drain_stderr())
        return self

    async def drain_stderr(self):
        while line := await self.process.stderr.readline():
            self.stderr_tail.append(line.decode(errors="replace").rstrip())

    async def _recv(self):
        headers = {}
        while line := await self.process.stdout.readline():
            if line in (b"\r\n", b"\n"):
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        else:
            return None
        return json.loads(await self.process.stdout.readexactly(int(headers["content-length"])))

    async def _recv_loop(self):
        try:
            while (msg := await self._recv()) is not None:
                if "method" in msg and "id" in msg:
                    # This fixture needs no client-side configuration or edits.
                    if msg["method"] not in {
                        "window/workDoneProgress/create", "workspace/diagnostic/refresh",
                        "client/registerCapability", "client/unregisterCapability",
                    }:
                        raise RuntimeError(f"Unexpected server request: {msg['method']}")
                    await self._send({"id": msg["id"], "result": None})
                elif "id" in msg:
                    if cell := self.requests.pop(msg["id"], None):
                        cell.set(msg)
                elif msg.get("method") == "experimental/serverStatus":
                    self.quiescent = msg["params"].get("quiescent", False)
                    if self.quiescent:
                        self.waiting_for_quiescence.set()
                    else:
                        self.waiting_for_quiescence.clear()
            raise RuntimeError("Language server closed stdout")
        except Exception as exc:
            self.failure = str(exc)
            for cell in self.requests.values():
                cell.set({"error": {"message": self.failure}})
            self.requests.clear()
            self.waiting_for_quiescence.set()

    async def checked(self, method, params=None):
        if self.failure:
            raise RuntimeError(self.failure)
        msg = await asyncio.wait_for(self.request(method, params), timeout=120)
        if "error" in msg:
            raise RuntimeError(f"{method}: {msg['error']}\n" + "\n".join(self.stderr_tail))
        return msg["result"]

    async def __aexit__(self, *args):
        try:
            await super().__aexit__(*args)
        finally:
            self.recv_task.cancel()
            self.stderr_task.cancel()
            await asyncio.gather(self.recv_task, self.stderr_task, return_exceptions=True)


def fixture(root, functions, errors):
    source = root / "lib.rs"
    model = root / "model.rs"
    text = "#![no_core]\nmod model;\nuse model::Record;\n"
    for i in range(functions):
        fields = "value: value" if i < errors else "value: value, flag: true"
        text += f"fn sample_{i}(value: i32) -> Record {{ Record {{ {fields} }} }}\n"
    source.write_text(text)
    model.write_text("pub struct Record { pub value: i32, pub flag: bool }\n")
    (root / "rust-project.json").write_text(json.dumps({"crates": [{
        "root_module": str(source), "edition": "2024", "deps": [], "cfg": [],
        "env": {}, "is_workspace_member": True, "display_name": "diagnostics_bench",
    }]}))
    return source, model, text


async def benchmark(args):
    binary = Path(shutil.which(args.rust_analyzer) or args.rust_analyzer).resolve()
    measurements = defaultdict(list)
    fingerprints = {}
    with tempfile.TemporaryDirectory(prefix="ra-diagnostics-") as temp:
        root = Path(temp)
        source, model, original = fixture(root, args.functions, args.errors)

        def record(name, value):
            # Temporary workspace paths differ between fresh server processes.
            normalized = json.dumps(value, sort_keys=True).replace(str(root), "/WORKSPACE")
            fingerprints[name] = hashlib.sha256(normalized.encode()).hexdigest()

        async with BenchClient(str(binary)) as client:
            await asyncio.wait_for(client.initialize(
                [str(root)],
                capabilities={
                    "window": {"workDoneProgress": True},
                    "experimental": {"serverStatusNotification": True},
                    "textDocument": {
                        "diagnostic": {"dynamicRegistration": False},
                        "codeAction": {
                            "dataSupport": True,
                            "codeActionLiteralSupport": {
                                "codeActionKind": {"valueSet": ["quickfix"]}},
                            "resolveSupport": {"properties": ["edit"]},
                        },
                    },
                },
                initialization_options={
                    "checkOnSave": False, "cachePriming": {"enable": False},
                    "procMacro": {"enable": False},
                    "diagnostics": {
                        "experimental": {"enable": args.experimental},
                        "disabled": ["redundant_field_names"],
                        "styleLints": {"enable": False},
                    },
                },
            ), timeout=30)
            await asyncio.wait_for(client.wait_for_quiescence(), timeout=120)
            versions = {source: 1, model: 1}
            for path in (source, model):
                await client.notify("textDocument/didOpen", {"textDocument": {
                    "uri": path.as_uri(), "languageId": "rust", "version": 1,
                    "text": path.read_text(),
                }})

            async def change(path, text):
                versions[path] += 1
                await client.notify("textDocument/didChange", {
                    "textDocument": {"uri": path.as_uri(), "version": versions[path]},
                    "contentChanges": [{"text": text}],
                })

            async def timed(name, method, params):
                start = time.perf_counter_ns()
                result = await client.checked(method, params)
                if name:
                    measurements[name].append((time.perf_counter_ns() - start) / 1e6)
                return result

            async def diagnostics(name=None, field="flag", expected=None):
                report = await timed(name, "textDocument/diagnostic", {
                    "textDocument": {"uri": source.as_uri()},
                })
                assert report["kind"] == "full", report
                items = report["items"]
                expected = args.errors if expected is None else expected
                assert len(items) == expected, (len(items), expected, items[:3])
                assert all(d["code"] == "E0063" and d["source"] == "rust-analyzer"
                           and field in d["message"] for d in items), items[:3]
                return items

            async def actions(diags, name=None, resolve_name=None, field="flag"):
                # Request at the first record constructor, inside the diagnostic's range.
                at = {"start": {"line": 3, "character": original.splitlines()[3].index("Record { value")},
                      "end": {"line": 3, "character": original.splitlines()[3].index("Record { value")}}
                result = await timed(name, "textDocument/codeAction", {
                    "textDocument": {"uri": source.as_uri()}, "range": at,
                    "context": {"diagnostics": diags[:1], "only": ["quickfix"]},
                })
                fixes = [a for a in result or [] if a["title"] == "Fill struct fields"]
                assert len(fixes) == (1 if diags else 0), result
                if fixes:
                    resolved = await timed(resolve_name, "codeAction/resolve", fixes[0])
                    assert field in json.dumps(resolved.get("edit")), resolved
                    return resolved

            # Warm the analysis and validate the fixture before starting the timer.
            diags = await diagnostics()
            record("initial_diagnostics", diags)
            record("initial_fix", await actions(diags))
            for _ in range(args.iterations):
                diags = await diagnostics("unchanged_diagnostics")
                await actions(diags, "unchanged_code_action", "unchanged_resolve")

            # Each edit bumps the database revision, keeping diagnostics and target position.
            for i in range(args.iterations):
                await change(source, original + f"// revision {i}\n")
                diags = await diagnostics("edited_diagnostics")
                await actions(diags, "edited_code_action", "edited_resolve")

            # Cross-file invalidation: same target text and offsets, different required field.
            await change(model, "pub struct Record { pub value: i32, pub flag: bool, pub ready: bool }\n")
            diags = await diagnostics(field="ready", expected=args.functions)
            record("dependency_diagnostics", diags)
            record("dependency_fix", await actions(diags, field="ready"))

            # A real edit must remove both diagnostics and their quick fixes.
            clean = original.replace("Record { value: value }", "Record { value: value, flag: true }")
            clean = clean.replace("flag: true }", "flag: true, ready: true }")
            await change(source, clean)
            diags = await diagnostics(expected=0)
            record("clean_diagnostics", diags)
            record("clean_fix", await actions(diags))
            # Return to the original input to check that fixes reappear.
            await change(model, "pub struct Record { pub value: i32, pub flag: bool }\n")
            await change(source, original)
            diags = await diagnostics()
            record("restored_diagnostics", diags)
            record("restored_fix", await actions(diags))

    version = await asyncio.create_subprocess_exec(str(binary), "--version", stdout=asyncio.subprocess.PIPE)
    version_out, _ = await version.communicate()
    output = {
        "binary": str(binary), "binary_sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
        "version": version_out.decode().strip(), "platform": platform.platform(),
        "functions": args.functions, "errors": args.errors, "iterations": args.iterations, "experimental": args.experimental,
        "unit": "ms", "correctness_checks": "passed", "response_fingerprints": fingerprints, "samples": dict(measurements),
        "summary": {k: {"median": statistics.median(v), "min": min(v), "max": max(v)}
                    for k, v in measurements.items()},
    }
    print(json.dumps(output, indent=2))
    if args.output:
        Path(args.output).write_text(json.dumps(output, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust-analyzer", required=True)
    parser.add_argument("--functions", type=int, default=300)
    parser.add_argument("--errors", type=int, default=1)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--experimental", action="store_true")
    parser.add_argument("--output")
    args = parser.parse_args()
    if not 1 <= args.errors <= args.functions or args.iterations < 1:
        parser.error("require 1 <= errors <= functions and iterations >= 1")
    asyncio.run(benchmark(args))


if __name__ == "__main__":
    main()
