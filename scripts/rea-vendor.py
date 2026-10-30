#!/usr/bin/env python3
"""Checks and config for the local REA install in vendor/rea (see vendor/README.md).

  python scripts/rea-vendor.py sync      allow/deny list for the 170 REA tools
  python scripts/rea-vendor.py smoke     Ghidra end-to-end check on the target DLL
  python scripts/rea-vendor.py wiring    what a `claude -p` session would actually see

All three start the REA server exactly as vendor/rea/rea.mcp.json describes (same
command, args and env Claude Code will use), so they test the real configuration.
scripts/install-rea.ps1 writes that file and runs `sync`.

sync   Lists the server's tools and writes vendor/rea/rea-loop-settings.json for
       `claude --settings`: allow = the read tools in ALLOW, deny = every other REA
       tool. A bare deny rule removes a tool from the model's context. Claude Code
       only defers MCP tools on Anthropic's own API, so behind a local-model
       ANTHROPIC_BASE_URL all 170 would load (~68k tokens); these 13 are ~2k.

smoke  Opens vendor/rea/targets/FFXiMain.unpacked.dll with Ghidra, waits out the
       first full analysis (minutes), then checks facts already byte-verified for
       FFXiMain.dll retail-2026-09 (VAs, image base 0x10000000):
         - xrefs to OpenMenu, VA 0x1015E350, include the 8 call sites in the menu
           command handler (VA 0x1014E7E1 ... 0x1014E9EA)
         - "gcMyroomDancerGet() failed" is found at VA 0x10388D04
         - procedure_pseudo_code decompiles OpenMenu without error
       Prints PASS or FAIL and the cold-start time. Exit code 0 on PASS.

wiring Starts `claude -p` with the session flags from vendor/README.md, reads the
       init event (the exact tool list the model gets), then stops it before any
       real work. PASS when the rea server is connected and exactly ALLOW is
       visible. Without a Claude Code CLI on PATH it SKIPs (exit 0): the check
       needs `claude`, nothing else about the install changes.
"""
import argparse
import collections
import hashlib
import json
import os
import pathlib
import queue
import re
import shutil
import struct
import subprocess
import sys
import threading
import time

PIN = "6.4.0"
SERVER_KEY = "rea"
ALLOW = [
    "open_binary", "close_binary", "binary_overview",
    "procedure_pseudo_code", "procedure_assembly",
    "procedure_callers", "procedure_callees",
    "xrefs", "find_xrefs_to_name", "search_strings", "search_procedures",
    "resolve_containing_procedure", "read_bytes",
]
# Left out on purpose (measured on FFXiMain.unpacked.dll): analyze_function returned 3 MB
# for OpenMenu, binary_session 0.5 MB, procedure_references up to 2 MB, and get_call_graph
# walks the whole reachable graph. Each would flood a local model's context.
ROOT = pathlib.Path(__file__).resolve().parents[1]
REA_DIR = ROOT / "vendor" / "rea"
DEFAULT_CONFIG = str(REA_DIR / "rea.mcp.json")
DEFAULT_SETTINGS = str(REA_DIR / "rea-loop-settings.json")
DEFAULT_TARGET = str(REA_DIR / "targets" / "FFXiMain.unpacked.dll")

# Smoke-test facts for FFXiMain.dll retail-2026-09 (TDS 0x6A995428), all VAs.
TDS = 0x6A995428
IMAGE_BASE = 0x10000000
OPEN_MENU = 0x1015E350
OPEN_MENU_CALL_SITES = [0x1014E7E1, 0x1014E87B, 0x1014E8AF, 0x1014E8CB,
                        0x1014E8E7, 0x1014E962, 0x1014E97E, 0x1014E9EA]
MYROOM_STRING = "gcMyroomDancerGet() failed"
MYROOM_VA = 0x10388D04


def log(msg):
    print(msg, flush=True)


