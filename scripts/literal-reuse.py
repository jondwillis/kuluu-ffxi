#!/usr/bin/env python3
"""Flag a numeric literal re-typed where a named const already carries it.

Registry: every integer `const NAME: <int type> = <literal>;` in tracked Rust
sources plus the build-time scraped tables under target/*/build/*/out. A line
that spells one of those values out again (test fixture, default, second
const) is a hit; the fix is to import the const. Usage:

  literal-reuse.py             whole tree
  literal-reuse.py --staged    added lines in the staged hunks
  literal-reuse.py --base REV  added lines since REV

Consts declared in tests/, examples/, benches/, a tests.rs module or below
a #[cfg(test)] line are fixtures, not names to import, so they are not
registered.
"""
import argparse
import glob
import os
import re
import subprocess
import sys

EXCLUDED_ROOTS = ("vendor/", "research/", "target/", "ffxi-agent/")
INT_TYPES = r"(?:u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize)"
LITERAL = r"-?(?:0x[0-9A-Fa-f_]+|0b[01_]+|[0-9][0-9_]*)"
CONST_DECL = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:\s*"
    + INT_TYPES + r"\s*=\s*(" + LITERAL + r")(?:" + INT_TYPES + r")?\s*;"
)
TOKEN = re.compile(
    r"(?<![A-Za-z0-9_.\-])(" + LITERAL + r")(?:" + INT_TYPES + r"|f32|f64)?(?![A-Za-z0-9_.\-])"
)
COMMENT = re.compile(r"(?<!:)//.*$")
PINNED = "PINNED"


def parse_int(text):
    text = text.replace("_", "")
    try:
        return int(text, 0)
    except ValueError:
        return None


MIN_MEANINGFUL = 1000


def trivial(v):
    v = abs(v)
    if v < MIN_MEANINGFUL:
        return True
    if v & (v - 1) == 0 or (v + 1) & v == 0:
        return True
    digits = str(v)
    return set(digits[1:]) <= {"0"}


def crate_of(path):
    return path.split("/", 1)[0]


def path_deps():
    """crate -> transitive set of workspace crates it depends on (incl. itself)."""
    direct = {}
    for toml in glob.glob("*/Cargo.toml"):
        crate = toml.replace(os.sep, "/").split("/", 1)[0]
        deps = {crate}
        with open(toml, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = re.search(r'path\s*=\s*"\.\./([A-Za-z0-9_-]+)"', line)
                if m:
                    deps.add(m.group(1))
        direct[crate] = deps
    closure = {}
    for crate in direct:
        seen, stack = set(), [crate]
        while stack:
            c = stack.pop()
            if c in seen:
                continue
            seen.add(c)
            stack.extend(direct.get(c, ()))
        closure[crate] = seen
    return closure


def tracked_rs():
    out = subprocess.run(["git", "ls-files", "*.rs"], capture_output=True, text=True, check=True)
    return [p for p in out.stdout.split("\n") if p and not p.startswith(EXCLUDED_ROOTS)]


def scraped_rs():
    return sorted(glob.glob("target/*/build/*/out/*.rs"))


TEST_PARTS = {"tests", "examples", "benches", "tests.rs"}
CFG_TEST = "#[cfg(test)]"


def is_test_source(path):
    return bool(TEST_PARTS & set(path.split("/")))


def build_registry(paths):
    """Consts from shipped code only: a test module's local fixture value is
    not a name the rest of the tree is expected to import."""
    registry = {}
    for path in paths:
        norm = path.replace(os.sep, "/")
        if is_test_source(norm):
            continue
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                for line in f:
                    if line.strip().startswith(CFG_TEST):
                        break
                    m = CONST_DECL.match(line)
                    if not m:
                        continue
                    value = parse_int(m.group(2))
                    if value is None or trivial(value) or PINNED in m.group(1):
                        continue
                    if "/out/" in norm:
                        crate = norm.split("/out/")[0].split("/")[-1].rsplit("-", 1)[0]
                        label = crate
                    else:
                        crate, label = crate_of(norm), norm
                    registry.setdefault(value, set()).add((crate, f"{m.group(1)} ({label})"))
        except OSError:
            continue
    return registry


def candidate_lines(args):
    if args.staged or args.base:
        cmd = ["git", "diff", "-U0", "--no-color"]
        cmd += ["--cached"] if args.staged else [args.base]
        cmd += ["--", "*.rs"]
        out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
        path, lineno = None, 0
        for raw in out.split("\n"):
            if raw.startswith("+++ "):
                path = raw[4:].removeprefix("b/")
            elif raw.startswith("@@"):
                m = re.search(r"\+(\d+)", raw)
                lineno = int(m.group(1)) if m else 0
            elif raw.startswith("+") and not raw.startswith("+++"):
                if path and not path.startswith(EXCLUDED_ROOTS):
                    yield path, lineno, raw[1:]
                lineno += 1
            elif not raw.startswith("-"):
                lineno += 1
        return
    for path in tracked_rs():
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                for n, line in enumerate(f, 1):
                    yield path, n, line.rstrip("\n")
        except OSError:
            continue


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--staged", action="store_true")
    ap.add_argument("--base")
    args = ap.parse_args()

    registry = build_registry(tracked_rs() + scraped_rs())
    deps = path_deps()
    hits = []
    for path, lineno, line in candidate_lines(args):
        decl = CONST_DECL.match(line)
        if decl and PINNED in decl.group(1):
            continue
        reachable = deps.get(crate_of(path), {crate_of(path)})
        code = COMMENT.sub("", line)
        for m in TOKEN.finditer(code):
            value = parse_int(m.group(1))
            if value is None or trivial(value) or value not in registry:
                continue
            owners = sorted(
                name
                for crate, name in registry[value]
                if crate in reachable and not (decl and name.startswith(decl.group(1) + " "))
            )
            if not owners:
                continue
            hits.append(f"  {path}:{lineno}: {m.group(1)} re-types {', '.join(owners)}")
    if hits:
        print("checks: literals - a value that already has a named const is spelled out again; import the const (tests included):", file=sys.stderr)
        print("\n".join(sorted(set(hits))), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
