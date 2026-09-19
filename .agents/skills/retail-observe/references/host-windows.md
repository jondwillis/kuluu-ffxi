# Host: native Windows

Use `scripts\observe.ps1` from PowerShell. It implements the same verbs as
`observe.sh` and reads the same `lib\keys.tsv`, so a drive recipe written on
another host reads here unchanged.

**Verification status:** written against documented Win32 behavior, not yet
exercised against a running client. `doctor` checks every prerequisite it
relies on -- run it first, and fix what it reports rather than writing a second
script beside it.

Native Windows is the simplest arrangement: no VM layer, no translation, the
client's own DirectX path, and the loaders behave exactly as their authors
intended. It has two failure modes the other hosts do not.

## Integrity levels beat everything

Windows refuses to deliver synthetic input from a lower integrity level to a
higher one. If the client runs elevated and your PowerShell does not, every key
and click is dropped **with no error at all** -- the same signature as a wrong
window handle. `doctor` prints whether this session is elevated; match it to the
client, or (better) run the client unelevated.

Clicking through a UAC prompt is a human's decision. `click-text` refuses while
one is on screen; do not work around that by using raw `click`.

## DPI scaling silently moves every click

On a scaled display, a non-DPI-aware process reads virtualized window
rectangles, so coordinates are wrong by the scale factor without anything
looking broken. `observe.ps1` calls `SetProcessDPIAware` at startup, which is
why its `capture` reports `scale:1x`: window units and image pixels are the same
number. If you write your own helper, do the same or your clicks will drift.

## Input: scan codes, not just virtual keys

FFXI reads the keyboard through DirectInput, which looks at scan codes. Injecting
a virtual key alone is the usual reason a game "ignores automation", so
`observe.ps1` maps each VK through `MapVirtualKey` and injects with
`KEYEVENTF_SCANCODE`. If a key still does not register, that is the layer to
debug -- not the key table.

`type` goes through `SendKeys`, which treats `+ ^ % ~ ( ) { } [ ]` as syntax;
`observe.ps1` braces those automatically. A literal `/` is safe here, unlike the
AppleScript path on macOS.

## Capture

`capture` uses `Graphics.CopyFromScreen` over the window rectangle, which reads
what is actually composited -- the right choice for a DirectX client, at the cost
of needing the window unobstructed and not minimized. `observe.ps1` brings the
window forward first.

`PrintWindow` is the alternative when you need an occluded window, but it
returns black for many hardware-accelerated surfaces, so it is not the default.

## OCR

`ocr` and `click-text` need **tesseract** on PATH (`winget install
tesseract-ocr` or the UB Mannheim build). Without it the verbs refuse rather
than guessing, and you read captures yourself.

Windows 10+ also ships `Windows.Media.Ocr`, which needs no install but is
awkward to drive from PowerShell (WinRT async projection). If tesseract is
unacceptable in your environment, that is the route to implement -- keep the
output contract identical (`TEXT<TAB>x<TAB>y`, window units, one line per visual
line) so nothing above it has to change.

## Launching

Nothing special: `launch` runs the loader in place with the install directory as
the working directory, because loaders resolve DATs, config and their own DLLs
against the working directory rather than argv[0]. See
[client-and-server.md](client-and-server.md) for loaders, servers and
credentials.

## Troubleshooting

| Symptom | Cause -> fix |
|---|---|
| No window matches but the client is visible | The launcher/config window has a different title -> `targets`, then set `FFXI_OBSERVE_WINDOW_TITLE` |
| Keys and clicks do nothing, no error | Integrity-level mismatch (elevated client, unelevated shell) -> `doctor`, then match them |
| Some keys work in menus but not in game | Virtual key without scan code reaching DirectInput -> confirm the scan-code path is in use |
| Clicks land at the wrong place on a scaled display | DPI awareness not set in your process -> `SetProcessDPIAware` before reading rectangles |
| Capture is black | The window is minimized or occluded, or a fullscreen exclusive mode is active -> restore/foreground it, or run the client windowed |
| `ocr` refuses | tesseract missing -> install it, or read the capture directly |
| Input lands in the terminal | A raise from an earlier invocation; each verb raises in-invocation, so do not batch keys around a single `show` |
