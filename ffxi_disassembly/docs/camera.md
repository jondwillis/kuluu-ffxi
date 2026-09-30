# FFXI retail client: event camera control (C pass)

How the retail client drives the camera during events: which opcodes touch it, what the camera
manager object is, how positions cross between event work slots and camera space, and how the
look-at opcodes pose the actors. Findings **C1..** are a third pass, distinct from the mob pass
(**F**, [mob_animation.md](mob_animation.md)) and the event-VM pass (**E**, [event_vm.md](event_vm.md)).
Conventions (RVA base 0x10000000, POL1-packed `.text`, evidence tiers) are in [../README.md](../README.md).
Raw dumps: local untracked `out4/d_*.md` (this pass).

Target binary: `FFXiMain.dll`, build TDS 0x6A7297F5 (same build as the E pass).

## 1. Scope

| # | Question | Status |
|---|----------|--------|
| Q1 | What does 0x46 DEFCAMERA do, case by case, in this build? | Resolved (C1, C4) |
| Q2 | Where do 0xAF's camera reads come from, and what object is it? | Resolved (C2, C3) |
| Q3 | What scale converts between event work slots and camera space? | C4 restore multiplier approximately 1/1000; C2 round-trip conversion unresolved |
| Q4 | How do 0x1E / 0x4A / 0x79 pose an actor, and what does lookatone write? | Resolved (C5, C6, C7, C8) |
| Q5 | How does 0x47 move the player and report it? | Resolved (C9) |
| Q6 | What does 0x38 write? | Resolved (C10) |
| Q7 | Focal length and camera-route playback | Partial: focal local (C11, E15/E19); route playback still E14/E18, tag-0x04 dispatch not located |

## 2. The camera manager object (C2, C3)

**C3 [local].** The camera manager is a singleton reached through the global object @rva
**0x45693C**: `0x15250` is a 4-byte getter `return [0x45693C + 0x50]` (148 xrefs in `.text`).
Sibling getters on the same global: `0x15220` (+0x94), `0x15230` (+0xD4), `0x15260` (+0x114).
This is the `FUNC_CYmDB_GetCameraMng2` of the XiEvents pseudocode (research/XiEvents/OpCodes/0x0046.md).

**C2 [local].** The object's layout matches the vendored XiClient `CameraManager`
(`research/XIClient/src/XIClient/include/World/Camera/CameraManager.h`):

| Offset | XiClient member | Event-side use |
|--------|-----------------|----------------|
| +0x44..+0x4C | `CachedEyePosition` (x/y/z) | 0xAF sub 0 reads these (C2) |
| +0x50..+0x58 | `CachedLookAtTarget` (x/y/z) | 0xAF sub 1 reads these |
| +0x5C | `CachedRollAngle` | not read by any event opcode found in this pass |
| +0x8C..+0x98 | `NextEyePosition` / `NextLookAtTarget` | written by the 0x46 restore path (C1, C4) |
| +0xBC | `PositionHistory` | not touched by event opcodes |

0xAF @0xBB9D0 (width 8, always EP += 8): `sub = EventData[EP+1]`. sub 0: `setworkofs(2, conv(cam+0x44))`,
`setworkofs(6, conv(cam+0x48))`, `setworkofs(4, conv(cam+0x4C))`. sub 1: same shape from cam+0x50/+0x54/+0x58.
sub > 1: no-op. `conv` = `0x311C6C`, a 39-byte float->i32 round-to-nearest-even (`fnstcw; or ah,0xC;
fistp`), 953 xrefs. So the event work slots carry **rounded integer** camera coordinates:
workofs(2) = x, workofs(4) = z, workofs(6) = y — the same x->2, z->4, y->6 mapping the PS2 pseudocode
states (research/XiEvents/OpCodes/0x00AF.md).

## 3. 0x46 DEFCAMERA (C1, C4)

**C1 [local].** Handler @0xB6450 (thunk 0xBC4CF). Preamble: `ent = EntityTable[u16@rva 0x485FBA]`
(the event entity index in a global, not a GetActorIndex argument); gate `ent.RenderFlags0 & 0x200`
else tail. `sub = EventData[EP+1]`, cases 0..3 via jump table @rva **0xB6684** (entries
0x100B649A / 0x100B652A / 0x100B6510 / 0x100B657C; sub > 3 falls to the tail):

