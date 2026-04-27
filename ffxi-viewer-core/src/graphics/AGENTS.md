# AGENTS.md — graphics settings (scoped)

`CLAUDE.md` is a symlink to this file. This note is loaded whenever you edit
anything under `ffxi-viewer-core/src/graphics/`.

This module owns the **user-facing graphics settings model**: `GraphicsSettings`
(`settings.rs`) plus the `GraphicsField` enum, the `GRAPHICS_FIELDS` array, and
the `apply_*_system` functions that push each setting onto the live render
pipeline.

## The rule: every graphics option is user-facing

A new graphics knob is **not done** until it appears in the in-app graphics menu.
That menu is the launcher (pre-login) screen in
`ffxi-client/src/view_native/launcher_ui/graphics.rs`; it does **not** hardcode a
list — it iterates `GRAPHICS_FIELDS` and renders one ◄ value ► row per entry via
`spawn_field_row`. So the single act that surfaces an option in the GUI is adding
its `GraphicsField` to `GRAPHICS_FIELDS`. Don't add a setting that's only
reachable from a slash command or an env var.

## Checklist — adding a graphics option (do every step)

1. **Field** on `GraphicsSettings` (`settings.rs`). Add `#[serde(default = "…")]`
   (or `#[serde(default)]`) so old `graphics.json` files still load — persistence
   is automatic via `Serialize`/`Deserialize` (`ffxi-client/src/graphics_store.rs`).
2. **Enum** variant on `GraphicsField` + arm in `GraphicsField::label()`.
   Indent the label with two leading spaces only if it's an "advanced" child knob
   (see `is_advanced`).
3. **Display** arm in `GraphicsSettings::value_label()`.
4. **Cycle** arm in `GraphicsSettings::cycle()`, backed by a `const *_SLOTS` array
   (use `cycle_slot*`). If the knob is a quality lever, set
   `self.preset = QualityPreset::Custom`; if it's orthogonal to the tier (e.g.
   sky style, zone lines), leave the preset alone — and add a test pinning that.
5. **GUI** — add the variant to `GRAPHICS_FIELDS`. (This is what makes it show up
   in the launcher menu.) Then add the matching label string to the in-game
   menu's `GRAPHICS_ENTRIES` mirror in `crate::hud::menu` (same order, before the
   trailing "Reset to High") — the `graphics_entries_match_field_labels` test
   pins these two lists in lockstep and fails if they drift.
6. **Presets** — set the field in **all** `for_preset()` arms (Low/Medium/High/
   Ultra; Custom inherits High). `preset_values_are_slot_aligned` will fail if a
   preset value isn't in its slot array.
7. **Apply** — write an `apply_<thing>_system` (mirror `apply_bloom_system` /
   `apply_anti_aliasing_system`) and register it in `crate::ViewerCorePlugin`
   (`lib.rs`), gated `run_if(resource_changed::<GraphicsSettings>)` unless it must
   track something else every frame.
8. **Tests** — extend the `settings.rs` test module: value_label smoke, cycle
   wrap, preset-cycle preservation/reset, and JSON roundtrip.
9. *(optional)* a `/<name>` slash command in
   `ffxi-client/src/view_native/slash_commands.rs` for quick in-session tweaking.

Keep the launcher renderer dumb: it should never need editing to show a new
option — if it does, the abstraction (iterate `GRAPHICS_FIELDS`) has leaked.
