#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

SCHEMA_VERSION = 2
FIRE = 10
SURFACES = ("visual", "audio", "session")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def root_for(path):
    path = Path(path).resolve()
    while not path.is_dir() and path != path.parent:
        path = path.parent
    result = subprocess.run(
        ["git", "-C", str(path), "rev-parse", "--show-toplevel"],
        capture_output=True, text=True, check=True,
    )
    return Path(result.stdout.strip()).resolve()


def surface(path):
    if path.suffix not in (".rs", ".wgsl"):
        return None
    if any(part in ("tests", "test", "vendor", "research", "target", ".git") for part in path.parts):
        return None
    name = path.as_posix()
    if name.startswith(("ffxi-audio/", "kuluu-render/src/audio")):
        return "audio"
    if path.suffix == ".wgsl" or name.startswith((
        "kuluu-render/", "kuluu/src/view_native/", "kuluu-viewer-wasm/", "ffxi-actor/",
    )):
        return "visual"
    return "session"


def ledger_path(session):
    key = hashlib.sha256(session.encode()).hexdigest()
    return Path(os.environ.get("TMPDIR", tempfile.gettempdir())) / "kuluu-runtime-verification" / (key + ".jsonl")


def inspection_path(path):
    key = hashlib.sha256(str(path.resolve()).encode()).hexdigest()
    return ledger_path("inspection:" + key)


def inspect(payload):
    inputs = payload.get("tool_input", {})
    name = inputs.get("path") or inputs.get("file_path")
    if not name:
        return
    path = Path(name)
    if not path.is_absolute():
        path = Path(payload.get("cwd", Path.cwd())) / path
    path = path.resolve()
    if path.is_file() and media_kind(path) == "visual":
        target = inspection_path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps({"path": str(path), "hash": digest(path), "inspected_at_ns": time.time_ns()}))


def track(session, cwd, files):
    root = root_for(cwd)
    records = []
    for name in files:
        path = Path(name)
        path = (Path(cwd) / path if not path.is_absolute() else path).resolve()
        owner = root_for(path.parent) if path.parent.is_dir() else root
        relative = path.relative_to(owner)
        if surface(relative):
            records.append({
                "root": str(owner), "path": str(relative), "hash": digest(path),
                "edited_at_ns": path.stat().st_mtime_ns if path.is_file() else time.time_ns(),
            })
    if records:
        ledger = ledger_path(session)
        ledger.parent.mkdir(parents=True, exist_ok=True)
        with ledger.open("a") as output:
            for record in records:
                output.write(json.dumps(record) + "\n")


def sources(root):
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        capture_output=True, check=True,
    )
    paths = {Path(os.fsdecode(name)) for name in result.stdout.split(b"\0") if name}
    return {str(path): digest(root / path) for path in sorted(paths) if surface(path)}


def media_kind(path):
    with path.open("rb") as stream:
        header = stream.read(32)
    if (header.startswith((b"\x89PNG\r\n\x1a\n", b"\xff\xd8\xff", b"GIF87a", b"GIF89a"))
            or (header.startswith(b"RIFF") and header[8:12] == b"WEBP")
            or header[4:8] == b"ftyp" or header.startswith(b"\x1aE\xdf\xa3")):
        return "visual"
    if (header.startswith((b"OggS", b"fLaC", b"ID3"))
            or (header.startswith(b"RIFF") and header[8:12] == b"WAVE")):
        return "audio"
    return "session"


