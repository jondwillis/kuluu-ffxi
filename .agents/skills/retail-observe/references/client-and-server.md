# Select a client, loader, and server

Select the client installation and server independently.
A private-server installation can include its own launcher, loader, and addons.
Keep installation files outside the checkout.

## Find the installation

```bash
kuluu install list
kuluu install path NAME
```

Use the returned path in the selected [host procedure](../SKILL.md#select-one-host-procedure).
The macOS Wine procedure uses `macos.py setup --install PATH`; its profile is private JSON.
The Linux shell procedure also accepts `FFXI_OBSERVE_INSTALL=kuluu:NAME`.
A complete installation includes `FINAL FANTASY XI/FFXiMain.dll`, the DAT files, and the PlayOnline viewer.

## Select a compatible loader

| Loader | Purpose |
|---|---|
| `pol.exe` / `polboot.exe` | PlayOnline viewer startup |
| `xiloader.exe` | Original client against a community server |
| Ashita or a server-branded launcher | Server-specific authentication, configuration, or addons |

Use the selected loader's help for its supported arguments.
Do not apply xiloader arguments to another launcher.
For macOS Wine commands, use [host-macos.md](host-macos.md); do not create a second shell profile.

Check these independent compatibility requirements:

- Match the loader version to the server's accepted version.
  LSB defines `SupportedXiloaderVersion` in `vendor/server/src/login/auth_session.h`.
- Match the loader to the installed `polcore.dll`.
  A missing loader patch pattern can prevent launch even when the server accepts the loader version.
  xiloader 2.1.2 failed on the 2026-09 viewer with `Failed to locate profileServerPortAddress2!`.
- Match the client version to the server settings.
  Use the installation registry's client profile instead of assuming that all installations contain the same build.
- Match auth, lobby, profile, and data ports to the selected server.

## Local LSB

Use the [verify skill](../../verify/SKILL.md) to prepare the local stack.
The map server must advertise an address the client can reach.
The account and character must exist before character selection can succeed.
Agents can drive authorized local test accounts without a credential handoff.

For the optional OrbStack connect/profile supplement, use the single procedure in [host-macos.md](host-macos.md#optional-local-orbstack-server-supplement).
Do not start duplicate services on occupied ports.
A rejected client version does not prove that input or capture is broken.

## Private servers and display settings

Follow the server's supported launcher and authorized login procedure.
Keep credentials out of profiles and logs.
A crashed session can retain its login slot; wait for server cleanup before another login attempt.

FFXI stores display settings in the Windows registry, including the Wine prefix's registry.
If no game window appears, check the background resolution in FFXI Config before changing the observation tools.
Use windowed mode for window-targeted captures.
Configuration applications have separate window titles; use the host procedure's discovery commands when needed.

A listed window can be loading or unresponsive.
Verify input with a fresh capture of a reversible action.
Stop if repeated input causes no visible change, or another driver is active.