| sub | Entry | Behaviour (this build) | EP |
|-----|-------|------------------------|----|
| 0 | 0xB649A | validate (0xAE9C0 with byte@0x35CFB0), 0x3D5D0, 0x84220, then restore the camera to four position globals @0x35D000..0x35D00C: `mng = GetCameraMng2(); mng->0x1E790()` (SetAt-shaped) and `mng->0x21690()` (SetCameraPos-shaped, arg = ent.ActorPointer) | +2 |
| 1 | 0xB652A | validate (0xAE9C0, 0), 0x842E0 (sibling of 0x84220), then `call 0x221C40` on the object @rva 0x6346D8 (the menu-disable call of the PS2 pseudocode, `FUNC_KaListBox_DisableDraw`) | +2 |
| 2 | 0xB6510 | mid-function entry into case 0's tail: only the `0x21690` SetCameraPos-shaped call | +2 |
| 3 | 0xB657C | validate, 0x3D5D0, 0x84220, one-time restore from work slots (C4), then the same two manager calls | +6 |

Tail (gate fail / sub > 3): EP += 2. Widths are therefore 2, 2, 2, 6 — XiEvents lists "2, 4"
(research/XiEvents/OpCodes/0x0046.md); the 4 form is not what this build's case 3 advances.

Mapping to the PS2 pseudocode (web tier): sub 1 = "disable user camera control + disable menu draw"
(0x842E0 sets the control flag, 0x221C40 the menu); sub 0 = "re-enable: SetViewMode(NowView) +
YmCameraTask_KillAll + EnableUserControlCamera + SetAt + SetCameraPos" (0x84220 = the enable sibling,
the two manager calls = SetAt/SetCameraPos); sub 3 is the same re-enable with the position taken from
the work slots 0xAF just captured instead of the fixed globals. The 0xAE9C0 wrapper @0xAE9C0: reads the
event entity via 0x485FBA, requires RF0 bit 9, then tail-jumps 0xA8CD0 (a validation trampoline).

