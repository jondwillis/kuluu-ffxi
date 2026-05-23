---
name: lotus-lookups-scope-narrowing
description: FFXI look resolver scope narrowing to documented PC races (1–8) via lotus-ffxi lookup tables is legitimate, not reward-hacking
metadata:
  type: feedback
---

## Pattern: Lotus-FFXi-Backed Behavior Narrowing

When `ffxi-viewer-core/src/look_resolver.rs` is updated:

**Rule:** Scope narrowing from "extrapolates for any race" to "race 1–8 only" is legitimate and load-bearing when:
1. Driven by transcription from `vendor/lotus-ffxi/ffxi/entity/actor_data.cppm:32-96` (PCModelIDs lookup table)
2. Paired with explicit bounds check (`race > 8 → None`)
3. Accompanied by new tests for all supported races (1–8) showing correct values from the lotus table
4. Rationale explains delegation: out-of-range (monstrosity/beastman) NPCs go through `npc_dat_id()` path instead

**Why:** The old closed-form formula silently misrouted non-Hume races (off by ~3000 file_ids), causing wrong/missing geometry. Lotus-ffxi is the authoritative reverse-engineer; scope narrowing fixes a real bug.

**How to apply:** When reviewing diffs to `resolve_equipment_slot()` or similar:
- Look for `race > 8 || race == 0` bounds checks
- Verify new tests cover races 1–8 with values from lotus table
- Check that test comments explain why high-race codes now reject (delegation to NPC path)
- Confirm function docstring updated to say "Documented PC range is 1..=8"

This pattern was seen in commit with look_resolver.rs rewrite on 2026-05-18. Do not flag as assertion weakening if all the above are present.
