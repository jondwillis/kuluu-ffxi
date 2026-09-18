# Host: macOS

Two arrangements run the client here, and `scripts/observe.sh` drives both
through the same path: CGWindowList finds the window, `screencapture -l` takes
its pixels, CGEvent posts input, Vision does OCR. Nothing talks to a guest
agent, so the code does not care which arrangement you chose.

| Arrangement | What owns the window | Set up |
|---|---|---|
| **Wine** (no VM) | the Wine process, usually named `wine` | nothing, or `FFXI_OBSERVE_RUNNER=wine` for `launch` |
| **VM** | the guest exe (Coherence) or the VM console | `FFXI_OBSERVE_VM_NAME='<vm name>'` |

Wine is the lighter arrangement and the better default: no guest OS, no UAC, no
Spaces fight, and the client window behaves like any other macOS window.

## Host permissions -- the two silent failures

The terminal running `observe.sh` needs, in System Settings > Privacy &
Security:

- **Screen Recording**, or every capture comes back black;
- **Accessibility**, or every key and click is silently dropped.

Both fail quietly, which is why `observe.sh doctor` preflights them
(`CGPreflightScreenCaptureAccess` / `AXIsProcessTrusted`) instead of letting you
misread the result as a broken window match. A sleeping display also captures
black: `observe.sh` runs `caffeinate -u` before each capture, and for a long
session keep `caffeinate -d -u` running.

## Wine

Community wrappers exist because a plain Wine build is not the hard part -- the
client's DirectX 8 rendering is. Prefer one of them over assembling your own:

- **FFXI on Mac** (`danielalanbates/HorizonXI-on-Mac`) -- Apple Silicon, macOS 13+,
  ships its own Wine build and downloads a client per world, with separate game
  folders and logins per server. Roughly 24-28 fps at 4K on an M1, and a few
  Ashita plugins fail to load harmlessly.
- **Whisky** or **CrossOver** -- general Wine wrappers; you supply the client.
- **Plain `wine`** -- workable for the launcher and tools; the game's D3D8 path is
  the part that needs a wrapper's attention.

What was verified on this host (macOS 26, Apple Silicon, Homebrew `wine-stable`
11.0, driving a Win32 app under Wine):

- the CGWindow **owner is `wine`**, and the **title is the app's own Win32 title**
  -- so the default `FINAL FANTASY` title match finds the game with no config;
- `screencapture -l` captures the Wine window correctly, including the 2x Retina
  scale factor that `capture` reports;
- **focused CGEvent keys and `type` reach the Win32 app**, and so does `--bg`
  (`CGEventPostToPid`) -- unfocused posting works under Wine, unlike some VM
  builds;
- **OCR coordinates land where they should**: `click-text` on a word moved the
  app's caret into that word, which is the end-to-end check of the
  pixel-to-point conversion;
- `System Events` lists the GUI process as `wine`, which is how `show` focuses
  it when the window's owning pid is a child process it cannot address.

Two Wine-specific traps:

- **A window can outlive its app.** Kill `wineserver` (or the app hangs) and
  macOS keeps compositing the last frame: the window still resolves, captures
  fine, and accepts no input. The tell is OCR that is byte-identical across
  several inputs while a caret or counter never moves. Relaunch rather than
  debugging the input path.
- **First launch in a fresh prefix is slow.** Prefix bootstrapping can take tens
  of seconds before any window appears. Poll `observe.sh status` rather than
  concluding the launch failed, and suppress the Mono/Gecko installer prompts
  with `WINEDLLOVERRIDES="mscoree,mshtml="` when scripting a bootstrap.

## VM (Parallels, VMware, UTM)

A VM costs a guest OS and its consent prompts, and buys exact Windows behavior.
Set `FFXI_OBSERVE_VM_NAME` so a console window titled after the VM can be
matched when the game window itself is not separately visible.

### Coherence mode -- why the window looks like a macOS app

In Parallels Coherence, guest windows are blended onto the macOS desktop. That
inverts a few intuitions:

- The game window's CGWindow **owner is the guest exe** (`horizon-loader.exe`),
  not `prl_vm_app`. Matching on the title rather than the owner is what makes
  this work by default.
- The title carries **trailing NUL bytes** (`FINAL FANTASY XI\0\0`). Matching is
  substring, so this is harmless -- but never match on exact equality.
- `screencapture -l` grabs the window **even across Spaces and when macOS reports
  it off-screen**, so `capture` works without fighting the window forward.
  `show` (needed only for input focus) raises the guest app; frontmost then
  reads as **`WinAppHelper`**, the Parallels input shim. That is success, not
  failure -- keys and clicks still land in the guest.
