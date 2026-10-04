# Melee recoil and result fields, 2026-10-04

## Scope

This bounded inspection supplements `2026-10-04-melee-damage-dispatch.md`.
It establishes authored reaction selection and the wire inputs used by the
inspected result selectors and recoil handlers. It does not establish the full
ordinary-melee knockback invocation chain or killing-blow transition timing.
No original-client window or server was launched.

## Authored sway is a separate resolution arm

The clean global `dam0` routine selects victim-side `sway` when result selector
`0x28` equals 1. Its successful-result arm instead tests that selector against
0 and selects victim-side `damh` or `damg`, as recorded previously. These are
separate conditional arms. The authored successful-result arm does not add a
`sway` call because knockback is nonzero.

For the inspected Savanna Rarab model, `sway` immediately calls `vswy`.
`vswy` contains control stages and three authored sound stages; it does not
contain a flinch or knockback stage. Thus that resource's name alone is not
evidence for applying knockback movement or a second hit flinch.

This is a claim about the inspected authored routes. It is not an exhaustive
claim that no other runtime path can start another routine.

## Result information and recoil inputs

Selector `0x28` reads the packet result's three-bit resolution field.
Selector `0x2B` reads the five-bit result information field, immediately after
the two-bit kind and twelve-bit animation fields. It does not read the
following thirty-one-bit modifier field. Consequently `crtl`'s mask-2 test
examines bit 1 of result information. LSB names that bit `CriticalHit` and
bit 0 `Defeated`; those names describe the server's wire contract.

The following five-bit scale field contains two separate inputs: low two bits
for hit distortion/recoil, upper three bits for knockback. The inspected
caster flinch stage, opcode `0x21`, uses only the low two bits. Zero recoil
suppresses that flinch in the inspected actor motion path. The ordinary
successful reaction resources contain that flinch stage.

The inspected target knockback stage, opcode `0x5E`, can use the upper three
bits to start target displacement and directional recoil motion. With its
authored mode selecting the packet magnitude, zero knockback returns without
starting that movement. Other authored modes can supply their own magnitude.
This does not establish that ordinary melee automatically invokes that stage.

These result-field mappings and the separation of recoil from knockback were
independently checked in both measured client builds named in Provenance.
The authored Rarab resource inspection used the clean retail install.

## Unresolved invocation and timing boundaries

The bounded inspection did not find a decisive ordinary-melee producer of
the target knockback stage. The callback's directly inspected entry forwards
to result processing and marks that result processed. The clean `dada`
sequence supplies `crtl`, the callback, and `dam0`, and the successful
`dam0` route supplies flinch. A claim that the callback additionally starts
`sway` for nonzero knockback is not established by this evidence.

The actor-state input for caster selector `0x3B` remains unclassified. Its
getter was confirmed previously, but its producer has not been tied to a
wire input or observable action. The exact `0x2B` mapping above does not
justify assigning that mapping to `0x3B`.

The inspected death-related status-to-routine path can select a `dead`
resource, and the actor update path changes status and invokes status motion.
The chain from a defeated action result or a received entity-status update to
that transition was not resolved. Therefore packet receipt, authored damage
callback, and first visible collapse cannot be assigned one common boundary
from this inspection. Both the dead-status latch and collapse-start timing
remain unverified.

## Provenance

Installed DLLs and existing independent POL1 text dumps were hash-checked at
the start of this pass against the measured client rows:

- `retail-2026-09`, patch `30260904_1`, DLL path
  `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`;
  SHA-256 `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
  `/private/tmp/emission-retail.text.bin` SHA-256
  `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
