#!/usr/bin/env python3
"""Explicit-download native embedding smoke test; never touches real user state.

Run after cargo build: python3 Scripts/test-memory-local.py --download
Not part of ordinary offline CI: downloads ~91 MB into a temporary directory.
"""

import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--download", action="store_true", required=True)
    parser.add_argument("--binary", default="target/debug/yeet")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    with tempfile.TemporaryDirectory(prefix="yeet-memory-smoke-") as temporary:
        root = Path(temporary)
        workspace = root / "workspace"
        workspace.mkdir()
        environment = dict(os.environ, YEET_CONFIG_DIR=str(root / "state"))

        def call(*arguments, text=None, offline=False, success=True):
            env = dict(environment)
            if offline:
                env.update(
                    HTTP_PROXY="http://127.0.0.1:1",
                    HTTPS_PROXY="http://127.0.0.1:1",
                    ALL_PROXY="http://127.0.0.1:1",
                    YEET_RUNTIME_DIR=str(root / "no-provider-runtime"),
                    PATH=str(root / "no-external-executables"),
                )
            start = time.monotonic()
            result = subprocess.run(
                [binary, "memory", *arguments], cwd=workspace, env=env,
                input=text, text=True, capture_output=True, timeout=300,
            )
            assert (result.returncode == 0) == success, (arguments, result.stderr)
            print(f"{' '.join(arguments[:3])}: {time.monotonic() - start:.3f}s PASS", flush=True)
            return json.loads(result.stdout) if success else None

        model, profile = "minilm-l6-v2", "smoke-local"
        assert call("local", "status", model)["state"] == "not-installed"
        installed = call("local", "install", model, "--download")
        assert installed["state"] == "artifacts-verified"
        validated = call("local", "validate", model, profile, offline=True)
        assert validated["state"] == "ready"
        assert validated["profile"]["dimensions"] == 384
        assert validated["relatedSimilarity"] > validated["unrelatedSimilarity"] + 0.2
        print("fixture cosine:", validated["relatedSimilarity"], validated["unrelatedSimilarity"], flush=True)

        saved = call("save", "-", "--local-only", text="Our database stores durable conversation history in SQLite.")
        call("save", "-", text="A dog plays with a puppy in the park.")
        long_text = "A puppy plays in the park. " * 90 + "The deployment uses stable source spans."
        long_record = call("save", "-", "--local-only", text=long_text)
        indexed = call("local", "index", model, profile, offline=True)
        assert indexed["indexed"] == 3 and indexed["blocked"] == []
        with sqlite3.connect(root / "state/memory/local-only.sqlite3") as database:
            count, end = database.execute("SELECT count(*),max(end) FROM vectors WHERE id=?", [long_record["id"]]).fetchone()
            assert count > 1 and end == len(long_text.encode())
        hits = call("local", "recall", model, profile, "persistent database storage", offline=True)
        assert hits[0]["record"]["id"] == saved["id"]
        assert hits[0]["record"]["privacy"] == "local-only"
        call("local", "recall", model, profile, "word " * 400, offline=True, success=False)
        call("local", "remove", model, "--confirm")
        assert call("local", "status", model)["state"] == "not-installed"
        assert call("get", saved["id"])["text"].startswith("Our database")
    print("Native download, validation, offline leaf indexing/recall and removal PASS")


if __name__ == "__main__":
    main()
