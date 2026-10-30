#!/usr/bin/env python3
import json
from pathlib import Path
import sys


def main():
    payload = json.load(sys.stdin)
    event = payload.get("hook_event_name")
    if event not in ("SessionStart", "UserPromptSubmit"):
        return
    instructions = (Path(__file__).resolve().parents[2] / "AGENTS.md").read_text()
    heading = "## Output language: ASD-STE100"
    section = instructions.split(heading, 1)[1].split("\n## ", 1)[0].strip()
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": event,
        "additionalContext": heading + "\n\n" + section,
    }}))


if __name__ == "__main__":
    main()
