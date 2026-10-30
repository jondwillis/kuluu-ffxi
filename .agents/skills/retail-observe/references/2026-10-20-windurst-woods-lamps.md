# Windurst Woods tunnel lamps

## Scope

This observation covers the Windurst Woods tunnel lamps in zone 241 and DAT 341.
The original client ran under Wine against the local LandSandBoat server.
It does not establish clean current-retail parity or an exact camera response rate.

## Inputs

The disposable character was `Lamptest`.
The viewport was 800 by 600 guest pixels.
The host capture was 1608 by 1272 pixels, including the window border, at scale 2.

The supported GM self commands were:

```text
!pos -120 -7.65 40
!pos -140 -10.05 40
!pos
```

The collision probe found lower floors at FFXI vertical positions `-7.650` and `-10.050`, respectively.
Its evidence is `artifacts/verify/release-080-current-main/camera-live-comparison/zone241-floor-probe.txt`.
The three-argument self commands changed the visible position.
A five-argument command with the same zone and character did not change the observed position.
Its failure cause remains unknown.

## Observed output

The first corrected view shows complete tunnel stairs and two small warm lamp glows beyond the arch.
The second corrected view shows four bright lamp cores with soft yellow and amber light around them.
The light remains close to each lamp and the adjacent wall.
The inspected views do not show large opaque white sheets across the tunnel.

The evidence directory is `artifacts/verify/release-080-current-main/retail-reference/`.
The first corrected capture is `selfmove-result.png`; its game clock reads `1:18`.
The second capture is `woods-near-floor.png`; its game clock reads `1:48`.
Both captures were opened for visual inspection.

The subsequent position query reports:

```text
Lamptest: X -140.0000 Y -10.0500 Z 40.0000 Rot 0 Zone 241 Floor 0
Terrain: Stone
```

The query capture is `near-query-result.png`; its game clock reads `2:03`.
The local text output is `near-query-result.ocr.txt` in the same directory.

## Input checks

One right-button drag moved 120 guest pixels horizontally and held that displacement for one second.
The camera direction changed visibly.
The displacement was 0.15 of the viewport width.
The before and after captures are `woods-ready.png` and `woods-drag120-after.png` in the evidence directory.

One wheel-up step and one wheel-down step produced no visible focal change.
The helper submitted both inputs, but delivery to the original camera zoom channel remains unverified.
These results do not establish the original wheel zoom scale.
A later bounded normal-message test sent one positive wheel notch to the unique foreground game window.
The window acknowledged the message, but inspected before and after captures showed no visible camera change.
The test stopped after that one message.
The proof is `wheel-message-proof.json` in the evidence directory.
Message acknowledgement does not establish an eligible camera zoom response or physical-device parity.
No matched native mouse input was available through the permitted native socket.

## Limits

The installation uses HorizonXI branding and the `horizonxi-2023` client profile.
Wine 11.0 supplied the runtime.
The graphics modification state was not fully established.
The capture does not measure exact lamp dimensions, alpha, flicker, or camera eye position.
The two captures use different night times and character positions.
They support the local appearance comparison, not exact pixel equivalence.

The initial position `(-120, -4, 40)` was below the verified collision floor.
Its position query reported `Terrain: None`.
Missing surfaces in those earlier captures are not evidence of a rendering defect.

Long literal text input dropped characters in one command entry.
Short literal chunks preserved the command.
Local text recognition checked each captured chunk before submission.
The incorrect entry was cancelled without submission.

## Provenance

- Observation date: 2026-10-20 UTC.
- Client profile: `horizonxi-2023`.
- Client DLL SHA-256: `f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
- DAT 341: `ROM/0/81.DAT`.
- DAT SHA-256: `77c82c162548c3d8cde14975f3382e3660dae13d8ca375485d403ba9cfd82ba3`.
- The registered HXI and current-retail installations supplied byte-identical DAT 341 files.
- Server command source: `vendor/server/scripts/commands/pos.lua`, `commandObj.onTrigger`.
- The deployed `/server/scripts/commands/pos.lua` matched the checked source during observation.
- Host window: 69883; owner PID: 10708.
- Private profile: `lsb-jev`; credentials were not recorded in this observation.

Logical DAT 341 resolves through each install's VTABLE and FTABLE to `ROM/0/81.DAT`.
Both tables select ROM 1 and file word 81; the file size is 9281824 bytes.
The files were compared directly and are byte-identical.
Resolver proof is `artifacts/verify/release-080-current-main/retail-reference/dat341-resolver-proof.json`.
The earlier `ROM/1/37.DAT` path and hash described another file and are superseded.
