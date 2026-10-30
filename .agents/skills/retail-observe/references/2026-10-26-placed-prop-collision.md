# Placed props as collision candidates: registration, pose records and what a prop test consumes, 2026-10-26

Record of how objects placed by zone data (planters, fences, crates - "props") enter a mover's
collision world on the October build: who registers them onto the mover-side candidate chain, what a
placement record contains, where prototype geometry comes from, and which parts of the prop narrow
phase are not yet established. Companion to
[2026-10-25-collision-query.md](2026-10-25-collision-query.md), which owns the query engine, the mover
probe, staging cadence and the triangle decode; sections there are cross-referenced rather than
repeated.

Spec sections are in interop terms; all addresses, function names and instruction-level detail are in
the Provenance section at the end (per repo convention).

## Candidate chain: what registration actually does on the mover side

- **Every mover carries one embedded collision shape block**, built by a single constructor run from
  every mover mode installer. The constructor zeroes the block's flags word, writes a float of plus two
  point zero into an extent slot, and empties the candidate-chain anchor; see "Step reach and actor
  shape" in the companion record for the full construction law. That anchor is the head of a singly
  linked chain of candidate nodes walked by next links.
- **Registration appends or reuses one node per mover through two patterns seen at four sites.** Two
  registration units call an alloc-or-append helper on the shape block and then initialize the returned
  node with an instance argument (allocate-node, stamp-instance). Two others first read the anchor: if
  it already holds a node they reuse it, otherwise they append one first - same helper, same
  initialization. All four are scene-side registration units driven at zone load / object spawn; none
  sits on the mover's frame path (cadence law and its closure are in the companion record).
- **Two of the sites additionally stamp a tagged requester descriptor** next to the node append: they
  resolve the instance's collision handle through the manager singleton, then push four words - the
  resolved handle object, zero, a small integer tag, and minus one (at one site) - into a helper that
  builds the descriptor while bumping a global request counter. The tags seen are **forty-two** at one
  unit and **sixty-nine** at another. What the descriptors feed downstream is not established; an
  earlier note's claimed code pair "zero-twenty-nine / zero-thirty-ten style codes" is **retracted
  except forty-two, which is confirmed**; sixty-nine replaces its second member as what was actually
  seen. The helper also derives a filter word from bit seven of the instance object's status field -
  recorded because it gates something downstream, not claimed as fully decoded.

## Placement record layout (what each candidate carries)

Each placement record is **twenty-six bytes**: one prototype-id halfword, then three single-precision
rotation angles, then a translation triple of three floats, packed with no alignment padding. This
matches the container law recorded in the companion record: a per-block count word, an array at stride
thirty of the first array's header family, and immediately after it the placement array itself at
stride twenty-six, addressed index-times-twenty-six from its base, with one-past-end derived from the
record-count word.

- **Prototype id**: sign-extended halfword. Accessors multiply it by sixty-four and add a container-owned
  table pointer: prototypes are **fixed sixty-four-byte entries** indexed directly by id. Index words at
  or above one hundred twenty-eight are rejected outright (both the mover-side contact leaf and the
  accessor path enforce this cap), so any single block places at most one hundred twenty-seven candidates
  before lookups start answering "no record".
- **Three rotation angles**: fed into sine/cosine composition helpers that multiply out a full transform;
  authored as three independent angles, not quaternions. The composed matrix is a plain row-major affine -
  orthonormal rows times an axis triple plus a translation column, no perspective part.
- **Translation triple**: copied through shared vector helpers to the descriptor's translation slots; at
  staging time each record additionally receives the requester's own translation triple added in (see
  "When each path fires" in the companion record - the per-frame staging walk offsets posed records by
  the actor's anchor while tests themselves re-run only on dirty).

## Prototype descriptors and instance pipeline

