# Generator emission count, 2026-10-04

## Scope

Independent inspection of the installed `retail-2026-09` and
`horizonxi-2023` clients establishes the ordinary generator batch-count
interpretation below. This is static binary evidence about emission attempts,
not a live screenshot measurement of visible particles. Allocation failure,
missing resources, culling, timing, and special generator modes can change what
actually appears on screen.

## Ordinary batch count

The lower nine bits of the generator emission flags encode an inclusive batch
limit, rather than the number of elements in an ordinary batch. For a generator
with nonzero authored particle lifetime, with the continuous-singleton flag
clear, the special batched-element flag clear, and density reduction inactive,
one eligible emission makes `raw_limit + 1` element-creation attempts.

| Raw limit | Attempts in one ordinary batch |
| ---: | ---: |
| 0 | 1 |
| 1 | 2 |
| 3 | 4 |
| 511 | 512 |

The generator body's emission flags are the existing four-byte DAT field at
body offset `0x68`, excluding the sixteen-byte resource chunk header. Only the
lower nine bits participate in this ordinary count calculation. The higher
mode bits must retain their independent interpretation.

For the density-reduced ordinary path, retail first scales the raw limit,
truncates the nonnegative result, and then adds one. If `d` denotes the effective
density factor actually used on that path, the attempts are
`trunc(raw_limit * d) + 1`. At an effective factor of 0.3, raw limits 0, 1, 3,
and 4 therefore produce 1, 1, 1, and 2 attempts. Applying a minimum of one to
the truncated product gives a different result for raw limit 4.

This observation identifies the count arithmetic on the reduced path. It does
not establish a complete portable classification of every resource eligible
for density reduction, the meaning of every graphics configuration setting,
or that an arbitrary caller-provided density factor represents retail policy.

## Periodic and immediate linked emission

Particle initializer opcode `0x3C` resolves another generator, resets its
emission countdown, and updates that linked generator with the newborn parent
particle as context. It reaches the same emission decision and ordinary batch
count calculation as a periodic generator update. It does not reinterpret the
linked generator's raw count as an exact cardinality and does not add a separate
count adjustment because it is linked.

Consequently an ordinary linked generator with raw limit 0, 1, or 3 also makes
1, 2, or 4 attempts per eligible batch. The linked generator's own mode flags,
authored lifetime, eligibility, and density classification remain relevant.
The number of due batches in one update is a timing question distinct from the
number of elements per batch; the native update can process more than one due
batch. This record does not specify a hitch policy for an independent client.

## Special modes and limits

A generator with the continuous-singleton flag set, or with authored particle
lifetime zero, takes a separate single-element path. A generator with the
special batched-element flag set also makes one element-creation attempt for
each eligible emission; that element can itself represent multiple particles.
Neither path uses the ordinary raw-limit-plus-one loop.

The immediate link invokes these same branches when its linked resource has
those modes. The ordinary rule must therefore not be used as a universal count
law for every generator.

No original-client window was driven for this investigation. This evidence
settles the ordinary count computation and the immediate-link call path on the
two identified builds; it does not prove presentation, parent transforms,
linked-resource lifetime ownership, packet delivery, or complete effect parity.

## Provenance

Install discovery used the trusted checkout's existing `target/release/kuluu
install list` and `install path retail` commands. Inputs were:

- `retail-2026-09`, patch `30260904_1`:
  `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`.
  DLL SHA-256 `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
  Fresh unpacked `.text` SHA-256
  `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
- `horizonxi-2023`, patch `30230905_0`:
  `/Users/jon/Library/Application Support/kuluu/installs/hxi/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`.
  DLL SHA-256 `f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c`.
  Fresh unpacked `.text` SHA-256
  `f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`.

Both PE images have image base VA `0x10000000` and `.text` RVA `0x1000`.
The actual section virtual sizes bound independent POL1 decoding:
`0x3275EE` for retail-2026-09 and `0x3230BE` for horizonxi-2023.
The decoder rejects invalid backward references and output exceeding the
section size. The output hashes match `KNOWN_CLIENTS` and the existing trusted
analysis dumps. Fresh temporary outputs are
`/private/tmp/emission-retail.text.bin` and
`/private/tmp/emission-hxi.text.bin`. No installed DLL was changed or executed.
Capstone 5.0.7 decoded the independently produced raw text.

The community search map was
`research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp`,
particularly `Idle`, `IsNever`, `GetLife`, and `ConstructFromData`. Its source
was used to locate hypotheses, not as the decisive count evidence. The existing
DAT parser's `GEN_FLAGS_OFFSET` / `PARTICLE_COUNT_MASK` mapping identifies the
portable authored field.

For retail-2026-09, `Idle` starts at RVA `0x496F0`. The ordinary count path is
RVA `0x4A750..0x4A7A4`: it starts the counter at zero, masks generator flags at
object offset `0xD8` with `0x1FF`, converts the limit, and exits only when the
counter is greater than the limit. Each permitted iteration calls element
creation at RVA `0x4E8D0`, then increments the counter. The conversion helper at
RVA `0x311C2C` explicitly selects truncation toward zero. The reduced branch
at RVA `0x4A75B..0x4A778` multiplies the raw limit by the scalar returned by RVA
`0x2D80`, then by the `.rdata` float at RVA `0x32B15C`, independently read as
`0.30000001192092896`, before the same conversion and inclusive comparison.

The singleton predicate at RVA `0x539E0` tests flags bit `0x400`, otherwise
calls RVA `0x493B0` to read the signed initializer lifetime at offset `0x22` and
test zero. The one-element branch in `Idle` is RVA `0x4A60E..0x4A628`.
Flags bit `0x20000000` selects the separate single-call branch at RVA
`0x4A6BE..0x4A6D5`.

Element creation dispatches initializer opcode-minus-one through the table at
RVA `0x5247C`; table entry for opcode `0x3C` contains VA `0x1004EE0E`.
The handler at RVA `0x4EE0E..0x4EE45` resolves the generator through the
initializer reference, clears its countdown at object offset `0xCC`, and
calls the same `Idle` RVA `0x496F0` with the parent element. The periodic
emission branch calls the same element-creation function.

For horizonxi-2023, the corresponding `Idle` starts at RVA `0x48B20`;
its ordinary count path is RVA `0x49B80..0x49BD4`, with the same nine-bit mask,
zero-start counter, truncation and greater-than exit. Its conversion helper is
RVA `0x30D85C`. The singleton branch is RVA `0x49A3E..0x49A58` and special
batched-element branch RVA `0x49AEE..0x49B05`; element creation is RVA `0x4DD00`.
The initializer dispatch table at RVA `0x518A0` contains the opcode `0x3C`
handler VA `0x1004E232` in its opcode-minus-one entry. That handler at RVA
`0x4E232..0x4E269` clears the same countdown and calls `Idle` RVA `0x48B20`.

Addresses apply only to the explicitly identified build. The agreement of
these two builds does not establish the rule for an uninspected client row.