class ReaServer:
    """Minimal MCP stdio client (newline-delimited JSON-RPC)."""

    def __init__(self, config_path):
        with open(config_path, encoding="utf-8-sig") as f:
            cfg = json.load(f)
        srv = cfg["mcpServers"][SERVER_KEY]
        env = dict(os.environ)
        env.update({k: str(v) for k, v in (srv.get("env") or {}).items()})
        cmd = [srv["command"]] + list(srv.get("args") or [])
        self.stderr_tail = collections.deque(maxlen=60)
        self.proc = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env, text=True, encoding="utf-8", errors="replace", bufsize=1)
        self.inbox = queue.Queue()
        self.next_id = 0
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_stdout(self):
        for line in self.proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                self.inbox.put(json.loads(line))
            except ValueError:
                self.stderr_tail.append("[stdout] " + line[:300])
        self.inbox.put(None)

    def _read_stderr(self):
        for line in self.proc.stderr:
            self.stderr_tail.append(line.rstrip()[:300])

    def _send(self, obj):
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params, timeout_s, label=None):
        self.next_id += 1
        rid = self.next_id
        params = dict(params)
        if method == "tools/call":
            params["_meta"] = {"progressToken": rid}
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        start = time.time()
        last_note = start
        while True:
            left = timeout_s - (time.time() - start)
            if left <= 0:
                raise TimeoutError("%s: no answer after %d s" % (label or method, timeout_s))
            try:
                msg = self.inbox.get(timeout=min(left, 20))
            except queue.Empty:
                msg = "tick"
            if msg is None:
                raise RuntimeError("REA server exited. Last stderr:\n" + "\n".join(self.stderr_tail))
            if msg != "tick" and msg.get("id") == rid:
                if "error" in msg:
                    raise RuntimeError("%s error: %s" % (label or method, json.dumps(msg["error"])[:2000]))
                return msg["result"]
            if time.time() - last_note >= 60:
                log("  ... %s still running, %d s elapsed" % (label or method, time.time() - start))
                last_note = time.time()

    def initialize(self):
        res = self.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "rea_vendor", "version": "1"}}, 120)
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return res

    def tools(self):
        out, cursor = [], None
        while True:
            res = self.request("tools/list", {"cursor": cursor} if cursor else {}, 120)
            out += res.get("tools", [])
            cursor = res.get("nextCursor")
            if not cursor:
                return out

    def call(self, name, args, timeout_s):
        res = self.request("tools/call", {"name": name, "arguments": args}, timeout_s, label=name)
        text = "\n".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
        blob = text if text.strip() else json.dumps(res.get("structuredContent") or {})
        if res.get("isError"):
            raise RuntimeError("%s returned an error:\n%s" % (name, text[:3000]))
        return blob

    def stop(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=30)
        except Exception:
            self.proc.kill()


def numbers_in(blob):
    vals = {int(h, 16) for h in re.findall(r"0x([0-9a-fA-F]+)", blob)}
    vals |= {int(d) for d in re.findall(r"(?<![0-9a-fA-Fx])([0-9]{9,10})(?![0-9])", blob)}
    return vals


def check_version(init):
    ver = (init.get("serverInfo") or {}).get("version")
    log("REA server version: %s (pinned %s)" % (ver, PIN))
    if ver != PIN:
        raise SystemExit("FAIL: installed REA is %s, the kit is pinned to %s" % (ver, PIN))


def cmd_sync(a):
    srv = ReaServer(a.config)
    try:
        check_version(srv.initialize())
        names = sorted(t["name"] for t in srv.tools())
    finally:
        srv.stop()
    missing = [n for n in ALLOW if n not in names]
    if missing:
        raise SystemExit("FAIL: server lacks allowed tools: %s" % missing)
    prefix = "mcp__%s__" % SERVER_KEY
    settings = {"permissions": {
        "allow": [prefix + n for n in ALLOW],
        "deny": [prefix + n for n in names if n not in ALLOW]}}
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(settings, f, indent=2)
        f.write("\n")
    log("tools on server: %d | allowed: %d | denied: %d" % (
        len(names), len(settings["permissions"]["allow"]), len(settings["permissions"]["deny"])))
    log("wrote %s" % a.out)
    log("PASS")


def pe_identity(path):
    with open(path, "rb") as f:
        data = f.read()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    machine = struct.unpack_from("<H", data, pe + 4)[0]
    tds = struct.unpack_from("<I", data, pe + 8)[0]
    base = struct.unpack_from("<I", data, pe + 24 + 28)[0]
    return machine, tds, base, len(data), hashlib.sha256(data).hexdigest()


