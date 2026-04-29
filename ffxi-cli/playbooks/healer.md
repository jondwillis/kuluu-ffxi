# Playbook: co-play healer

Two-character (or harness-plus-human) party run with the agent
playing a support role: stick to the leader, react to `/tell`-driven
heal cues, watch for incoming aggro. Target latency from `tell`
arrival to `cast` dispatch: ≤ 2 s p50.

## Pre-run checklist

| Field | Example | Source |
|---|---|---|
| Leader's `target_id` | 17,895,683 (any in-zone player UniqueNo) | Read from `party://members` after the leader invites the agent |
| Leader's character name | "Vanari" | User-supplied |
| Healer spell ids | 1 (Cure I), 2 (Cure II), 19 (Banish), … | User-supplied; FFXI Spells.dat indices |
| Self `target_id` and `act_index` | both fields populate after zone-in | `diagnostics://session` and `party://members` |

Healing requires party membership (the server only exposes party
HP/MP via `0x0DD GROUP_LIST` and `0x0DF GROUP_ATTR` to actual
party members). The actual `/invite` exchange happens through
the FFXI UI on the leader's side; the agent accepts via the
supplied "yes" event-end interaction. **There is no v1 tool
for accepting a party invite** — flag this; the user accepts
manually in their own client today.

## Setup

Once in-party:

```
follow { target_id: <leader_id>, distance: 3.0 }
```

3.0 yalms holds you in casting range of the leader without
clipping into mob hitboxes.

## Heal cues

Subscribe to `scene://current`, `party://members`, and the
high-signal events.

### Cue: `tell` from leader containing `@cure` or `@heal`

```
on_event(TellReceived { from, text }):
  if from == <leader_name> and text.matches("@cure" | "@heal"):
    cast { spell_id: 1, target_id: <leader_id>, target_index: <leader_act_index> }
```

The latency budget from `TellReceived` arrival to `cast` dispatch
landing in `cmd_tx` is ≤ 2 s p50, ≤ 5 s p99. The reactor will
pump the `0x01A` action to the server within one 200 ms tick.

### Cue: `party_member_low_hp` notification

The reactor fires `PartyMemberLowHp { id, pct }` whenever a member
crosses below 25 % HP. Treat this as wake-up only; re-read
`party://members` and pick the target manually:

```
on_event(PartyMemberLowHp { id, pct }):
  read party://members
  cast { spell_id: 1, target_id: id, target_index: <member.act_index> }
```

Do **not** auto-cast on every notification — re-check current HP
in the resource read; the notification is edge-triggered and the
member may have already been cured by the time the LLM responds.

### Cue: `engaged_by` on self

Aggro on the healer is a kite-or-tank decision. Default behavior:
trust the tank. If the tank's HP is healthy:

```
on_event(EngagedBy { entity_id }):
  cancel
  path_to { x, y, z }   # behind the tank, picked from scene://current
  follow { target_id: <leader_id>, distance: 3.0 }
```

If the tank is at < 50 % HP (read `party://members`), kite first:

```
  path_to { x, y, z }   # away from the mob, ~25 yalms behind tank
  cast { spell_id: 1, target_id: <self_id>, target_index: <self_act_index> }
```

## Spells you might want surfaced

These are common enough that the user's setup will provide their ids:

| Spell | Why |
|---|---|
| Cure I/II/III | Single-target heal |
| Banish I | Damage; undead-only |
| Dia I | Slow accuracy debuff |
| Stoneskin | Pre-buff before a pull |
| Protect | Group buff |

The agent should **only** cast spells the user has provided ids
for. Don't guess: an invalid `spell_id` is silently rejected by
the server with no observable error.

## Failure modes

| Symptom | Likely cause | What to do |
|---|---|---|
| Cast goes through but no HP recovery on the target | Wrong `target_id` (server rejects mismatched id↔index) | Re-read `party://members`, refresh `act_index`. |
| `EngagedBy` followed by sustained damage with no kite | Aggro stuck on the agent past the kite distance | Increase `path_to` distance; healers without enmity reduction usually need 35+ yalms to drop aggro. |
| Cast latency > 2 s consistently | Reactor backed up | Check `diagnostics://session::sync_in/out` lag; if reactor is healthy, the LLM's own response time is the bottleneck. |
| Spell interrupted by mob hits | Healer in melee range | Hold further with `follow { distance: 18.0 }` (casting range). |

## Pass criteria for the test run

The forward plan's Stage-12 acceptance for the healer run:

1. Two clients in-party (or one + a human leader).
2. Round-trip from leader's `/tell @cure` to the agent's `cast`
   landing in cmd_tx ≤ 2 s p50.
3. At least one `PartyMemberLowHp` → `cast` cycle observed.
4. The `aggro` HUD overlay (Stage V3) lights up when a mob
   targets the healer; the kite path produced by `path_to`
   actually breaks the aggro inside two ticks of stepping.
