# Successful-hit offhand context, 2026-10-04

## Supported rule

In the inspected ordinary attack-resource selection path, caster selector
`0x3B` classifies the selected attack in the context of the caster's offhand
appearance model. It is not the result information field, the defeated flag,
or a death-status predicate.

The classification becomes true when both conditions hold:

1. The caster's normal appearance setup classified its offhand look model as
   qualifying for the alternate weapon context described below.
2. The actual resolved attack routine's authored name begins with `b`.

The inspected selection path makes the classification false otherwise,
including when the requested resource does not resolve. This checks the
resolved routine, not just the requested animation number.

For a player-style attack resource, result kind 1 and result animation 1
request the `b` attack variant. The server names ordinary animation 1
`LEFTATTACK`. Other ordinary selected attack variants do not automatically
qualify merely because the actor carries two weapons. The evidence therefore
ties selector `0x3B` to an offhand/left-swing context, with the precise resolved
resource and appearance predicates above, rather than to the attack's damage
or defeated flags.

When the clean global `dam0` successful-result arm evaluates this classifier,
true selects victim-side `damh` and false selects victim-side `damg`.
Both selected resources contain a flinch stage, as established previously.
The caster supplying this classifier is the attacker inherited by `dam0`;
the victim supplies the selected reaction resource.

## Offhand appearance classification is build-scoped

For the normal appearance-model setup inspected here, the input is the
offhand graphical look entry: slot 7, with its low twelve bits identifying the
model within that slot's model table. The setup validates the model against
the available model table; an out-of-range model becomes model 0.

A mapped offhand model qualifies unless its low-twelve-bit model number lies
in one of these inclusive excluded ranges:

| Client build | Excluded offhand model ranges |
| --- | --- |
| `horizonxi-2023` | 0-63, 117-143, 471-511, 640-703 |
| `retail-2026-09` | 0-63, 117-143, 471-511, 640-703, 1180-1195 |

In particular, the ordinary no-offhand model 0 does not qualify. The newer
build adds one excluded range. This record does not assign item classes or
names to every model in these ranges, or replace appearance-model input with
an item ID or a job's ability to dual wield.

## Applicability and limits

The producer, resolved-name predicate, and build-specific offhand ranges were
independently inspected in both measured clients. The normal appearance path
is the scope of the range rule. Missing model tables, appearance loading
races, special actor setup paths, and other attack-resource lookup branches
were not exhaustively specified.

This establishes a meaningful authored attack context for selector `0x3B`.
It does not specify retail's private state layout or require reproducing its
storage. It also does not prove that every NPC action passes through the
same appearance setup, or that every possible caller updates this context at
the same boundary. No live visual result was captured.

No death linkage was established in this producer. This record supplies no
new claim about dead-status latching or first visible collapse timing.

## Provenance

Freshly hash-checked installed DLLs and previously prepared independent,
bounded POL1 text dumps:

- `retail-2026-09`, patch `30260904_1`:
  `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`,
  SHA-256 `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
  `/private/tmp/emission-retail.text.bin` SHA-256
  `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
- `horizonxi-2023`, patch `30230905_0`:
  `/Users/jon/Library/Application Support/kuluu/installs/hxi/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`,
  SHA-256 `f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
  `/private/tmp/emission-hxi.text.bin` SHA-256
  `f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`.

Both image bases are VA `0x10000000`; dump text begins at RVA `0x1000`.
Capstone 5.0.7 decoded confirmed instruction boundaries and call sites.
Installed binaries were not modified or executed.

For retail-2026-09, selector `0x3B` at RVA `0x6295D..0x62999` obtains the
caster and invokes vtable slot `0x254`. Actor vtable RVA `0x32D710` maps
that slot to getter RVA `0xA4760`, which returns bit 6 of actor flags at
offset `0x840`. The preceding vtable slot `0x250` maps to setter RVA
`0xA4770`. A bounded search of indirect calls to that slot found the producer
branches at RVAs `0xD6905` and `0xD691E`; the setter also exists in other
actor tables, so no whole-program absence claim is made.

The producer function RVA `0xD6850..0xD692E` selects an attack resource
through RVA `0xD6940`, then tests actor mode byte offset `0x8B5` and the
resolved resource's name byte at offset `0x20` against `0x62` (`b`). The
true branch supplies 1 to the setter; all remaining branches supply 0.
Its direct callers include scheduler opcode `0x24`, RVA `0x5A95E`, and
opcode `0x5D`, RVA `0x5AA0F`, through calls at RVAs `0x5A99C` and `0x5AA55`;
an actor-level entry at RVA `0xD67B0` also calls it at RVA `0xD67FA`.
The opcode `0x5D` path requests an `at`-style resource variant. The bounded
resource-selector path at RVA `0xD69B1..0xD6A35` uses current-result kind
word offset 4 equal to 1 and animation word offset 6 equal to 1 to request
first-character `b`, then performs actual resource lookup. Current-result
wire mappings are pinned by `2026-10-04-melee-recoil-and-result-fields.md`.

Retail normal model setup at RVA `0xD3C15..0xD3D32` reads appearance index
7 through getter RVA `0x84480`, validates the model against its race/slot
model groups, then initializes mode byte `0x8B5` true and clears it for the
five inclusive range pairs. The range literals are initialized at RVAs
`0xD3CB2..0xD3CF1`, including final pair `0x49C, 0x4AB`; the loop uses ten
endpoints. Getter RVA `0x84480` obtains the indexed look value from an actor
override or telemetry array beginning at offset `0xFC`.

For horizonxi-2023, getter RVA `0xA3B60` and setter RVA `0xA3B70` use the
same flag. Their paired actor table entries are at RVAs `0x3297E8` and
`0x3297EC`, corresponding to slots `0x250` and `0x254` from table base
RVA `0x329598`. Producer RVA `0xD5730..0xD580E` performs the same
mode/resolved-name test and calls those slots at RVAs `0xD57E5` and
`0xD57FE`. Resource selection at RVA `0xD5891..0xD5919` corroborates
kind 1 / animation 1 selecting the `b` request. Appearance classification
at RVA `0xD2BEF..0xD2C64` has four range pairs and uses eight endpoints.
Independent bounded instruction assertions confirmed each build's name
predicate and range endpoint count.

The clean global DAT 0 `ROM/0/0.DAT` was freshly hashed:
`5b2ac1bc3efbb73a3c9ffbb884ed6b7066355910a91635bfeef14bd027c6c158`.
Its successful `dam0` selector-`0x3B` branch is pinned by
`2026-10-04-melee-damage-dispatch.md`. No DAT or earlier record was altered.

XIClient `World/Actor/LookSlot.h` and `SkeletalMeshActor.cpp` supplied the
offhand hypothesis. Its older four-range table was not substituted for the
newer retail table. The independent binary inspection above settled both.
LSB `vendor/server/src/common/mmo.h` (`look_t`) and
`vendor/server/src/map/packets/char_update.cpp` corroborate graphical slot 7
as `look.sub`, serialized with slot prefix `0x7000`.
`vendor/server/src/map/attack.h` names animation 1 `LEFTATTACK`; `attack.cpp`
uses it for the ordinary left attack, with separate kick/throw variants.

Raw reading stopped when this record was prepared. No production code,
shared index, tracking, services, or account data were changed.