**C4 [local].** Case 3's one-time block: `if (!byte@rva 0x48982A) { byte |= 1; f32@0x489960 =
(workofs(2)) * f32@0x32A22C; f32@0x489964 = (workofs(4)) * f32@0x32A22C; f32@0x489968 =
(workofs(6)) * f32@0x32A22C; f32@0x48996C = 1.0f; }` where **f32@rva 0x32A22C = 0x3A83126F =
approximately 0.001 (1/1000)**. The four f32s then feed the two manager calls.
The exact coordinate conversion in C2 still needs to be reconciled with this scale;
these bits do not support a 32x coordinate-space claim. 0x47 uses the same positional
scale and a yaw multiplier of approximately 6.283/4096 (C9).

## 4. Look-at opcodes (C5, C6, C7, C8)

**C5 [local].** 0x4A @0xB6710 (width 9, always EP += 9): `i1 = GetActorIndex(code@EP+1)`,
`i2 = GetActorIndex(code@EP+5)`; both must resolve to non-null entities. Position source per entity:
if `ent.RenderFlags0 & 0x80` then `(obj+0x140, obj+0x148)` where `obj = [ent+0xD4 + 0x260]` (the
event VM's +0x260 sub-object, i.e. the authored event position), else `(ent+4, ent+0xC)` (the live
position). Then `angle = fpatan(p1y - p2y, p2x - p1x)` — the negated-dy form, i.e. the yaw that points
an actor's +X forward along the offset to its target (the same basis kuluu's round-12 look-at fix
implements). Storage: if the looker has RF0 bit 7: `[obj+0x150] = 0`, `[obj+0x154] = angle`,
`[obj+0x158] = 0`, `[obj+0x15C] = u32` (the second GA out-slot; the PS2 comment marks EventDir[3]
"possibly wrong"); else `[ent+0x18] = angle`, `[ent+0x14] = 0`, `[ent+0x1C] = 0`. Finally
`lookatone(code@EP+1, code@EP+5)`. XiEvents' PS2 pseudocode matches field for field, including the
bit-7 override and `atan2(-(y2-y), x2-x)` (research/XiEvents/OpCodes/0x004A.md).

**C6 [local].** `lookatone` @0xB8820 (thiscall xievent, two args, `ret 0xC`; 148-ish xref fan-in from
the 0x1E/0x4A/0x79 families): GA on both codes; both entities non-null; **both** `RenderFlags0 &
0x200`; then on ent1: `byte ent1+0x12D: and 0xFD; or 1` (bit 0 of the second byte of the RenderFlags3
dword @ent+0x12C set), `u16 ent1+0x146 = own u16 index`, `u16 ent1+0x148 = target u16 index`. No
RetFlag; no EP change (the caller advances).

**C7 [local].** 0x79 @0xB88F0 ("look at / rotate towards another entity"): `sub = EventData[EP+1]`.
sub 0: `lookatone(code@EP+2, code@EP+6)`, EP += 10. sub 1: same lookatone plus a `getworkofs(0xA)`
value pushed and left on the stack (the PS2 third argument, research/XiEvents/OpCodes/0x0079.md),
EP += 12. sub 2: GA(code@EP+2) -> ent; if non-null and RF0 bit 9: `byte ent+0x12D: and 0xFE; or 2`
(bit 1 of Flags3 byte 1 — a different look mode than lookatone's bit 0), `u16 ent+0x14A =
workofs(6)` (PS2 `LookAxisX`), `u16 ent+0x14C = workofs(8)` (PS2 `LookAxisY`); EP += 10. sub > 2 or
gate fail: EP += 10. Widths 10/12 match XiEvents.

**C8 [local].** 0x1E @0xB2D30 ("look at and begin talking", width 5, EP += 5): GA(code@EP+1) -> ent.
Position: if RF0 bit 7 then the entity's live position `(ent+4, ent+0xC)` is compared against the
event position — `ExtData = [xievent+0x260]`, `EventPos = (ExtData+0x140, ExtData+0x148)`; the angle
`fpatan(EventPos.z - ent.y, ent.x - EventPos.x)` (same negated form) is stored at **`ExtData+0x154`
= `EventDir[1]`**. Then `lookatone(xievent+8 /*event entity server id*/, code@EP+1)` — the event
entity (the NPC) is the looker, the code@1 entity the target. Matches the PS2 pseudocode
(research/XiEvents/OpCodes/0x001E.md), whose fourth argument (6) has no local counterpart.

## 5. 0x47 player position update (C9)

**C9 [local].** @0xB62C0 (width 2 / 10). `sub = EventData[EP+1]`. sub 1: if
`!byte@rva 0x4855E8 && !byte@rva 0x47FAEF` (the two RecPending flags) then EP += 2; RetFlag = 1
(yield while pending). sub 0: `v1 = workofs(2) * 0.001`, `v2 = workofs(4) * 0.001`,
`v3 = workofs(6) * 0.001` (f32@0x32A22C again), `v4 = workofs(8) * f32@0x329D2C * f32@0x330250`
where 0x329D2C = 0x40C90E56 = 6.283f and 0x330250 = 0x39800000 = 0.000244140625f (1/4096); then `call 0xA05F0(v1..v4)`
(`FUNC_SendPendingXzyTag`, the 0x005C-packet sender of the XiEvents page). On success:
`byte@0x4855E8 = 1; byte@0x47FAEF = 1; EP += 10`. On failure: RetFlag = 1 (retry next tick). The PS2
pseudocode also scales x/y/z by 0.001; the recorded bits agree with that positional scale.
The yaw multiplier is approximately 6.283/4096 (research/XiEvents/OpCodes/0x0047.md).
0x43 sub 1 polls the same 0x4855E8 flag (the event-report half of the pair; 0x43 sub 0 sends via
`0xA0520`, its sibling of 0xA05F0).

## 6. 0x38 local mode (C10)

**C10 [local].** @0xB6190 (width 3, EP += 3): `ax = getworkofs(1); ah |= 0x20; u16@rva 0x48093C = ax`.
0x48093C is `CliEventModeLocal` (research/XiEvents/OpCodes/0x0038.md: hides the local player, hides
UI pieces, allows the camera to move apart from the player). The PS2 pseudocode writes
`HIBYTE(val) | 0x20` into the whole word (low byte zeroed); this build ORs 0x20 into the high byte
and keeps the low byte. 0x48093C is also one of the three EventIdle gate bytes' neighbours (E9 lists
0x480502 / 0x487074 / 0x480930 as the gate; 0x48093C is a separate word the reset routine clears, C-
appendix below).

## 7. Focal length and routes (C11)

**C11 [local + web].** The 280/350 selection is local: @rva 0x5940A..0x5947D branches on
`byte@rva 0x487FC0` (nonzero -> 280.0f first-person, else 350.0f) and passes the value to the focal
setter @rva 0x15290 (E19). The XiClient side confirms the semantics: `CameraTask.cpp` builds the
endpoint list from the camera resource's control points (Position, FovCalculationParameter, Target,
Roll — the 0x06 route keyframes of E18) and, for `END_AT_CURRENT_POS`, sets the final focal to
`280.0f` if `ControllableActor::is_first_person_view` else `350.0f`; `CameraManager.cpp` eases the
projection focal by `CheckTick() * 6.0f` per tick toward/away from 350.0f. `FOV_deg = 2 *
atan2(192, focal)` with the 192 half-viewport-height term staying web tier (E15). Route playback
itself is the 0x2D zone action (E14: `[zoneObj->vt+0x18](key, actor2, actor1)`); the tag-0x04
scheduler dispatch that turns a 0x06 route into a CameraTask was not located in this build (E19
open item, still open).

## 8. Globals and entity fields found in this pass

| RVA / offset | Meaning |
|--------------|---------|
| 0x485FBA | u16 index of the event entity in the global table @0x480B30 (0x46, 0xAE9C0) |
| 0x45693C | camera global object; +0x50 = camera manager (C3) |
| 0x48093C | CliEventModeLocal u16 (0x38) |
| 0x48982A | one-time flag for 0x46 case 3's work-slot restore |
| 0x489960 / 0x489964 / 0x489968 / 0x48996C | restored camera x / z / y / (1.0f) f32s |
| 0x32A22C | approximately 0.001f (1/1000) work-slot -> camera-space scale |
| 0x329D2C / 0x330250 | 6.283f / 0.000244140625f (0x47 yaw scale) |
| 0x4855E8 / 0x47FAEF | RecPendingFlag / RecPendingXZYFlag (0x47, 0x43) |
| 0x6346D8 | menu object for 0x46 sub 1's disable call (0x221C40) |
| ent+0x14 / +0x18 / +0x1C | pitch / yaw / roll f32s (0x4A storage) |
| ent+0x12C | RenderFlags3 dword; byte +0x12D bit 0 = lookatone active, bit 1 = 0x79 sub-2 look mode |
| ent+0x146 / +0x148 | u16 self index / target index (lookatone) |
| ent+0x14A / +0x14C | u16 LookAxisX / LookAxisY (0x79 sub 2) |
| ent+0xD4+0x260 | event VM sub-object: +0x140/+0x148 EventPos x/z, +0x150..+0x15C EventDir (0x4A bit-7 path, 0x1E) |
| xievent+0x260 | ExtData pointer (0x1E: EventPos/EventDir live here) |

## 9. Findings index

| C | One line | Evidence |
|---|----------|----------|
| C1 [local] | 0x46 case-by-case decode; widths 2/2/2/6; callee map | out4/d_46_defcamera.md |
| C2 [local] | 0xAF reads CachedEye/LookAt (+0x44/+0x50), conv 0x311C6C, workofs 2/4/6 = x/z/y | out4/d_af_campos.md |
| C3 [local] | 0x15250 = [0x45693C+0x50] camera manager getter; layout matches XiClient CameraManager | out4/d_cammng_getter.md, XiClient header |
| C4 [local] | work-slot -> camera scale approximately 1/1000 (0x32A22C); one-time restore into 0x489960..0x48996C | out4/d_46_defcamera.md |
| C5 [local] | 0x4A: GA(1)/GA(5), bit-7 EventPos override, negated-atan2 yaw to ent+0x18 (or VM +0x154) | out4/d_4a_lookat.md |
| C6 [local] | lookatone @0xB8820: both RF0 bit 9; ent1+0x12D bit 0, +0x146/+0x148 self/target u16 | out4/d_lookatone.md |
| C7 [local] | 0x79: sub0 lookatone EP+10, sub1 +workofs(0xA) EP+12, sub2 Flags3 bit 1 + LookAxis 0x14A/0x14C | out4/d_79_lookat2.md |
| C8 [local] | 0x1E: angle event-pos -> entity into ExtData+0x154; lookatone(event entity, code@1); EP+5 | out4/d_1e_looktalk.md |
| C9 [local] | 0x47: xz/y x 0.001, yaw x 6.283/4096 -> 0xA05F0; pending flags 0x4855E8/0x47FAEF; 0x43 polls the same | out4/d_47_posupdate.md, out4/d_43_eventreport.md |
| C10 [local] | 0x38: u16@0x48093C = workofs(1) with high byte \| 0x20 (low byte kept, unlike PS2) | out4/d_38_localmode.md |
| C11 [local+web] | 280/350 @0x5940A via 0x487FC0 (E19); CameraTask END_AT_CURRENT_POS and 6.0f/tick easing web-confirmed | out4/, XiClient CameraTask.cpp / CameraManager.cpp |

## 10. Kuluu-facing notes

- The look-at basis is the negated atan2 (C5): `from_rotation_y(-dz.atan2(dx))` — already what
  kuluu-render's `look_at_rotation` does after the round-12 fix; the entity-side storage offsets
  (ent+0x18 yaw, +0x12D bit 0, +0x146/+0x148) are the retail fields a generic look-at consumer would
  mirror.
- 0x46 sub 1 is the "camera off + menu off" pair; kuluu's cutscene HUD-hide (0x67, see
  [ui.md](ui.md) U1) is the wider variant. The two are independent opcodes.
- The recorded C4 multiplier is approximately 1/1000. Reconcile the C2 read conversion before
  deriving a camera-position round trip through event work slots (0xAF -> 0x46 case 3 / 0x47);
  the recorded bits do not establish a 32x coordinate convention.
- Camera-route playback (0x2D -> 0x06 route -> CameraTask) remains the open item from E14/E19; this
  pass located the manager and the focal path but not the tag-0x04 dispatch.