def record(args):
    root = root_for(Path.cwd())
    if not args.summary.strip():
        raise ValueError("summary must describe an observation or concrete blocker")
    if args.verdict in ("pass", "blocked") and not args.artifact:
        raise ValueError("pass and blocked require captured artifacts")
    if args.verdict == "pass" and not args.surface:
        raise ValueError("pass requires --surface visual, audio, or session")
    if args.verdict == "pass" and any(s in args.surface for s in ("visual", "audio")) and not args.inspection:
        raise ValueError("visual/audio pass requires --inspection describing what you inspected")
    if args.verdict == "pass" and "visual" in args.surface and not args.build:
        raise ValueError("visual pass requires --build identifying the changed client executable")
    if args.verdict == "waived" and not args.authorization:
        raise ValueError("waived requires --authorization quoting the user's explicit opt-out")
    artifacts = []
    for name in args.artifact:
        path = Path(name).resolve()
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"artifact missing or empty: {name}")
        artifacts.append({
            "path": str(path), "hash": digest(path), "kind": media_kind(path),
            "captured_at_ns": path.stat().st_mtime_ns,
        })
        inspected = inspection_path(path)
        if inspected.is_file():
            observation = json.loads(inspected.read_text())
            if observation.get("hash") == artifacts[-1]["hash"]:
                artifacts[-1]["inspected_at_ns"] = observation["inspected_at_ns"]
    for required in ("visual", "audio"):
        if args.verdict == "pass" and required in args.surface and not any(a["kind"] == required for a in artifacts):
            raise ValueError(f"{required} pass requires captured {required} media; logs and tests do not qualify")
    if args.verdict == "pass" and "visual" in args.surface and not any(
        a["kind"] == "visual" and a.get("inspected_at_ns", 0) >= a["captured_at_ns"] for a in artifacts
    ):
        raise ValueError("open the captured image with view_image (Codex) or Read (Claude) before recording visual pass")
    build = None
    if args.build:
        executable = Path(args.build).resolve()
        if not executable.is_file() or not executable.stat().st_size:
            raise ValueError("build executable missing or empty")
        build = {"path": str(executable), "hash": digest(executable), "built_at_ns": executable.stat().st_mtime_ns}
    marker = {
        "schema_version": SCHEMA_VERSION, "verdict": args.verdict,
        "summary": args.summary, "surfaces": args.surface,
        "inspection": args.inspection, "user_authorization": args.authorization,
        "verified_at_ns": time.time_ns(), "root": str(root),
        "source_hashes": sources(root), "artifacts": artifacts, "build": build,
    }
    target = root / ".verify/latest.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_suffix(".tmp")
    temporary.write_text(json.dumps(marker, indent=2) + "\n")
    temporary.replace(target)
    print(f"recorded {args.verdict} -> {target}")
    if args.verdict == "blocked":
        print(f"Handoff: Runtime verification remains blocked and incomplete. {args.summary}")


def disclosure_text(message):
    message = re.sub(r"(?m)^\s{0,3}>\s?", "", message)
    message = re.sub(r"`+", "", message)
    while True:
        plain = re.sub(r"(?<!\w)(\*\*|__|\*|_)(?=\S)(.+?)(?<=\S)\1(?!\w)",
                       lambda match: match[2], message, flags=re.DOTALL)
        if plain == message:
            return " ".join(plain.split())
        message = plain


def last_message(payload):
    if payload.get("last_assistant_message"):
        return payload["last_assistant_message"]
    transcript = payload.get("transcript_path")
    if not transcript or not Path(transcript).is_file():
        return ""
    message = ""
    for line in Path(transcript).read_text().splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if entry.get("type") == "assistant":
            content = entry.get("message", {}).get("content", [])
            if isinstance(content, list):
                text = "\n".join(c.get("text", "") for c in content if isinstance(c, dict))
                if text:
                    message = text
    return message


