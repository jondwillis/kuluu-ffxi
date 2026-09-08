---
name: beads
description: Use when working in a repository that uses bd or Beads for durable project task tracking, issue dependencies, blocker management, multi-session handoff, or shared work memory. Trigger when the user asks to find ready work, claim or close tasks, create follow-up work, inspect blockers, recover project context, or choose between local planning and persistent project tracking.
---

# Beads

Use Beads as the shared project task system. Local plans, scratch files, and personal memories are useful, but they are not the durable source of truth for project work.

## Kuluu Policy

Read this repository's `AGENTS.md` before using the generic workflow below.
Its Beads policy intentionally differs from the stock `bd prime` context:

- Start routine work with `bd ready` or `bd show <id>`; the shared session
  hook already supplies the concise project context. Do not inject raw
  `bd prime` output automatically.
- Beads is the durable tracker, but `MEMORY.md` remains the cross-session
  memory system and in-session todos remain ephemeral. Do not migrate either
  into `bd remember`.
- The repository profile grants normal commit authority, but push and Dolt
  sync still require explicit confirmation.

If the hook context is absent, verify the workspace with:

```bash
bd where
```

## Preferred Route

Use the `bd` CLI when shell access is available. It is the most compact and direct Beads interface.

## Core CLI Workflow

1. Find work:

```bash
bd ready
bd list --status=open
bd list --status=in_progress
```

2. Inspect before editing:

```bash
bd show <id>
```

3. Claim work atomically:

```bash
bd update <id> --claim
```

4. Create durable follow-up work when implementation reveals new tasks:

```bash
bd create "Short title" --description="Why this exists and what needs to be done" --type=task --priority=2
```

5. Close completed work:

```bash
bd close <id> --reason="Completed"
```

## What Belongs In Beads

Use Beads for:

- shared project tasks
- blockers and dependencies
- discovered follow-up work
- work that must survive thread reset, compaction, or handoff
- status that another person or agent should be able to resume

Use agent-local planning tools only for the current turn's execution checklist. Do not treat them as shared project state.

## Rules

- Do not create markdown TODO files as the source of truth when Beads is available.
- Do not use `bd edit`; it opens an interactive editor. Use `bd update` flags instead.
- Prefer `--json` when parsing `bd` output programmatically.
- Follow the repository's `AGENTS.md` when its workflow differs from the generic Beads defaults.
- Do not auto-close or mutate tasks unless the work is actually complete.
