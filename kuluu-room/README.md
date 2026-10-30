# Kuluu animation room

The standalone room runs animation, particle, lighting, and zone cases through
the production Kuluu renderer. It opens directly, without the launcher, an
account, or a server connection.

The room uses default graphics settings. It does not read or change saved player
settings. Its initial lighting does not depend on a launcher zone.

Select a game install with `kuluu install use NAME`, or set `FFXI_DAT_PATH`.
Start the room:

```bash
cargo run -p kuluu-room -- --unfocused --mute
```

The `--unfocused` option opens a visible window without requesting keyboard focus.
The `--mute` option disables music and sound effects.

Read the controls supported by this build:

```bash
cargo run -p kuluu-room -- --knobs
```

This command needs no game install, server, or window. The report lists case
names, environment variables, and automatic case timing. The `ANIMTEST_*`
environment variable names remain unchanged. Set `ANIMTEST_AUTO` to run a sequence
of cases. Use the reported screenshot case and output variable for GPU readback.

The room has no key-drive socket. It does not support Windows offscreen parking
through `KULUU_WINDOW_HIDDEN`.

Use `--no-install` only to test the missing-install diagnostic. This option does
not load game files and cannot establish rendering correctness.

Follow the [verification skill](../.agents/skills/verify/SKILL.md) for evidence
requirements. Open and inspect each capture before reporting a visual result.