def evidence_error(root, edits, message):
    marker_path = root / ".verify/latest.json"
    if not marker_path.is_file():
        return "no runtime evidence marker"
    try:
        marker = json.loads(marker_path.read_text())
        if marker.get("schema_version") != SCHEMA_VERSION or marker.get("root") != str(root):
            return "evidence is from an old schema or another worktree"
        latest = max(e["edited_at_ns"] for e in edits)
        if marker.get("verified_at_ns", 0) < latest:
            return "evidence predates the last edit"
        for edit in edits:
            current = digest(root / edit["path"])
            if marker["source_hashes"].get(edit["path"]) != current:
                return f"evidence does not cover current source: {edit['path']}"
        verdict = marker.get("verdict")
        if verdict == "waived":
            quote = marker.get("user_authorization", "")
            return None if quote and quote in message else "waiver needs explicit user authorization disclosed in the handoff"
        artifacts = marker.get("artifacts", [])
        if not artifacts:
            return "no captured artifacts"
        for artifact in artifacts:
            path = Path(artifact["path"])
            if not path.is_file() or not path.stat().st_size or digest(path) != artifact["hash"]:
                return f"artifact missing or changed: {path}"
        if verdict == "blocked":
            if not any(a["captured_at_ns"] >= latest for a in artifacts):
                return "no fresh blocker diagnostic after the last edit; capture the actual failure and record blocked again"
            summary = marker.get("summary", "")
            reason = disclosure_text(summary)
            report = disclosure_text(message)
            if reason and reason in report and re.search(r"\bblocked\b", report, flags=re.IGNORECASE):
                return None
            return ("blocked verification must be disclosed as incomplete with the recorded reason.\n"
                    f"Evidence marker: {marker_path}\n"
                    "Recover in the final handoff, keeping the bead open:\n"
                    f"Runtime verification remains blocked and incomplete. {summary}\n"
                    "Inline code, emphasis, blockquotes and wrapped whitespace are accepted; preserve the full reason. "
                    "A disclosure failure needs a corrected report, not another drive or a refreshed marker.")
        if verdict != "pass":
            return f"verification verdict is {verdict!r}, not pass"
        required = {surface(Path(e["path"])) for e in edits}
        if not required.issubset(set(marker.get("surfaces", []))):
            return "evidence uses the wrong runtime surface"
        if "visual" in required:
            build = marker.get("build")
            if not build or digest(Path(build["path"])) != build["hash"]:
                return "visual evidence does not identify the inspected client build"
            rust_edit = max((e["edited_at_ns"] for e in edits if Path(e["path"]).suffix == ".rs"), default=0)
            if build["built_at_ns"] < rust_edit:
                return "client executable predates the Rust edit; rebuild before capturing"
        for kind in required & {"visual", "audio"}:
            if not marker.get("inspection"):
                return f"{kind} evidence was not inspected"
            captured_after = max(latest, marker["build"]["built_at_ns"]) if kind == "visual" else latest
            matching = [a for a in artifacts if media_kind(Path(a["path"])) == kind and a["captured_at_ns"] >= captured_after]
            if not matching:
                return f"no fresh {kind} capture after the last edit; logs and unit tests do not qualify"
            if kind == "visual" and not any(a.get("inspected_at_ns", 0) >= a["captured_at_ns"] for a in matching):
                return "fresh visual capture was not opened for image inspection"
        return None
    except (ValueError, KeyError, TypeError, OSError):
        return "malformed or unreadable evidence marker"


def check(payload):
    session = payload.get("session_id")
    if not session:
        return "Verification gate: hook payload has no session_id. Fix hook registration before completing runtime changes."
    ledger = ledger_path(session)
    if not ledger.is_file():
        return None
    latest = {}
    for line in ledger.read_text().splitlines():
        entry = json.loads(line)
        latest[(entry["root"], entry["path"])] = entry
    groups = {}
    for (root, _), entry in latest.items():
        groups.setdefault(root, []).append(entry)
    errors = []
    message = last_message(payload)
    for root, edits in groups.items():
        error = evidence_error(Path(root), edits, message)
        if error:
            errors.append(f"{root}: {error}\n" + "\n".join(e["path"] for e in edits))
    if errors:
        return ("Verification gate: runtime changes remain unverified, including committed edits.\n\n"
                + "\n\n".join(errors)
                + "\n\nUse the verify skill. Visual changes require a fresh screenshot/video of the changed build and pixel inspection; audio requires listening. "
                "Record pass with --surface visual|audio|session --inspection '<observed result>' --artifact <capture>. "
                "If a real blocker prevents the drive, capture its diagnostic, record --verdict blocked, keep the bead open, and report the exact reason as blocked. "
                "Tests, slow builds, and elapsed effort do not replace runtime evidence.")
    return None


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="mode", required=True)
    tracking = commands.add_parser("track")
    tracking.add_argument("--session", required=True)
    tracking.add_argument("--cwd", required=True)
    tracking.add_argument("files", nargs="+")
    recording = commands.add_parser("record")
    recording.add_argument("--verdict", choices=("pass", "fail", "blocked", "waived"), required=True)
    recording.add_argument("--summary", required=True)
    recording.add_argument("--surface", action="append", choices=SURFACES, default=[])
    recording.add_argument("--inspection", default="")
    recording.add_argument("--authorization", default="")
    recording.add_argument("--artifact", action="append", default=[])
    recording.add_argument("--build")
    commands.add_parser("inspect")
    commands.add_parser("check")
    args = parser.parse_args()
    try:
        if args.mode == "track":
            track(args.session, args.cwd, args.files)
        elif args.mode == "record":
            record(args)
        elif args.mode == "inspect":
            inspect(json.load(sys.stdin))
        else:
            reason = check(json.load(sys.stdin))
            if reason:
                print(reason)
                return FIRE
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Verification gate: {error}", file=sys.stderr)
        return FIRE if args.mode == "check" else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
