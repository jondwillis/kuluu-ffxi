# FFXI agent harness — agent playbook (OpenCode / pi.dev / generic MCP)

This file mirrors `CLAUDE.md` for harnesses other than Claude Code. The
behavior contract is identical — only the launch invocation differs.

## Quick start

The MCP server config lives in `.mcp.json` at the repo root. Both OpenCode
and pi.dev support stdio MCP servers; point them at this file.

**OpenCode:**
```bash
opencode --mcp .mcp.json
```

**pi.dev:**
Add to your pi.dev project config:
```json
{
  "mcpServers": [
    { "command": "cargo run -p ffxi-mcp", "transport": "stdio" }
  ]
}
```
*(Confirm pi.dev's exact MCP transport invocation — stdio support has not
been verified end-to-end yet.)*

## Tool / resource catalog

See `CLAUDE.md` § "Tools" and "Resources" — semantics are identical. The
behavioral contract is "issue high-level goals, don't drive per-tick
motion".

## Required env

```
FFXI_USER, FFXI_PASS, FFXI_CHAR_ID, FFXI_CHAR
FFXI_SERVER (optional, defaults to 127.0.0.1)
FFXI_AUTH_PORT, FFXI_DATA_PORT, FFXI_VIEW_PORT (defaults match Phoenix)
FFXI_MAP_HOST_OVERRIDE (optional — for docker host networking workarounds)
FFXI_MCP_GOAL_PATH (optional — defaults to ~/.config/ffxi-mcp/goal.json)
```

## Smoke-test checklist

1. `cargo build -p ffxi-mcp` succeeds.
2. Harness reports the `ffxi` server connected and lists 8 tools, 4 resources.
3. Reading `scene://current` returns "Session not started" or a stage prose.
4. Calling `snapshot` triggers a `SceneSummary` event and a fresh diagnostics
   dump.
5. With a real Phoenix container running (`phoenix dev compose up`), the
   stage transitions Idle → Authenticating → InZone within ~5 s.

## Differences from CLAUDE.md

- pi.dev's MCP transport choice (stdio vs SSE/HTTP) **needs confirmation**;
  the plan currently assumes stdio. If pi.dev requires SSE, we'll add an
  alternate transport flag to `ffxi-mcp` (rmcp's `transport-streamable-http-server`
  feature) — that's purely additive.
- OpenCode's slash-command surface for MCP tools may render tool names
  differently; the underlying contracts are unchanged.
