#!/usr/bin/env python3
"""ClaudeDeck statusline writer.

Configured as a Claude Code `statusLine.command` (scoped to ClaudeDeck-launched
sessions via `claude --settings`). On every render Claude pipes a JSON blob to
our stdin; we extract the context-window fill % and write it to a per-key file
that the OpenDeck plugin polls. We also print a short line to stdout, which
becomes the session's in-terminal status line.

The per-key file is keyed by the CLAUDEDECK_KEY env var that the plugin sets when
it launches the session, so the plugin can map file -> button without knowing the
session id in advance.
"""
import json
import os
import sys
from pathlib import Path


def status_dir() -> Path:
    rt = os.environ.get("XDG_RUNTIME_DIR") or "/tmp/claudedeck"
    return Path(rt) / "claudedeck"


def main() -> None:
    try:
        data = json.load(sys.stdin)
    except Exception:
        data = {}

    cw = data.get("context_window") or {}
    pct = cw.get("used_percentage")
    model_obj = data.get("model") or {}
    model = model_obj.get("display_name", "")
    model_id = model_obj.get("id", "")

    # In-terminal status line (stdout).
    line = f"{pct}% ctx" if pct is not None else "… ctx"
    if model:
        line += f" · {model}"
    sys.stdout.write(line)

    # Per-key file for the OpenDeck plugin.
    key = os.environ.get("CLAUDEDECK_KEY")
    if key:
        d = status_dir()
        d.mkdir(parents=True, exist_ok=True)
        (d / f"{key}.json").write_text(json.dumps({"pct": pct, "model": model_id}))


if __name__ == "__main__":
    main()