def cmd_smoke(a):
    target = os.path.abspath(a.target)
    machine, tds, base, size, sha = pe_identity(target)
    log("target %s\n  machine %#x  TimeDateStamp %#x  ImageBase %#x  size %d\n  sha256 %s"
        % (target, machine, tds, base, size, sha))
    if machine != 0x14C or tds != TDS or base != IMAGE_BASE:
        raise SystemExit("FAIL: not the retail-2026-09 unpacked DLL (need x86, TDS %#x, base %#x)" % (TDS, IMAGE_BASE))
    results = []
    srv = ReaServer(a.config)
    try:
        check_version(srv.initialize())
        srv.call("open_binary", {"path": target, "provider_id": "ghidra"}, 300)
        log("open_binary ok. First Ghidra query now: import + full auto-analysis of the DLL.")
        t0 = time.time()
        overview = srv.call("binary_overview", {}, a.deadline_min * 60)
        cold = time.time() - t0
        log("binary_overview ok after %.0f s (cold start). Overview size %d chars." % (cold, len(overview)))

        t1 = time.time()
        xr = numbers_in(srv.call("xrefs", {"address": "0x%X" % OPEN_MENU}, 900))
        miss = [hex(x) for x in OPEN_MENU_CALL_SITES if x not in xr]
        results.append(("xrefs to OpenMenu 0x1015E350 include the 8 handler call sites", not miss, miss))
        log("xrefs answered in %.1f s" % (time.time() - t1))

        st = numbers_in(srv.call("search_strings", {"pattern": MYROOM_STRING, "mode": "literal",
                                                    "case_sensitive": True}, 900))
        results.append(("string '%s' at VA 0x10388D04" % MYROOM_STRING, MYROOM_VA in st, []))

        fn = srv.call("procedure_pseudo_code", {"procedure": "0x%X" % OPEN_MENU}, 900)
        results.append(("procedure_pseudo_code decompiles OpenMenu (%d chars)" % len(fn), len(fn) > 200, []))
        if a.dump:
            with open(a.dump, "w", encoding="utf-8") as f:
                f.write(fn)
        try:
            srv.call("close_binary", {}, 300)
        except Exception as e:
            log("close_binary: %s" % e)
    finally:
        srv.stop()
    ok = all(r[1] for r in results)
    for name, good, extra in results:
        log("  [%s] %s%s" % ("ok" if good else "XX", name, ("  missing: %s" % extra) if extra else ""))
    log("cold start: %.0f s" % cold)
    log("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


def cmd_wiring(a):
    claude = shutil.which(a.claude)
    if claude is None:
        # Found on GEOALIEN 2026-10-14: without Claude Code installed, Popen died with a raw
        # FileNotFoundError traceback. The wiring check cannot run here — skip cleanly; the REA
        # install itself (sync/smoke) does not need `claude`.
        log("SKIP: '%s' CLI not found on PATH - wiring checks require Claude Code. "
            "Install it and re-run; sync and smoke still validate this install." % a.claude)
        sys.exit(0)
    cmd = [claude, "-p", "Reply with the single word OK.", "--mcp-config", a.config,
           "--settings", a.settings, "--permission-mode", "dontAsk",
           "--output-format", "stream-json", "--verbose", "--max-turns", "1"]
    log("running: " + " ".join(cmd))
    p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         text=True, encoding="utf-8", errors="replace")
    watchdog = threading.Timer(240, p.kill)
    watchdog.start()
    init = None
    try:
        for line in p.stdout:
            try:
                obj = json.loads(line)
            except ValueError:
                continue
            if obj.get("type") == "system" and obj.get("subtype") == "init":
                init = obj
                break
    finally:
        watchdog.cancel()
        p.kill()
    if not init:
        err = p.stderr.read()[-2000:] if p.stderr else ""
        raise SystemExit("FAIL: no init event from claude. stderr tail:\n" + err)
    tools = init.get("tools", [])
    rea = sorted(x for x in tools if x.startswith("mcp__%s__" % SERVER_KEY))
    servers = {s.get("name"): s.get("status") for s in init.get("mcp_servers", [])}
    want = sorted("mcp__%s__%s" % (SERVER_KEY, n) for n in ALLOW)
    log("rea server status: %s" % servers.get(SERVER_KEY))
    log("tools the model gets: %d total, %d from REA" % (len(tools), len(rea)))
    extra = [x for x in rea if x not in want]
    missing = [x for x in want if x not in rea]
    if extra:
        log("  REA tools that should have been removed: %s" % extra)
    if missing:
        log("  allowed REA tools that are missing: %s" % missing)
    ok = servers.get(SERVER_KEY) == "connected" and not extra and not missing
    log("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sync")
    s.add_argument("--config", default=DEFAULT_CONFIG)
    s.add_argument("--out", default=DEFAULT_SETTINGS)
    m = sub.add_parser("smoke")
    m.add_argument("--config", default=DEFAULT_CONFIG)
    m.add_argument("--target", default=DEFAULT_TARGET)
    m.add_argument("--deadline-min", type=int, default=70)
    m.add_argument("--dump", default="", help="write the OpenMenu pseudocode result to this file")
    w = sub.add_parser("wiring")
    w.add_argument("--config", default=DEFAULT_CONFIG)
    w.add_argument("--settings", default=DEFAULT_SETTINGS)
    w.add_argument("--claude", default="claude")
    a = ap.parse_args()
    {"sync": cmd_sync, "smoke": cmd_smoke, "wiring": cmd_wiring}[a.cmd](a)


if __name__ == "__main__":
    main()
