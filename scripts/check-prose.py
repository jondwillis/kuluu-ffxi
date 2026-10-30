#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import os
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
VALE_VERSION = "3.24.0"


def main():
    parser = argparse.ArgumentParser(description="Check prose with the repository Vale rules.")
    parser.add_argument("--procedures", action="store_true", help="Use the 20-word instruction limit.")
    parser.add_argument("files", nargs="*", help="Markdown or text files; read stdin when omitted.")
    args = parser.parse_args()
    vale = shutil.which(os.environ.get("VALE", "vale"))
    if not vale:
        parser.error(f"Install Vale {VALE_VERSION}, or set VALE to its executable path.")
    version = subprocess.check_output([vale, "--version"], text=True).strip()
    if version != f"vale version {VALE_VERSION}":
        parser.error(f"Expected Vale {VALE_VERSION}; found {version}.")
    config = ROOT / (".vale/procedures.ini" if args.procedures else ".vale.ini")
    command = [vale, "--no-global", "--output=JSON", f"--config={config}"]
    if args.files:
        command.extend(str(Path(path).resolve()) for path in args.files)
    else:
        command.append("--ext=.md")
    result = subprocess.run(command, text=True, capture_output=True)
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="")
    try:
        reports = json.loads(result.stdout)
    except ValueError:
        print(result.stdout, file=sys.stderr, end="")
        return result.returncode or 2
    count = 0
    for path, alerts in reports.items():
        for alert in alerts:
            count += 1
            print(f"{path}:{alert['Line']}: {alert['Check']}: {alert['Message']}")
    return result.returncode or int(count > 0)


if __name__ == "__main__":
    sys.exit(main())
