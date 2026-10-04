# Melee damage dispatch, 2026-10-04

## Scope and evidence strength

This observation combines the clean authored effect DAT with independently
inspected scheduler handlers from the installed `retail-2026-09` and
`horizonxi-2023` clients. It establishes authored ordering, routine context,
missing-reference behavior in the inspected handler, and important distinctions
between condition inputs. It does not establish the complete packet-to-action
pipeline, live animation timing, or the death transition boundary.

## Authored ordinary melee chain

Both installations have the same global effect file, DAT 0, resolving to
`ROM/0/0.DAT`. Its `dada` routine calls `atpr` immediately, then schedules these
three stages at authored delay 4, in this order:

1. Same-context subroutine `crtl`.
2. Damage callback opcode `0x2B`.
3. Same-context subroutine `dam0`.

The inspected Savanna Rarab model, DAT 1569 (`ROM/4/109.DAT`), calls `dada`
from `ati0` at authored delay 32. This authors the callback and the two
subroutines at delay 36 in the combined sequence. The selected branches of
`dam0` and `crtl` have their own delay 1. These are authored engine timing
units; the record does not equate them with wall-clock time or display frames.

The damage callback opcode does not itself name `dam0`, `crtl`, or `ldam`.
The named routines above are authored stages in `dada`. It is therefore
necessary to distinguish the callback from the surrounding sequence when
reasoning about missing routines or timing.

## Routine actor roles

The ordinary same-context subroutine opcode preserves the caller's caster and
target. Thus when `dada` runs with attacker as caster and victim as target,
`crtl` and `dam0` inherit those roles.

The reaction calls inside `dam0` use the target-side subroutine opcode `0x09`.
That opcode resolves the routine on the target actor and starts it with the
actor roles exchanged. Consequently the reaction itself runs on the victim,
with the attacker as its target. Evaluating `dam0` as a sequence under the
attacker's context and executing its selected reaction on the victim are
separate operations.

`crtl` calls `hi14` or `hi29` with the same-context opcode. Those spark routines
retain attacker-as-caster and victim-as-target. The clean `crtl` routine does
not call `ldam`. The clean global `dam0` routine also does not call `ldam`;
`ldam` is instead named by the separate global `daml` routine's successful-hit
arm. This is a statement about these authored routes, not proof that no other
retail sequence ever invokes `daml` or `ldam`.

## Conditions selecting reactions

For the ordinary successful-result arm (`dam0` selector `0x28` equal to zero),
`dam0` tests selector `0x3B` against one. A true result calls victim-side
`damh`; the alternative calls victim-side `damg`.

The client obtains selector `0x3B` from the caster actor. It does not obtain
that selector through the result-record accessor used by selector `0x28` and
selector `0x2B`. This investigation has not established the observable meaning
or producer of that actor state. In particular, mapping selector `0x3B`
directly to the packet result's information or defeated bits is not justified
by this DAT branch table or the inspected selector implementation.

`crtl` tests mask 2 on selector `0x2B`, then uses selector `0x38` to choose
`hi14` versus `hi29`. Selector `0x2B` reads a word from the current result
record. The full wire-decoder-to-record mapping remains outside this bounded
inspection. Selector `0x38` derives from the attacking actor's identity
classification; it is not a random choice. The branch selects `hi14` for the
true classifier and `hi29` for the alternative.

Both global `damg` and `damh` author a caster flinch stage, opcode `0x21`.
Their initial target-side subroutine is respectively `chit` and `chih`, and
both also author same-context `sdam` and `vdam` calls. The selected routine's
caster is the victim because `dam0` calls it through the target-side opcode.
The clean `damh` data does not support describing it as a routine with only
visual effects and no flinch stage.

## Missing-resource behavior

The inspected same-context subroutine handler first uses an existing resolved
reference or searches the current document's resource hierarchy by the authored
name. If that search supplies no reference, the handler returns without
starting a replacement routine. There is no hardcoded substitution of `damg`
or `ldam` in this inspected missing-reference branch.

Resource hierarchy lookup and target-side actor lookup are separate fallback
surfaces. The target-side handler contains additional actor/global resource
searches. This record does not specify the whole resource-search order or
claim that deleting one local resource makes it unavailable globally.

## Death timing remains unresolved

The authored `dada` stages establish where the callback sits relative to
`crtl` and `dam0`; they do not establish when a defeated result latches dead
status or starts the model's `dead` collapse routine. The inspected callback
forwards to result processing. Its immediate chain was not sufficient to tie
the complete dead-status transition and collapse start to either packet receipt
or the callback's authored delay.

No original-client window was driven. The proposition that a killing blow
starts `dead` exactly on this callback, and any distinction between dead-status
latching and fall-over onset, remain unverified here. Existing DAT evidence
that `dead` contains collapse and corpse-pose motion does not settle that
invocation boundary.

## Provenance

The inputs are the measured `KNOWN_CLIENTS` rows:

- `retail-2026-09`, patch `30260904_1`, DLL
  `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`.
  DLL SHA-256 `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
  Fresh unpacked text SHA-256
  `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