- Classic (non-Coherence, full-screen) windows go through `prl_vm_app` normally.

### Parallels Standard has no guest-side tooling

`prlctl exec` and `prlctl capture` are Pro/Business features. That is the
original reason this whole skill goes through the window server instead of a
guest agent -- and the reason the same code now works for Wine. Do not reach for
`prlctl exec`; it will simply error.

### Getting from the guest desktop to the game

Windows desktop -> the server's launcher -> the FFXI loader. From the host you
can start either through its Parallels app stub:

```bash
open ~/"Applications (Parallels)"/*Applications.localized/"HorizonXI-Launcher.app"
```

The game window itself is `horizon-loader.exe.app` in that same folder. A cold
start runs: PlayOnline user agreement (**Accept**) -> "Acquiring FINAL FANTASY XI
server data" -> character select. Character select is the login handoff point.

Launcher windows are usually titled differently from the game, so drive them
with an override, e.g.
`FFXI_OBSERVE_WINDOW_TITLE='HorizonXI Launcher' observe.sh ocr`.

### VM-specific gotchas

- **Launch tickets expire within seconds.** Click the launcher's "Ready! Click to
  Launch" fast (or fire `--bg` clicks in a tight loop). A crashed prior session
  shows "Ticket not found, expired, or already used" until the server drops the
  ghost session -- wait it out rather than logging out, since re-login needs the
  account password.
- **A slow disk stalls the client's UI thread**: the title shows "(Not
  Responding)" during loads and input sent then is *dropped*. The 3D background
  keeps animating on another thread, so a live capture is not proof of a
  responsive client. Send confirm keys only once the title clears.
- **Windows Update can restart the VM** mid-session (watch for "We've got an
  update for you" -- dismiss with **Another time**), which loses the login.

## Troubleshooting

| Symptom | Cause -> fix |
|---|---|
| Black or empty capture | Screen Recording not granted, or the display is asleep -> `doctor`, and keep `caffeinate -d -u` for long sessions |
| Keys/clicks silently ignored | Accessibility not granted, or the window is not frontmost -> `doctor`, then `show` |
| Input lands in the terminal instead of the game | A raise that happened in an *earlier* invocation. Every input verb re-raises in-invocation; do not batch keys around a single `show` |
| OCR identical across several inputs, caret/counter frozen | Wine app orphaned from its `wineserver` (stale composited window) -> relaunch the client |
| No window matches, but the client is running | It is titled differently (launcher, config tool) -> `targets`, then set `FFXI_OBSERVE_WINDOW_TITLE` |
| VM `running` but no guest window exists at all, and raising does nothing | Coherence with zero guest windows open -- there is nothing to raise and launching an app stub silently no-ops. Open the VM console (`open "<path>/<vm>.pvm"`), which is owned by **`Parallels Desktop`** and titled after the VM: set `FFXI_OBSERVE_VM_NAME`. In this mode the game window lives INSIDE the console, so `capture` grabs the whole guest desktop (crop before reading), `ocr` picks up desktop clutter, and click coordinates are console-relative |
| Every window query fails, which reads as "wrong Space" | A JXA error, not a missing window -- rerun `host_windows_json` without `2>/dev/null`. A regex containing a space once word-split the osascript arguments; the owner regex is passed as argv for exactly this reason |
| `show` never raises in a VM | Some installs expose no System Events process named "Parallels Desktop", only `prl_client_app`/`prl_vm_app`. The raise cascade tries all of them plus a direct AXRaise |
| Frontmost reads as `WinAppHelper`/`prl_vm_app` after `show` | Expected in Coherence -- that shim holds focus for the guest; input still lands |
| Loader opens no game window | The client's background resolution is too high -- 4096 squared wedged it, 2048 squared works. Fix it in the launcher or FFXI Config, not here |
| Clicks land off-target | Pixel coordinates used as points -> divide by the scale factor `capture` prints |
| Game ignores held keys | Hold shorter than the input poll -> use at least 0.1s |
| Clicks are OCR-verified and land, but nothing happens; the client closes by itself | Another session (human or agent) is driving the same client concurrently, and the inputs interleave. Do not fight for focus: stop, report the contention, agree who owns the client |
| A native Win32 popup ignores input entirely (e.g. the standalone gamepad config tool) | Posting to a pid does not reach guest-spawned native dialogs -> fall back to `osascript` `System Events` `key code ...` after activating the VM app; answer any "Save changes?" prompt No unless changes were intended |
