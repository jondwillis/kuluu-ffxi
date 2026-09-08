# Harness configuration (`.agents/` canonical; `.claude/` and `.codex/` = adapters)

Agent-facing content lives in **`.agents/`**, harness-neutral, mirroring the
`CLAUDE.md → AGENTS.md` symlink pattern. Harness-specific registration stays
in its native adapter directory.

**The rule: how a harness *discovers* a thing decides how it gets wired.**

| Kind | Discovery | Wiring | Example |
| --- | --- | --- | --- |
| Skills, subagents | Harness scans a well-known directory | Real content in `.agents/`, Claude follows a symlink; Codex reads it directly | `.claude/skills → ../.agents/skills`, `.claude/agents → ../.agents/agents` |
| Claude hooks | Claude takes an explicit path from a config file | Real scripts in `.agents/hooks/`, path pointer in `.claude/settings.json` — **no directory to symlink, so `.claude/hooks/` does not exist** | `${CLAUDE_PROJECT_DIR}/.agents/hooks/…` |
| Codex hooks | Codex takes an explicit path from its native hook config | Real scripts in `.agents/hooks/`, path pointer in `.codex/hooks.json` | `.agents/hooks/beads-prime-start.sh` |
| Root instruction file | Harness reads a fixed filename | `AGENTS.md` is canonical; the Claude-specific name is a symlink | `CLAUDE.md → AGENTS.md` |
| Per-user / runtime state | Harness writes it | Real file in the owning adapter, gitignored, never mirrored | `.claude/settings.local.json`, `.claude/.bandwidth/` |

Git tracks exactly three things under `.claude/`: the two symlinks and
`settings.json`. The tracked `.codex/` files enable native hooks and point
back to `.agents/`; they contain no project guidance.

`scripts/checks.sh harness` enforces the table — broken symlink, symlink
escaping `.agents/`, missing/non-executable hook path, a reappeared
`.claude/hooks/`, a malformed Codex hook registration, or a doc citing a
`.claude/` path that isn't tracked all fail the stage. It runs first in
`.githooks/pre-push` and as its own CI step.

## Layout

- `.agents/skills/` — [agentskills.io](https://agentskills.io/specification)
  standard layout. Codex discovers it directly; Claude Code follows the
  `.claude/skills` symlink.
- `.agents/agents/` — subagent definitions (Markdown + frontmatter),
  symlinked from `.claude/agents`.
- `.agents/hooks/` — standalone shell scripts speaking the shared JSON hook
  protocol: a payload on stdin (`session_id`, `transcript_path`, `tool_name`,
  `stop_hook_active`, …), decisions via exit code / stdout JSON. The scripts
  are the portable asset; registration is the adapter. Each `stop.d/` check
  is independently testable:
  `echo "$payload" | .agents/hooks/stop.d/20-commit.sh; echo $?`
  (exit 0 = pass, exit 10 = fire with the reason on stdout).

## `ffxi-agent/` is a separate, intentional exception

`ffxi-agent/` ships its own `.claude/{hooks,agents,skills,settings.json}`
as *real* directories, plus an `opencode.json` that reads a `.claude/`
path. That is not drift: it is the runtime playbook for an LLM agent
*playing the game*, shipped as a unit, with a different audience from this
repo's dev harness. It is out of scope for the table above, and
`scripts/checks.sh harness` skips it.
