# Render & combat-feedback gaps (observation note)

Source: the second "Ffxi features" voice note (2026-06-05, ~6 min), recorded the
morning after the 31-minute menu walkthrough that became
`vanilla-menu-parity-plan.md`. Where that note is about *missing UI*, this one is a
**bug list** captured while watching another player (and self) from a starter zone,
referenced against a vanilla HorizonXI client. Transcribed and reconciled against
current code.

## Observed gaps

### A. Player-character models render heads only
Other PCs show only a head — equipped gear (maple wand, hatchling shield, rabbit
cap on the observed character) is not rendered on the body. Matches the standing
[[character-model-rendering-gap]] memory (PCs render static/half-body while NPCs
animate). Not specific to the observed character — the gear is confirmed equipped.

### B. Entities snap to "distance ~500" and vanish, then reappear
When the observed player **casts a spell / engages**, their model disappears and the
target panel distance jumps to ~50 then ~500. The entity comes back after a
movement/position update. Affects **both mobs and other PCs**. Reads as a position
field being clobbered (reset to origin / a sentinel) by the action/cast update path
rather than carrying the last-known position. `/target <name>` still resolves the
name while the model is gone, confirming the entity still exists — only its
position is wrong.

### C. Dead mobs never despawn
Three tiny Mandragoras killed nearby keep their name + model on the floor. The
vanilla client fades dead entities out after a delay; we never remove them, so
corpses accumulate. Lifecycle gap — see the despawn/cleanup symmetry expected by the
`bevy-lifecycle-symmetry` skill.

### D. No cast / combat animations for observed entities
- Black Mage casting magic: no cast animation on the caster.
- PC↔mob melee: no swing/hit animation — and per (B) both models tend to vanish
  during the fight anyway.
This is the same missing-feedback family the user hit with self-cast **Boost**
(no animation on a successful ability).

### E. Battle-message text doesn't substitute parameters
- Spell start-cast renders as e.g. **"`<name>` starts casting spell number
  1-800-1609571 on `<target>`"** — the **`<spell>` name is not substituted**; a raw
  numeric param is shown instead. Same substitution-gap family as the recast
  "`Time left: (h:mm:ss)`" literal (msg id 202).
- Status application renders **"`<target>` is poison"** instead of **"is
  poisoned"** — looks like the wrong string is pulled (the effect *name* rather than
  the *"is afflicted"* message), and a lower/upper-case mismatch suggests a
  different lookup is needed for the resolved-effect line.

Relevant code: `ffxi-client/src/session.rs` —
`substitute_system_placeholders` (~1975), `substitute_battle_placeholders` (~2424),
`build_battle2_line` (~2263), `template_for_id` (~2303). The `<spell>` / `<ability>`
substitution is not resolving for the start-cast category; the recast time token is
not formatted as `h:mm:ss`.

## Relationship to other docs
- UI that's simply *absent* (contextual menu, trade, item detail, /check) →
  `vanilla-menu-parity-plan.md`.
- Self-cast targeting + ability feedback + recast formatting → tracked in the
  abilities plan (see `docs/ROADMAP.md` ability lines).
- Roadmap parity glyphs for animation/despawn/message-substitution should reflect
  these as `[~]`/`[ ]` where not already tracked.