- `horizonxi-2023`, patch `30230905_0`, DLL
  `/Users/jon/Library/Application Support/kuluu/installs/hxi/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`.
  DLL SHA-256 `f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
  Fresh unpacked text SHA-256
  `f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`.

Both image bases are VA `0x10000000`, with text at RVA `0x1000`.
The bounded independent unpack prepared during this session produced
`/private/tmp/emission-retail.text.bin` and
`/private/tmp/emission-hxi.text.bin`, matching the row hashes. Capstone 5.0.7
was used for ephemeral inspection. Installed binaries were neither changed
nor executed.

Both `ROM/0/0.DAT` files have SHA-256
`5b2ac1bc3efbb73a3c9ffbb884ed6b7066355910a91635bfeef14bd027c6c158`.
The inspected chunks are `dam0` at file offset `0x7F900`, `crtl` at `0x80070`,
`daml` at `0x80BE0`, and `dada` at `0x81800`. The existing trusted
`target/debug/examples/dat-routine-stages` was run with explicit clean retail
`FFXI_DAT_PATH` for DAT 0's relevant routines and DAT 1569's `ati0` and `damg`.
An independent bounded chunk/stage walk corroborated the authored branch words.
The normal-hit inner `dam0` test carries field-selector words `0x0003001C,
0x0000003B`, comparison words `0x0001001C, 1`, and equality operation `0x0C`.
The critical gate in `crtl` carries field-selector `0x2B`, comparison 2, and
mask-test operation `0x11`; its variant predicate uses field-selector `0x38`.

The community hypothesis maps were
`research/XIClient/src/XIClient/source/Game/Scheduler/CMoSchedulerTask.cpp`,
`Tags/0x03.cpp`, and the actor attachment accessors. XIClient's handlers for
`0x2B`, `0x09`, `0x64`, and `0x6B` are undecompiled stubs in this checkout.
`research/xim/src/jsMain/kotlin/xim/resource/EffectRoutineInstance.kt` was read
only as a condition-variable hypothesis map. Neither community source is the
decisive runtime evidence below.

For retail-2026-09, opcode dispatch starts at RVA `0x57FB0`: it subtracts two
from the opcode and indexes the handler table at RVA `0x5DC1C`.

- Opcode `0x03`, RVA `0x5946D`: ordinary subroutine execution calls the target
  accessor at RVA `0x627D0`, then the caster accessor at RVA `0x62770`, preserving
  their roles when calling scheduler execution at RVA `0x56BB0`. Its unresolved
  path at RVA `0x594D3..0x5950E` searches through RVA `0x72290` and returns via
  RVA `0x5AC96` if no reference is found.
- Opcode `0x09`, RVA `0x597EF`: target actor resource lookup uses attachment
  target getter RVA `0x3B900`; the successful execution at RVA
  `0x598BC..0x598CF` pushes the caster from RVA `0x57BE0`, then target from RVA
  `0x57BF0`, exchanging their argument roles at the same execution entry.
  The intermediate missing-resource paths contain further actor and global
  searches; their full policy was not reconstructed.
- Opcode `0x2B`, RVA `0x593B4`: passes the task's callback/result object at
  offset `0x90` to RVA `0x18C810`; it does not directly dispatch named damage
  routines. RVA `0x18C810` finds the next unprocessed result, forwards to RVA
  `0x9E090`, then marks the result processed. The examined RVA
  `0x9E090..0x9E36C` path handles result/message effects; broader death-state
  callers were not identified decisively.
- Field selection helper RVA `0x62900`: selector `0x3B` at RVA
  `0x6295D..0x62999` obtains the caster and invokes vtable slot offset `0x254`.
  Actor vtables at RVAs `0x32D710`, `0x330F40`, and `0x3313E8` point that slot
  to RVA `0xA4760`, a getter of bit 6 in actor flags at offset `0x840`.
  Vtable references in construction paths and the resource-starting method at
  slot offset `0x298` distinguish these actor tables from other pointer runs.
  The flags bit's producer and observable meaning were not established.
- Selectors `0x28` and `0x2B` instead use the callback/result record accessor
  RVA `0x18CEC0` through the table at RVA `0x62D10`: `0x28` reads the first
  record dword at RVA `0x62BB1`; `0x2B` reads the record word at offset 8 at RVA
  `0x62BF7`. The record accessor chooses the current result from the callback
  object. This is distinct from the actor flag getter above.
- Selector `0x38`, RVA `0x62A65..0x62A91`, obtains the callback's attacker actor
  through RVA `0x18C940`, obtains its telemetry and identity word, and returns
  true when the ServerID high-byte mask `0xFF000000` is clear. The full packet
  translation into that callback object was not reconstructed.

For horizonxi-2023, corresponding opcode dispatch is RVA `0x573E0` with table
RVA `0x5D04C`. Opcode `0x03` is RVA `0x5889D`; opcode `0x09` is RVA `0x58C1F`,
with swapped-role execution at RVA `0x58CEC..0x58CFF`; opcode `0x2B` is RVA
`0x587E4`, forwarding to RVA `0x1899D0`, then result processor RVA `0x9D490`.
The same actor-getter selector `0x3B` is at RVA `0x61D8D..0x61DC9`, and the
same identity classifier selector `0x38` is at RVA `0x61E95..0x61EC1`.
The original selector paths were independently decoded on this build; the
full death transition and result-field wire mapping remain unverified here too.

Every binary address above applies only to the explicitly identified build.
No absence claim about the whole retail binary is inferred from one bounded
call chain or from missing community reconstruction.
