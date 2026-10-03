#!/usr/bin/env python3
import json
from pathlib import Path
import shlex
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def shell_cwd(payload):
    inputs = payload.get("tool_input", {})
    cwd = Path(inputs.get("workdir") or payload.get("cwd") or Path.cwd())
    boundary = "\n;&|<>"
    tokens = shlex.shlex(inputs.get("command", ""), posix=True, punctuation_chars=boundary)
    tokens.whitespace = " \t\r"
    tokens.whitespace_split = True
    command = tokens.get_token()
    directory = None
    if command == "cd":
        directory = tokens.get_token()
    elif command == "git":
        while argument := tokens.get_token():
            if all(char in boundary for char in argument):
                break
            if argument == "-C":
                directory = tokens.get_token()
                break
    if directory and not all(char in boundary for char in directory):
        candidate = Path(directory)
        cwd = candidate if candidate.is_absolute() else cwd / candidate
    return str(cwd.resolve())


def main():
    event = sys.argv[1]
    payload = json.load(sys.stdin)
    if event == "PostToolUse" and isinstance(payload.get("tool_response"), dict) and payload["tool_response"].get("isError"):
        return 0
    tool = payload.get("tool_name", "")
    script = None
    if event == "Stop":
        script = HERE / "stop.d/25-verify.sh"
    elif event == "PostToolUse" and tool == "view_image":
        result = subprocess.run([sys.executable, str(HERE / "runtime-verification.py"), "inspect"],
                                input=json.dumps(payload), text=True, capture_output=True)
        if result.returncode:
            print(json.dumps({"decision": "block", "reason": result.stderr.strip()}))
        return 0
    elif tool in ("Bash", "exec_command"):
        payload["cwd"] = shell_cwd(payload)
        script = HERE / ("session-edits-bash-pre.sh" if event == "PreToolUse" else "session-edits-bash-post.sh")
    elif event == "PostToolUse" and tool in ("apply_patch", "Edit", "Write", "NotebookEdit"):
        script = HERE / "session-edits-record.sh"
    if not script:
        return 0
    result = subprocess.run(["bash", str(script)], input=json.dumps(payload), text=True, capture_output=True)
    if result.returncode:
        reason = result.stdout.strip() or result.stderr.strip() or "Runtime verification hook failed. Repair the hook before completing changes."
        print(json.dumps({"decision": "block", "reason": reason}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError) as error:
        print(json.dumps({"decision": "block", "reason": f"Runtime verification hook failed: {error}"}))
