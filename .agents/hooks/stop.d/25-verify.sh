#!/usr/bin/env bash
set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
exec python3 "$here/../runtime-verification.py" check
