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
    for event in ("SessionStart", "UserPromptSubmit"):
        groups = hooks.setdefault(event, [])
        kept = []
        for group in groups:
            handlers = [h for h in group.get("hooks", []) if "ste100-reminder.py" not in h.get("command", "")]
            if handlers:
                kept.append({**group, "hooks": handlers})
        kept.append({"hooks": [{
            "type": "command",
            "command": 'root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0; common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0; for hook in "$root/.agents/hooks/ste100-reminder.py" "$(dirname "$common")/.agents/hooks/ste100-reminder.py"; do if [ -f "$hook" ]; then python3 "$hook"; exit $?; fi; done',
            "timeout": 10,
        }]})
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
            print("Codex project hooks missing or stale; run .agents/hooks/install-codex-project-hooks.py", file=sys.stderr)
            return 1
    else:
        path.write_text(json.dumps(wanted, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
