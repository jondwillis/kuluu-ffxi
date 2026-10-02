#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import sys

COMMAND = 'python3 "$(git rev-parse --show-toplevel)/.agents/hooks/codex-project-hook.py"'
EVENTS = {
    "PreToolUse": "Bash",
    "PostToolUse": "Bash|Edit|Write|NotebookEdit|view_image",
    "Stop": None,
}


def configured(data):
    hooks = data.setdefault("hooks", {})
    for event, matcher in EVENTS.items():
        groups = hooks.get(event, [])
        kept = []
        for group in groups:
            handlers = [h for h in group.get("hooks", []) if "codex-project-hook.py" not in h.get("command", "")]
            if handlers:
                kept.append({**group, "hooks": handlers})
        group = {"hooks": [{"type": "command", "command": f"{COMMAND} {event}", "timeout": 30}]}
        if matcher:
            group["matcher"] = matcher
        kept.append(group)
        hooks[event] = kept
    return data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    path = args.root / ".codex/hooks.json"
    current = json.loads(path.read_text())
    wanted = configured(json.loads(json.dumps(current)))
    if args.check:
        if current != wanted:
            print("Codex runtime verification hooks missing or stale; run .agents/hooks/install-codex-project-hooks.py", file=sys.stderr)
            return 1
    else:
        path.write_text(json.dumps(wanted, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