- Prototypes are read as a global table of fixed sixty-four-byte entries, byte-indexed with the low three
  bits law (a prototype byte masked to seven against the global table) inside the heavy instance-pipeline
  branch; see "Down from the entry methods" in the companion record. Which DAT container retail's pipeline
  walks its prototype table out of is **not established on the generated-C side**; the geometry question it
  stands for was answered data-side by kuluu's own loader over real zone files (see "Where prop faces
  physically ship (answered from kuluu's loader)" below).

- The descriptor that records pour into is at least sixty-four bytes of float slots with a documented
  default pattern (three dword positions seeded to one, every other dword through its head region zero;
  setters write rotation in place and copy the translation triple to three consecutive slots).

## Where prop faces physically ship (answered from kuluu's loader)

Kuluu parses the October install's zone MZB chunks directly, and they carry placed collision geometry
inline in the **same MZB as the zone**, which answers the container question for implementations:

- Each MZB collision-object record names its mesh by a **geometry offset inside the same plain chunk**
  (kuluu reads each placement's faces at that offset with a direct mesh parse), and carries its own full
  four-by-four transform - pose is authored per placed object, no prototype id needed to fetch it.
- The zone's collision-object array is **index-parallel to the render-placement table**: collision object k
  belongs to drawn model placement k. Drawn props and their faces pair by position in the two tables,
  which is the data-side twin of "registration appends the instance onto the mover side".
- Placements that share one mesh offset share one submesh (deduped at load), so sharing-by-geometry -
  what retail's prototype id expresses - shows up here as repeated offsets.
- Proof in play: kuluu bakes those posed placements once per zone load into the very geometry the player
  grounds on, and an installed transport ferry's own walkable deck (a placed prop) comes from its placement
  geometry inside the same file - pinned by `installed_ferry_has_a_walkable_ship_and_separate_backdrop`
  (`kuluu-render/src/transport.rs`).

## Prop contact: what is established and what is not (R4)

- **The mover-side contact answer never touches prop geometry directly.** It selects a single candidate by
  list position, requires that node's readiness bit plus handle resolution, then null-tests an indexed slot
  in the staged hit bank at stride twenty-six (full law in "Fall and landing (mover-probe re-read)" of the
  companion record). Staged slots, not raw prototype bytes, are what contact is answered from.
- **The engine query's own candidate loop dispatches one virtual per candidate object** - a per-shape test
  slot thirty-two bytes into each shape class table - driven by a walker over the chain that passes (query
  descriptor, two stack outputs, owner). That is the narrow phase for anything attached as a shape.
- **What the narrow-phase pipeline consumes** when it runs on staged geometry: sixty-byte-wide vertex slots
  holding two three-float vectors twelve bytes apart, face index groups ten bytes wide indexing those slots
  under kuluu's wide mask, with the bit-fifteen fetch-shape selector, fixed-step segment march at a
  millionths comparison, and interpolated hit points written to the caller buffer - all established end to
  end in "Down from the entry methods" of the companion record. Prop poses enter this by the staging walk:
  each registered candidate's staged records receive the requester's translation triple per frame; rotation
  composition happens through the angle helpers above.
- **Named gap (deliberate, not a runtime-pointer excuse):** which shape classes implement the per-shape test
  slot for prop geometries specifically - box versus sphere versus mesh bodies of individual props - has not
  been enumerated class-by-class on this build. The one enumerated implementation is the manager-resolved
  **zone-entry collision class** (its table, installers and triangle-walk chain are recorded in the companion
  record). W2 must either enumerate prop-class implementations before implementing per-shape tests, or drive
  all props through the staged-pipeline path that this record shows the mover actually consumes.
- **W2 decision (closed as written):** kuluu takes the second option. Every placed prop answers through the
  one unified triangle walk - posed faces baked from the placement's own inline mesh into the zone's merged
  set - which is exactly what the enumerated zone-entry implementation feeds, so no per-shape class table was
  built and none of the unenumerated classes were invented. Blocking on a rotated authored pose, standing on
  a posed crate top, and walking past a planter are pinned by `rotated_fence_blocks_on_its_rotated_face`,
  `crate_top_is_standable_at_its_posed_height` and `planter_blocks_walk_move`
  (`kuluu/src/view_native/walker/step.rs`). The class-by-class enumeration stays open as retail evidence
  work; it gates nothing in kuluu.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| each mover owns a candidate chain anchored in its embedded shape block; registration appends/reuses nodes at zone load and spawn, never on the frame path | kuluu-render/src/dat_mzb.rs MzbCollisionBlock | missing: kuluu has one merged geometry per zone block with no per-mover candidate chains to register into (this record) |
| placement records are twenty-six bytes: prototype id halfword, three authored rotation angles, translation triple | kuluu-render/src/dat_mzb.rs load_mzb_placed reading ffxi-dat/src/mzb.rs MzbPlacement into MzbInstance; the collision-object array is index-parallel to parse_mmb_placements | matches in the law (re-scored W2 2026-10-25): kuluu reads the same compact per-instance pose records - geometry offset plus authored transform, paired with drawn placements by position - and feeds them to collision once; differs only in timing: baked at zone load, not composed per candidate query (this record) |
| prototypes are fixed sixty-four-byte entries indexed by id times sixty-four from a container-owned pointer, ids capped below one hundred twenty-eight | kuluu-render/src/dat_mzb.rs load_mzb_placed dedupes placements sharing a mesh offset into one submesh | missing as a table: kuluu keeps no shared prototype index or id cap; geometry sharing happens by identical offsets instead (this record) |
| pose applied as row-major affine composed at staging/query time from angle triples via sine/cosine helpers, requester translation added to staged slots each frame while tests re-run on dirty only | kuluu-render/src/dat_mzb.rs build_collision_geometry (posed bake), pinned by rotated_fence_blocks_on_its_rotated_face and crate_top_is_standable_at_its_posed_height in kuluu/src/view_native/walker/step.rs | matches in the transform law; differs in timing: kuluu bakes every posed instance once at load rather than composing staged slots per query (this record) |
| mover-side contact answered from a single position-selected node's staged hit slot gated by readiness bit and manager handle resolution | kuluu/src/view_native/walker/step.rs step_vertical answering through contact_floor over the merged baked set | differs: kuluu answers support/contact directly from merged zone geometry with no candidate nodes, handles or staged slots (this record) |
| per-candidate narrow-phase virtual exists on every shape class; its prop-specific implementations are not yet enumerated, the zone-entry implementation is | kuluu/src/view_native/walker/step.rs (unified triangle walk over posed baked props); obstacles.rs DoorObstacle remains for authored door-state boxes only | differs in mechanism, closed by the sanctioned path (W2/W3 2026-10-25): kuluu drives every prop through the one merged triangle walk - what the enumerated zone-entry class feeds - rather than per-shape polymorphic tests; pinned by planter_blocks_walk_move, rotated_fence_blocks_on_its_rotated_face and crate_top_is_standable_at_its_posed_height (this record) |
| requester descriptors tagged with small integer codes (forty-two and sixty-nine observed) built from resolved handles with a status-bit-derived filter word | none | missing, semantics unestablished: kuluu has no counterpart and the descriptor consumers are not yet read; do not invent one for W2 without a read (this record) |

## Provenance

Method: generated C recompile of `FFXiMain.dll`, build 2026-10-05, image base `0x10000000`; looked up per
[vendor/rea/INSTALL.md](../../../../vendor/rea/INSTALL.md). Sept-build RVAs quoted from notes were re-used
only where re-read here by behavior.

Shape block and anchor: constructor `f_1002b4b0` (vptr final `0x1032a75c` @`0x1002b4ee`, flags zeroed
@`0x1002b4f4`, extent seed `[+0x40]=0x40000000` @`0x1002b4f7`, anchor `[+0x44]` zeroed @`0x1002b4fe`;
sub-constructors `f_10033710`/`f_1001d170`/`f_10025bf0`/`f_10025260`/`f_10038a20`); installer sites
`0x100c52ab`, `0x100c556b`, `0x100c56eb`, `0x100c588b`, `0x100c59ba`, `0x100c5d3e` (this = mover + 0x674).
List-head accessor `f_1002b5a0` (`lea eax,[ecx+0x44]`). Alloc-or-append helper `f_1002b6a0(this=shape
block)` returns the node pointer.

Registration windows (four sites): in `f_100cf210`: append @`0x100cf24b`, null-early-exit test
`0x100cf252..54`, instance stamp `f_10029e50(node, [esp+0xc])` @`0x100cf261`, handle resolve via
`f_10071e90` @`0x100cf26c` on `[[arg]]`, then requester push sequence - `push -1` @`0x100cf27c`, `push 0`
@`0x100cf27e`, **`push 0x2a`** @`0x100cf280`, handle @`0x100cf282`, descriptor this=`lea ecx,[esp+0x20]`
@`0x100cf283`, call `f_100742b0` @`0x100cf287`. In `f_100cf750`: anchor read via `f_1002b5a0`
@`0x100cf78d`, reuse-if-nonnull branch (`test ebp,ebp; jne` @`0x100cf794..96`), else append @`0x100cf79a`,
stamp `f_10029e50` @`0x100cf7b0`. In `f_100d0b50`: same ensure pattern (anchor read @`0x100d0bf5`, append
@`0x100d0c02`), mover-side handler `f_100cfa50(mover, node)` @`0x100d0c13`, then requester pushes `push 0`
@`0x100d0c1a`, **`push 0x45`** @`0x100d0c1c`. In `f_100cbde0`: staging/registration calls into
`f_1002b6e0` with shape-block bases at sites `0x100ccdc1`, `0x100ccdf5` (as recorded in the companion
record's contact-probe subsection). Requester-descriptor helper `f_100742b0`: bumps global counter
`[0x1047d130]` @`0x100742bb..c1`, stores args into `[esi]/[+8]/[+0xc](=arg2&arg3)/[+0x10]`, derives the
filter word from `[[obj+0x24] & 0x7f]` via dec/neg/sbb @`0x100742e2..ef` into `[esi+0x18]`, then
`f_10074380`/`f_100743d0` pair @`0x100742f6/0x10074301`. Codes `0x29`: searched in these bodies, not found
- claim retracted as recorded above.

Placement records and accessors: container chain, stride-thirty header law, record array element =
`A + 4 + 26*i`, one-past-end via `[A+0]`, index cap (`>= 0x80` → zero) with sites, prototype id scale `shl
6` against `[owner+0x14]` via accessor `f_1002a9e0`, angle composition helpers `f_10027b80`/`f_10027bd0`/
`f_10027c20` into the 4x4 multiply `f_10027d10`, descriptor ctor/reset `f_10027990`/`f_100279b0`, and the
mover-side leaf chain (`f_1002a9b0` → `f_100351f0` → `f_10035210`, readiness via `f_10071060`, manager
singleton `[0x1047d128]`, hit bank `+0x32` halfword / stride `+26` slots from `+0x34`) are sited in the
companion record's sections "Placement record layout (what the accessors read)" and "Gates, contact
selection, lean ladder and cadence (mover-probe re-read)" - identical functions verified consistent across
the two passes.

Narrow-phase pipeline sites: per-candidate virtual slot +0x20 walk `f_1002a700` (descriptor, two stack
outputs, owner) as recorded in the movement map's section 3b and the companion record; staged-slot/ten-byte
face/bit-fifteen selector/millionths march details under "Down from the entry methods: gating, pose
pipeline and segment march" (same file). Zone-entry class table `0x1032a430` installed by `f_10025010`
(inside registration path `f_10029f70`) and `f_100250c0`; its chain resolves through manager `[0x1047d128]`
via `f_10072fb0`, then `f_100224d0` → `f_10023290` → {`f_10023700`, `f_10023bd0`→`f_10024020`} plus
`f_10024900` → {`f_10024b60`, `f_10024c50`, `f_10024e30`}.

Data-side evidence (not generated C): `kuluu-render/src/dat_mzb.rs load_mzb_placed` reading each
placement's mesh via `ffxi_dat::mzb::parse_mesh_at` at the placement record's geometry offset, and the
collision-object array index-parallel to `parse_mmb_placements` (`ffxi-dat/src/mzb.rs`, record comments on
MzbPlacement::object_index), over real October-install zone files; ferry proof test
`installed_ferry_has_a_walkable_ship_and_separate_backdrop` (kuluu-render/src/transport.rs).

Not established (named gaps): prop-class implementations of the +0x20 slot beyond the zone-entry class;
on the generated-C side which container retail walks its sixty-four-byte prototype table out of (the
geometry-shipping question is answered data-side as recorded above); consumers and meaning of the tagged
requester descriptors; whether a prop's staged slots are ever built without the requester-offset walk
(i.e., static props at authored pose with no mover translation) - the pipeline shows the offset walk, not
its absence. No runtime-state pointer was treated as an acceptable stop except the manager-returned
hit-bank allocations, whose layout is read from code above.