- `horizonxi-2023`, patch `30230905_0`, DLL path
  `/Users/jon/Library/Application Support/kuluu/installs/hxi/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`;
  SHA-256 `f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
  `/private/tmp/emission-hxi.text.bin` SHA-256
  `f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`.

Both use image base VA `0x10000000`, text RVA `0x1000`. Capstone 5.0.7 was
used for ephemeral inspection; installed binaries were neither altered nor
executed. The dumps originated in the earlier independent bounded unpack in
this same evidence session and matched both known text hashes.

The clean global `ROM/0/0.DAT` SHA-256 is
`5b2ac1bc3efbb73a3c9ffbb884ed6b7066355910a91635bfeef14bd027c6c158`.
`dam0` begins at file offset `0x7F900`; the condition preceding `sway` at
`0x7FE98` contains field-selector words `0x0003001C, 0x28`, constant words
`0x0001001C, 1`, and equality operation `0x0C`. The following target-side
routine name is `sway`. The trusted prebuilt `dat-routine-stages` example
corroborated this route and the local `sway`, `vswy`, and `damg` stages.
DAT 1569 resolves to `ROM/4/109.DAT`, SHA-256
`2096f24330520fb5c33527eaf38ab092e11041e4623c48f11b78a28d21599cf9`.

For retail-2026-09, packet result decoding at RVA `0x18CA40`, specifically
`0x18CB1A..0x18CB78`, reads widths 3, 2, 12, 5, 5, 17, 10, 31 in order.
RVA `0x18D1D0..0x18D22B` copies each decoded result unchanged into the final
result record after its actor/identity header. Current-result accessor RVA
`0x18CEC0..0x18CEE0` returns that copied result; selectors at RVA `0x62BB1`
and `0x62BF7` read its first dword and word at offset 8 respectively.
Bitreader RVA `0x18C570..0x18C5B7` consumes least-significant bits first.
Thus the two sequential scale subfields match the low-two/upper-three split.

Flinch handler RVA `0x5A6B6..0x5A777` obtains recoil through helper RVA
`0x5E350`, which masks result word offset `0xA` with 3. Motion path RVA
`0xCDD60..0xCDE60` rejects zero recoil, then selects directional hit motion.
Knockback helper RVA `0x5E360` shifts the same word right by 2. Target
knockback handler RVA `0x5AB63` uses it at RVA `0x5AC14`; zero returns, and
nonzero reaches target movement RVA `0xAA8E0` and directional motion.
Related opcode `0xBF`, RVA `0x5ACA3`, also uses that helper; its complete
producer chain was not reconstructed.

For horizonxi-2023, independently decoded result input RVA `0x189C00`,
result-copy RVA `0x18A390..0x18A3EB`, and accessor RVA `0x18A080` preserve
the same mapping. Recoil/knockback helpers RVAs `0x5D780` and `0x5D790`
perform the same mask/shift. Target knockback handler RVA `0x59F93` uses
the latter at RVA `0x5A044`, with the same zero check and target-movement path.

The retail callback RVA `0x18C810..0x18C86C` forwards to RVA `0x9E090` and
marks its result processed. Result processor RVA `0x9E090..0x9E36C` contains
further effect/message calls; none was used to infer absence of indirect
death or movement effects. The actor status-motion dispatcher RVA `0x8C6C0`
contains the dead-resource selection at RVA `0x8CADA..0x8CB5A`, using the
table at RVA `0x32D5F0`. Its calls at RVAs `0x8C547` and `0x8EC50` were
boundedly inspected. RVA `0x8EC39..0x8EC65` changes actor telemetry status
and calls initialization/status motion, but the wire-result/status producer
and visible timing were not traced. These latter partial paths were inspected
only in retail-2026-09 and are not asserted as a cross-build timing rule.

LSB wire names were corroborated in
`vendor/server/src/map/packets/s2c/0x028_battle2.cpp`,
`vendor/server/src/map/action/action.h`, `enums/action/info.h`, and
`vendor/server/src/common/utils.cpp` (`packBitsBE`). XIClient's
`World/Actor/ActorTelemetry.cpp` was used only to locate the status-motion
hypothesis; its death-action table is incomplete and supplied no decisive
timing evidence. No proposed Rust implementation was used as an oracle.

Raw binary inspection stopped when this record was prepared. Implementation
must use the spec sections independently; unresolved chains remain unresolved.
