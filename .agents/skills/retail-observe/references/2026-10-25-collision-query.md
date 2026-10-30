# Collision: the movement-time query interface, its candidate walk and record layouts, 2026-10-25

What the retail client actually does when movement asks "can I be here". Read from the installed
client's whole `FFXiMain.dll` recompiled to C (KNOWN_CLIENTS row `retail-2026-10`, DLL build
2026-10-05). No live client session was run; nothing here came from the screen.

Headline: movement-time collision is a **candidate-instance query**. A receiver supplies its own pose
and a caller-supplied result buffer; an engine walks a list of nearby candidate shapes, composes each
candidate's pose from a compact placement record, and hands it to that shape class's own test method.
There is no column ray over terrain triangles anywhere on this path, which is why kuluu's zone-triangle
ground law cannot be checked against these anchors - see the comparison table below.

## Query interface (inputs and outputs)

The query handler belongs to the controllable-object interface (seven classes in the image share it),
so actors, scene entries and the camera unit all expose it at the same place:

- Inputs taken from the receiver: its shape-and-query sub-object; a world translation triple read from
  three adjacent floats of the owning actor; an identity rotation seed; one mode bit inside the
  sub-object's flags word.
- Input from the caller: a result buffer, sized 44 bytes at the wrapper that movement uses, plus a mode
  selector and an extra argument passed through untouched to the engine.
- Output: the handler fills the caller's result vector - three components, horizontal pair in slots one
  and three, vertical element in slot two - then **accumulates into that second slot**: it picks a fixed
  vertical penalty of minus 1.3 world units (or minus 0.5 when an alternative gate-and-posture chain passes
  instead; **zero** unless one of the two chains does), multiplies it by one positive lean/posture float
  chosen from three candidates in priority order - first the interface accessor every consumer uses, then
  two actor-local posture floats when that accessor is not positive - and adds the product into whatever
  the engine left in that vertical element. It returns whether the engine accepted the query at all.
- The wrapper movement calls reduces the buffer to a single length: 0.8 times the length built from
  slot one and slot three of the buffer - the horizontal pair only, skipping the accumulated vertical
  element. That is the "room I have" margin used to shorten steps.

## Candidate walk

The engine keeps candidates in a singly linked list rooted in the shape sub-object (each node holds the
candidate object then its next link). Two entry shapes exist: mode two addresses one candidate by
walking the chain, scales the result by that candidate's own per-axis scale triple and merges it into
the caller with element-wise vector add; the other mode requires a bit of the shape's flags word to be
set, walks the chain for validity, and forwards the caller's arguments to the batch test path. Nothing
in the walk reads geometry - candidates are selected by presence in the list only.

Before any candidate is used, three readiness conditions must hold, all cheap bookkeeping:

- its registration handle points at an entry inside the manager's fixed window (a 256-KiB span), and
  that entry is not one of the manager's own linked-node slots;
- the handle resolves to a live object;
- a pending-state word, read from the last node of the handle's ring, reads zero.

The getter used throughout returns the live object pointer stored at the head of a registration slot,
or null - it contains no arithmetic at all, which is why these helpers carry no geometry.

## Placement record layout (what the accessors read)

Candidates are described by a **26-byte placement record**; the field widths below come straight from
the reads:

| bytes | width | meaning |
| --- | --- | --- |
| 0-1 | u16 id word (sign-extended when used as an index) | prototype selector. It scales by 64 and adds a table pointer kept by the owner, so **prototypes are 64-byte entries** indexed by this word; an accessor exists that only returns it, and refuses indices at or above 128 |
| 2-5, 6-9, 10-13 | three floats | rotation angles. Each is fed to a setter that takes the sine and cosine of its argument and composes a matrix - so pose is authored as three angles, not as a quaternion or matrix |
| 14-25 | three floats | translation triple. Copied into the query descriptor as a vector, and merged into results by the element-wise vector helper |

The records live in an array whose header word (the record count) is the first word of its block, right
after a **second** array inside the same container: 30-byte entries beginning immediately after the
container's own 52-byte head, with their count kept as a word at byte 50 of the container. Element and
one-past-end are both computed from that header - so the record count is authoritative, never a capacity.

The query descriptor the records pour into is at least 64 bytes: three float slots default to one, every
other dword through its 60th byte starts at zero; rotation setters overwrite it in place and a translation
setter copies a triple near its end.

## How candidates reach a shape, and what is dispatched per candidate

A second pass followed the dispatch chain to its end. Structure, established instruction by instruction:

- Candidates are **registered scene instances**, not freshly built query objects. Scene registration code
  resolves an instance through the same manager that serves motion names (the registry whose entries also
  back animation lookups like idle/walk/run), asks it for the instance's own array of shape entries, and
  pushes each resolved entry onto the receiver's candidate list with a plain append - one append helper,
  four registration call sites in the scene module.
- The engine walks that list per query. For **each** candidate it calls one wrapper, which does *not* test
  anything itself: it walks the candidate's own attached-shape list and asks each attached shape to handle
  the query through a method in that class's dispatch table (the ninth entry), passing the composed query
  descriptor, two stack outputs, and the owner.
- So there are two levels of list walking - candidates, then shapes attached to a candidate - and one
  dispatch point per shape. The wrapper and the walk contain no arithmetic at all.

**What is *not* behind that dispatch (correcting an earlier reading).** Enumerating every class dispatch
receptacle in the image's read-only data section whose ninth entry does floating-point work yields only
thirteen tables out of the whole DLL, and opening each one shows animation-rate interpolation and scene or
walker update code - never a plane/edge/radius test. Notably, for the controllable-object interface family
itself that same dispatch position is occupied by the walker driver, which confirms the slot is a generic
"handle this per frame / per query" entry rather than a collision test. The earlier statement in this record
that the narrow phase sits behind that ninth entry was wrong and is retracted.

**Where the arithmetic actually lives.** Floating-point code reachable from these anchors is a small set of
shared vector helpers - normalize-a-vector-and-return-its-length (with near-zero guarded by dividing by one),
element-wise add, scale, matrix copy and multiply, plus two distance helpers used by both the camera's pull-in
and scene code. Nothing else in this band does math at all. The shape *contents* themselves - vertex arrays,
plane equations, box or capsule extents - are reached through pointers stored per instance by the code that
reads the DAT blocks; those pointers are runtime state, so following them statically stops here exactly as
the lookup procedure prescribes. The prototype lookup (an id word scaled to 64-byte entries against a table
pointer kept by the owner) has **one** consumer in the entire image: a getter that returns "the prototype of
list element number N". Prototype tables are therefore data handed around, and the test code consumes them via
per-instance pointers rather than through any dispatch table.

What this means for reading retail collision further: the next step is not more dispatch-table work but the
data side - find the loaders that fill each registered instance's geometry pointers (the DAT block readers),
because only they reveal the shape kinds and their extents.

### Two levels below every shape test, and a whole-image signature scan

The reasonable objection to the census above is that a shape-test entry could be a thin dispatcher with the
real geometry work one or two calls deeper, so every entry named in the census was followed through two
levels of direct calls before being ruled out. Two levels down, the arithmetic that exists is unit-vector,
walker-integration and vector add/scale/normalize work - the same handful of helpers the query wrapper and
the camera already use. None of those closures reaches code that walks a vertex or index array. The
dispatcher-over-triangle-test hypothesis is therefore not supported on this build.

A second, independent test looked at the image from the other side: every function in it was scored for the
shape of a triangle traversal (indexed sixteen-bit loads feeding floating-point arithmetic), and exactly one
function matched. It is not geometry. Its callers pass pointers into an error catalogue that sits next to it
in read-only data, and that catalogue is the embedded PNG/zlib reader's own message table, including lines
like "Width too large for libpng to process image data." and "Missing PLTE before IDAT"; the matching
function itself picks one of three bit masks from a format byte and multiplies it into a word-sized count,
which is what an image transform stage does. So the single triangle-shaped signature in the whole client
belongs to the image decoder.

**Retraction.** The conclusion of that paragraph - no client-side traversal of zone triangles anywhere in this
build - is wrong, and both tests were the reason. Each asked what geometry *code* looks like; neither asked for
the numbers only one data format in this game uses. Searching the recompiled image for the constants kuluu's own
reader has to use found the traversal immediately, so the negative result was an artefact of the instrument.

### The same decode, found through kuluu's own constants

Kuluu reads a zone's collision triangles out of its model data with two index masks (an index word whose high
bit is kept clear at 32767, and one narrower mask at 16383), eight bytes per triangle record, each index
doubled-and-added-to-itself to step through vertex records twelve bytes wide. Searching the whole recompiled
image for that arithmetic returned twenty-four functions doing at least two masked fetches of it, in two
families.

The collision-band family is not a signature match but a wired one. A proxy class installs its own entry table;
two adjacent methods of that class read the collision manager, ask it to resolve an entry for a handle (the same
manager and resolver already named above), and hand the resolved object straight on. Only **one** of the pair
tests anything: it reaches the masked triangle fetches through three helpers. The other is not a test path at
all - when its requester-bit and pending-byte handshake allows, it walks its instance table and translates
every pose entry by the query issuer's translation triple, an offset builder over sixty-byte staging records;
an earlier pass called it "a sibling variant of the same shape", which understated that difference. A further
helper fans out to three near-identical siblings that materialise posed faces from ten-byte record groups
rather than testing them. So zone triangles *are* walked in this build behind a manager-resolved entry reached
through the candidate pair - but not from the mover's own per-frame query; see "When each path fires" below.

What is established: the fetch law (index words masked as kuluu masks them; the test consumes staged vertex
slots sixty bytes wide, and the sibling pass materialises posed faces from ten-byte record groups), that it
sits behind a manager-resolved entry in the collision band, and that it shares the engine's vector
add/scale/normalize primitives with everything else already recorded. Which callers drive the pair was open at
that pass and is closed below. Terrain height therefore stays unclaimed.

### Down from the entry methods: gating, pose pipeline and segment march

The follow-down read of both entry methods answers what their arguments and records are consumed as (one open
question above - plane equations versus bare vertices - is now partly answered; see the march below).

Gating. Both entry methods check a *requester* status word's loaded bit before doing geometry work, and the
collision object carries its own pending byte: when it is set, the first method runs the full instance pipeline
below and clears the byte on completion; the second method's rebuild - the pose-offset staging pass named in
"The same decode" above, not a test - runs only while that byte is clear. So this pair fires on load and dirty
transitions through the loaded-bit/pending-byte handshake, not as a continuous per-frame cost; which callers
hit the entry methods at all was left open there and is closed in "When each path fires" below.

Instance pipeline. The first method's heavy branch walks three parallel arrays: prototype-table references read
as bytes masked to seven bits against a global table of fixed 64-byte entries (the same cap the layout section
records), a vertex-record array stepped twelve bytes wide, and per-instance pose records sixty bytes wide. For
each instance the pipeline computes its transform: three orthonormal rows times an axis triple plus a translation
column - a plain row-major affine with no perspective part - through one shared helper, and stages transformed
axis slots on the stack for the tests below.

Narrow phase (segment march). The test below the candidate chain marches a query segment by subtracting a fixed
step from a remaining-length float each lap against a comparison at about one millionth. What it consumes is
*staged* geometry, not raw model data: vertex positions sit in sixty-byte-wide slots holding two three-float
vectors twelve bytes apart, and the faces stepping across them are ten-byte groups of index halfwords that
index into those slots under kuluu's wide mask (an earlier pass described these fetches as reading following
twelve-byte vertex records; a direct re-read shows sixty bytes per index - see Provenance). Per face it passes
the first vector of one indexed slot plus, from the second indexed slot, *both* vectors when bit fifteen of
that fetched index halfword is clear, and only its first vector - duplicated into both parameter positions so
the second contributes nothing - when that bit is set. Degenerate candidates are rejected by sign tests on the
fetched index halfwords. On hit the point is written to the caller's result buffer by interpolation along the segment direction
(each output axis = accumulated base plus direction component times remaining step), and the lap loop continues
while length remains - so a long query accumulates hit points across multiple fixed steps rather than solving one
time-of-impact analytically. Plane equations as stored data: still not seen; what was seen is per-edge fetch and
fixed-step intersection arithmetic.

### When each path fires

CORRECTED 2026-10-25 (contact-probe pass): this section's headline - that neither walker sits on the mover's
query path and that therefore no per-frame mover query samples zone triangles - is withdrawn. The mover object
carries one virtual method which is its own contact-query body, and running it walks the registered candidate
list through the same per-shape dispatch whose terrain branch reads triangle records. See "Terrain height and
gravity, per frame (contact-probe pass)" below for what this pass established and for the cadence sub-question
still open. The rest of this section is kept as history with its wrong parts marked.

- WITHDRAWN - **The mover probe is answered from authored placement records only.** The two dispatch modes this
  pass described are real, but the conclusion drawn from them does not hold. The mover-side contact test walks
  every registered candidate and calls a fixed method per candidate shape, and that dispatch's terrain branch
  steps twelve-byte vertex slots selected by index masks over eight-byte triangle records - zone geometry, not
  placement records. So a walking actor's per-frame query does sample zone triangles, and kuluu's triangle-sampled
  ground law is checkable against retail at these anchors.
- WITHDRAWN in part, corrected by "Fall and landing (mover-probe re-read)" below - **The contact test is
  all-candidates-until-one-passes.** The list head inside the mover object, the finite bounded lookup, and the
  zero-answer-arms-the-fall-step law all stand; what does not stand is "walks every entry and dispatches one test
  method per attached shape": the walk advances to the single node at the caller's query-word position and runs a
  fixed readiness-plus-handle-resolution leaf on it, with no per-shape virtual dispatch in that body. The full
  corrected law is in the re-read section.
- **The test pair (narrow-phase pipeline) is reached from scene-registration functions.** That much stands -
  registration units hand resolved instances to their receivers (append site quoted under Provenance), and one
  wrapper in the family stages a fresh query descriptor, walks the receiver's candidate list once and dispatches
  each entry's test method, twice per call through thin wrappers. WITHDRAWN is the cadence word built on it:
  "zone triangles are walked at load, on dirty transitions and on scene re-registration, not continuously".
  The mover-side contact test above walks candidates and dispatches the same per-shape method every frame for a
  moving actor, whatever these two entry methods do.
- **Cadence sub-question now open.** The pending byte one entry method tests - and clears immediately after its
  walk - is written from inside the same family: candidate-registration constructors and the staging pass. This
  pass saw no store to it from the mover's commit unit. So this record neither restates "dirty transitions only"
  nor asserts per-frame-from-commit; which registrations mark a requester pending while it moves is unresolved.
- **The staging pair (pose-offset builder) fires from scene-update code.** One driver is the controllable-object
  interface table's entry that the movement map names as the walker/scene driver - reached through its
  tail-call wrapper installed in that same table - and one further caller of the walk is reachable only via
  indirect dispatch. That walk translates each registered candidate's staged records by the requester's
  translation triple (two vector adds per sixty-byte record), so staging offsets follow actors between events
  while the tests themselves re-run on dirty. A further pass read what that walk carries for each class
  registered in it: at the collision instance, the walked slot is pose-offset staging end to end - its whole
  backend body is a scratch mirror plus one gated call into the stager, with no edge into any test - and at the
  scene-inline member registered beside it, the same slot resolves from that class's table to a shared
  do-nothing stub. Nothing on this per-frame walk measures anything.

### Terrain height, per frame (position quad and its commit)
Pressing the writer side of the walker's output pointer located the mover-side per-frame law for actor position,
height included. It is arithmetic on a pair of quads inside each mover object - three coordinates plus a fourth
word that defaults to one - and no geometry sampling enters it:

- **The position lives in two adjacent quads**, an anchor block and a working block (offsets under Provenance).
  The per-frame update unit seeds a stack temporary with that quad from one of three sources - the anchor block
  when idle, the working block itself while movement is engaged, or the four-word block behind the same position
  slot accessor the camera reads through - then updates the temp with exactly one mode handler and commits it
  into the working block field by field. The commit is a single shared setter function; every routine write to a
  mover's coordinates in the client goes through this one function (twenty-five enclosing call sites listed under
  Provenance), and no path examined derives any component from zone geometry.
- QUALIFIED - **Walking never changes height.** True as stated for the input integration: its displacement
  vertical is written zero twice on every walk branch. It does not mean a moving actor's height cannot change:
  the contact probe (see "Terrain height and gravity, per frame") accumulates a fall step into the mover's own
  result vectors while the contact test reads nothing, and scene drivers carry measurement into the state block.
  The input-driven integration builds its horizontal pair from heading and
  step; the vertical slot of that displacement triple is hard-written zero twice on every walk branch - once when
  locals initialize and once immediately before the integration call chain (sites under Provenance) - so local
  input cannot alter an actor's height. Scripted relocation handlers fill all three components themselves;
  remotely driven entities instead consume per-axis accumulator words added into the temp under a delta-timed
  smoothing gate, fed from outside this unit.
- **A scene-side mirror is copied right after the commit.** The mover update slot calls the update unit and then
  copies working-block coordinates into an adjacent transform-ready quad, setting its fourth component to one -
  what scene composition consumes. Several driver classes repeat that mirror copy for their own receivers (sites
  under Provenance). What writes that mirror later is open: accessors sharing the same offset word also operate on
  it as bit flags, so the mirror quad's reader-side role stays unresolved even though its write sites are pinned.
- **A position-set unit moves all blocks at once.** One external-vector setter writes scratch, working and anchor
  quads together through the commit function - the teleport/spawn/wire path (one caller; chain under Provenance).
  The accumulator words that feed remotely driven entities were not named this pass (a broad offset census is
  ambiguous across classes); wire-fed coordinate updates are the obvious feeder, and they reach the position only
  through the commit setter either way.
- WITHDRAWN - Consequence for kuluu: the claim that retail's client-side height law is "quad arithmetic only"
  and that kuluu's triangle-sampled ground law has no retail counterpart on movement anchors is withdrawn. The
  mover owns a per-frame contact probe over its candidate list whose shapes' terrain branch reads triangle
  records, it accumulates a fixed vertical penalty times a lean float into the result vector's second component
  when nothing is contacted, and scene drivers fold measurement results back into the state block that the commit
  reads. Quad arithmetic describes the storage hops; it does not describe how the stored value gets made.
The upper census band named in this section keeps its reading: the install's own hook labels resolve it to the
zone-draw path (scene zone draw begin/end, map-cull returns, model culling; Provenance), i.e. rendering-side
consumption of raw twelve-byte-step vertices - not a collision loader (no allocator or resource-manager use
anywhere in its members) and not a height query.

### The position command and its feeder blocks (wire-side pass, correcting the previous section)

The previous section concluded from mover-object bookkeeping that retail has no client-side ground law. That
conclusion was drawn while enumerating writers of one commit setter only and is withdrawn as too strong: the
position a local player reports to the server does not live in those mover fields at all, and this pass followed
the command itself to find where it lives.

- **One encoder builds the position-and-state command.** Across the whole image exactly one routine allocates
  that command id, and it is called from the per-frame network tick of the local player's controller class (call
  chain under Provenance). The command carries a three-component position in horizontal/vertical-horizontal/
  vertical order matching the server-side header, plus movement clock, frame number, facing byte, a packed mode
  bitfield, a facetarget word and an engine timestamp.
- **The wire triple is copied out of one four-word block in the controller object.** The encoder first copies two
  four-word blocks from behind the controller's state block into stack locals (position block, then orientation
  block), sums the three sent components and drops the command if that sum fails a magnitude sanity test, then
  writes those same three consecutive words straight into the packet. A fourth word of the position block is not
  transmitted. When the controller has no state block or the scene object reports a flag byte, an alternative
  branch instead seeds the locals from a pair of blocks held in the controller itself; that pair doubles as the
  command history, since one gated path copies each into a previous/current snapshot and another clears both.
- **Three units write the transmitted position block, all by straight dword copy.** A per-frame mirror unit walks
  the entity table and refreshes this block from an actor record's own position triple (three words) and orientation
  quad taken one field-set further on; a placement/teleport unit writes it from a scene handle object or, for another
  entity id, from that entity's table record; and a third thin-pair unit (two wrappers, three arguments) fills both
  the position block and its adjacent orientation block from an argument record. Only these three functions store
  into those words anywhere in the image; every other move of this data goes through one shared four-word copy
  helper, whose call sites carrying a state-block base are confined to exactly two sender-class units plus the
  encoder itself - listed under Provenance.
- **No computed height reaches the transmitted position quad.** No routine anywhere in the image stores a
  floating-point result (a coprocessor store) into any component of that quad; every writer moves an existing word.
  (Orientation-side components do receive computed float writes from two facing units - recorded separately, and not
  heights.) So if the client derives stair-climbing height from geometry, it does so into the *actor record* fields
  that this mirror unit reads, not here.
- **Consequence and remaining open item (honest statement).** The wire position for the local player is a mirror of
  an actor record's position triple; that record's three words are therefore where a client-side ground law would
  have to land. This pass did not name the per-frame writers of those record fields: an offset census over them is
  ambiguous across classes (the same word offsets recur as flags and unrelated data), which is exactly the kind of
  negative the previous section was built on and will not be asserted again. The next lookup is bounded and named:
  resolve the actor-record class behind the entity table, then enumerate writers of its first position triple.
- **What this pass does settle positively:** (1) the command's field layout and gating against the server-side
  header; (2) that mover anchor/working quads are NOT the wire source - they are a separate coordinate set whose
  commit setter, previous-pass analysis included, describes scene composition rather than what the player reports;
  (3) the complete writer set of the transmitted block (three units, copy-only), which bounds every later claim
  about height feeding the wire.

### Terrain height, per frame (authoritative-record writer census - closing pass)

SUPERSEDED 2026-10-25 by "Terrain height and gravity, per frame (contact-probe pass)" below. The finite copy-unit
censuses in this section stand; the end-to-end conclusions drawn from them do not.


The bounded lookup left open by the wire-side pass is done: the actor record behind the entity table was resolved
by constructor identity, and every store to that record's reported position words is enumerated. The result is
positive and finite, and it settles the height question for the reported chain:

- **Every writer of the authoritative position triple is a member of the record's own class, and not one of them
  computes a component.** Seven units in the whole image store those three words (sites and member identification
  under Provenance): an init member that zeroes the triple with the working blocks; a spawn-placement member that
  writes all three components straight from its call arguments into a fresh record before inserting it into the
  table; two promotion members that copy the record's own authored target triple over the current triple when a
  mover command lands (one unconditionally on arrival of an in-step relocation, one gated behind mover-state
  tests); a refresh member copying the controller state block's transmitted quad back into the record on command
  events; an attach-record member copying a four-word position/orientation set from the record attached to the
  actor's aux field; and the relocation-command member of the network handler. Every source is a stored word or a
  call argument - wire payload, target block, or previous state.
- WITHDRAWN as written - **Y is never replaced on any hop.** Each *copy* hop this pass enumerated does move an
  existing word, and that enumeration stands as a finite list of copy units. It cannot carry "never": it bounds
  only the units that store those words directly, not what computes the value they carry. This record now
  documents measurement (probe contact tests and lean-scaled vertical accumulation) reaching mover state blocks
  through scene drivers and an integration unit, so a computed height can be present before any of these copies
  runs. Read together with "Terrain height and gravity, per frame", which parts of the reported chain are
  pass-through and which receive computed vertical remain open questions for a follow-up pass; no end-to-end
  claim either way is supported.
  then out through the fan-out member (which copies the record triples into every mover state block: the committed
  quad, the anchor pair behind the attach record, and two script-applier blocks), to the encoder's wire copy -
  each hop is pass-through. No scene-query result, accumulator or triangle-derived value feeds any of those sites,
  so terrain height in the reported position per frame equals whatever the last authored command set.
- QUALIFIED - **The authoritative side receives no mover output on walk paths.** The scene objects carrying mover
  commits hold back-pointers to these records, their position setters copy an argument triple in and immediately
  run the promotion member, so a scene-side set is another straight-copy feed. That observation is about those
  setter units only; it does not extend to probe output, which reaches mover state blocks through other drivers
  (see the new section) rather than through these setters. The mover's own integration does stay inside its
  anchor and working quads between commanded sets - that part of the old sentence is consistent with this record.
- **The only coordinate arithmetic anywhere on these writers** sits inside the relocation-command member: six
  windows that scale an authored per-axis velocity (target-minus-current divided by a tick count, multiplied back
  through elapsed time) or apply stored offset pairs to all three components, plus door/event transitions that
  save and restore whole triples through globals with a fixed authored vertical offset applied when the step
  counter reads full. Inputs are payload words and constants; none read zone geometry.
- **Spawn coordinates arrive on the wire.** The placement member receives its triple as call arguments beside the
  record's type word, confirming entity positions enter the client as authored values from network data, exactly
  matching the server-side spawn records.
- WITHDRAWN - Consequence for kuluu: "per-frame terrain height in retail is wire-authored end to end; nothing on
  the client samples ground geometry into position" is withdrawn. The mover's per-frame contact test dispatches shape
  tests that read terrain triangle records, and its negative answer drives a lean-scaled vertical accumulation; scene
  drivers then scale a measurement global by half and add measurement into state-block vertical components. Geometry
  is sampled client-side on these anchors. Whether the local player's reported coordinates take that value every
  frame is a separate question this record does not settle (see "Open items" in the new section).
- QUALIFIED - The last unexplored stage of the height path, the mover's mode machinery, was read and closed
  "with the same answer". Its enumeration of the four modes and their feeds stands; its closing sentence that no
  candidate reads zone geometry into height does not hold at the anchors now documented (the contact probe is not
  one of those four modes and it does read them).
 Per-frame mover updates run one shared routine that seeds a working quad, dispatches on four flag-controlled modes (scripted relocation, scripted orientation, accumulator motion, script-applier steps), lets the walk integration fill the quad otherwise, then commits through the single setter; its five gating virtuals and its accumulator helper were read from all class tables this pass. Every vertical feed into that quad is a stored word or authored scalar: the accumulator triple those modes integrate receives no coprocessor store anywhere in the image (finite census, both offset print forms), and its one writer computes it from desired-minus-current position of the mover itself; the script-applier blocks interpolate between authored anchor pairs. No candidate there reads zone geometry into height either.
- One earlier note corrected: a registration unit in the mover band was recorded as linking record and handle
  objects through two reciprocal fields; on this build it computes an id-tagged offset from a counter global
  instead (sites under Provenance), so neither link of that claim reproduces as stated, and record-to-scene
  membership rests on setter back-pointers. Pressed further the same pass: mover factories do allocate a fresh
  actor record beside the mover and attach it to the free entity-table slot - but through their own object-tail
  field written separately, not through that registration unit (sites under Provenance).

### Terrain height and gravity, per frame (contact-probe pass)

This pass read a mover object's own probe method, its contact test, the shape-test terrain branch that feeds it,
and the scene drivers that consume what those probes measure. Together these establish a client-side vertical law
on movement anchors, which every earlier section of this record denied or left unclaimed.

- **The probe is one virtual method on the mover class.** A single slot in all seven mover-class interface tables
  holds it, and callers reach it only through that dispatch. Its body: copy a query vector out of a block inside
  the mover object; ask one of the engine's query units with that vector as both operand pair (a self-anchored
  probe); merge a result into the mover's own staged result area (sixty-byte records, as elsewhere in this
  record), whose first four words are this frame's answer - three float components plus a fourth word; then run
  three gate virtuals, each paired with an entity predicate, to decide whether the actor is standing on something
  or passing through.
- **The contact test walks every registered candidate.** It fetches the candidate list head from inside the mover
  block (the same block family that carries its result vectors), follows each entry's next link, and calls one
  fixed method per attached shape. Zero candidates pass is reported as zero: touching nothing. The lookup is
  bounded - an index word at or above 128 returns zero immediately.
- **The shape test reads zone geometry.** Under the dispatch sits the walk already named in this record: a segment
  march over staged records whose vertex slots are twelve bytes apart, selected by the two index masks kuluu uses,
  and inside that branch an indexed read of triangle-record halfwords with the same stride arithmetic kuluu's
  decoder applies to eight-byte triangle records. So the mover probe's contact test is a terrain-geometry test,
  not placement-record arithmetic.
- **The fall step.** When the gates say "no contact" and two entity predicates agree, this frame's vertical
  penalty is a fixed minus one point three world units; on an alternate branch (a different gate pair plus its own
  predicate) it is a fixed minus nought point five. Both are constants carried in the probe body. The penalty is
  multiplied by one positive lean/posture float taken from a ladder, and the product is *accumulated* into the
  second component of this frame's result vector (read-modify-write, so it compounds across frames).
- **The lean ladder.** The multiplier comes from a virtual getter on the actor plus two mover-side fields: if the
  getter returns zero, use one field; else if that field is zero and a second getter's argument returns nonzero,
  use a second field; otherwise re-read the first getter. Those two mover fields are written by the mover band
  from per-actor vertical measurements (one shared actor getter, plus one variant scaled by constants) and reset
  to one at mode transitions - both print forms of their offsets give finite writer sets.
- **Contact means hold; no-contact means step.** On contact, no penalty word is stored at all in this unit, so the
  vertical component keeps whatever value the query left: nothing snaps an actor down or up. Off contact, one
  constant times lean per frame accumulates downward, unclamped inside this unit - matching what an external
  memory-write probe shows (pop height up and the actor hovers instead of being snapped back while standing,
  lands only when something comes within reach while moving).
- **Two scalar wrappers read the result block's other words.** One computes a margin: two float components
  (the third and fourth of them) squared, rooted, times a constant; it never reads the accumulated second word.
  Another measures the raw length of the answer triple (all three components, vertical included). Both are called
  from movement integration.
- **A measurement global carries height information per frame.** A routine present at one fixed slot in all seven
  mover tables issues three differently-numbered queries and publishes what each returns into one scene-wide
  float: one branch stores the result block's second word as an integer, another stores it whole, a third stores it
  times one half. Two scene drivers multiply that global by the same constant wrapper uses; a driver with no such
  scale zeroes a companion word instead.
- **The global lands in state-block vertical components.** In an integration unit, after copying the committed
  position quad out of the mover's accessor into locals, that measurement global is added into one local and the
  sum written back beside it; another caller adds the same global into the vertical component of a mover state
  block (its two horizontal neighbours adjusted by the measured difference on each axis) through shared vector
  helpers. This is client-side geometry-derived height entering position storage.
- **What feeds committed Y.** The commit unit reads its four values one at a time from the mover's own position
  accessor - which returns that same state block - so committed coordinates equal the state-block quad, and this
  pass established a measurement feed into that block. No coprocessor store straight from the probe result block's
  second word into committed coordinates was found in the units read; how often (and through which helper) the
  accumulated fall reaches the state block is not fully pinned.
- **Open items, named.** (1) Which query numbers mean what: only this pass's slots are read; other classes share
  the probe dispatch but implement it differently, and their bodies were not opened. (2) The last hop from result
  block to state block on walking actors. (3) Whether the local player's reported coordinates equal that state-block
  quad every frame or only during movement engagement - which of this pass's units writes the mover's anchor block,
  and how the controller picks between them, is unresolved. This record makes no end-to-end claim in either
  direction about the wire.

## Comparison against kuluu's `dat_mzb` collision structures

| item | retail verdict against kuluu |
| --- | --- |
| Terrain as indexed triangle soup (vertex positions, per-triangle authored normals, u32 indices) | **agrees in the decode law**: retail steps eight-byte triangle records with two index masks into twelve-byte vertex slots, exactly kuluu's `ffxi-dat/src/mzb.rs` decode (`TRI_INDEX_MASK`, `TRI_FLAGGED_INDEX_MASK`); see "The same decode, found through kuluu's own constants". Kuluu merges the result per zone block (`kuluu-render/src/dat_mzb.rs MzbCollisionBlock`) where retail keeps it per registered entry |
| Walked at query time behind a manager-resolved entry rather than an instance shape list | **differs**: kuluu resolves against its own merged blocks with no handle registry step, so a retail-style "resolve then test" never happens in kuluu |
| Per-instance pose from a compact placement record (id word, three angles, translation) | **differs**: kuluu keeps one merged mesh per zone block plus separate authored placements for models (`ZoneMmbBuild`, `ZoneAreaBox`); no per-candidate pose records |
| Prototype descriptors indexed by an id word at fixed 64-byte stride | **no counterpart** in kuluu |
| Candidate selection: the contact answer picks the single candidate whose list position equals the caller's query word, requires that node's readiness method to pass and its handle to resolve in the collision manager, then answers from a staged hit slot - no per-shape virtual dispatch anywhere in it (corrected 2026-10-25; the earlier "membership walk with a per-shape test" reading of this site is withdrawn) | **differs**: kuluu filters triangles by grid cell and column (cell index in `kuluu-render/src/dat_mzb.rs MzbCollisionGeometry::nearest_wall_contact` and the column probes) rather than maintaining candidate lists or handle resolution (rescored W3 2026-10-25: the old column-walker helper named here is deleted; merged-zone querying is unchanged in substance) |
| Readiness gate (registered handle, resolved object, no pending state) before any use | **agrees in spirit** with kuluu's block-load gating (`kuluu-render/src/dat_mzb.rs LoadedZoneGeom`, `MzbCollisionGeometry` suppression), but kuluu has one gate per zone block rather than per instance |
| Rotation authored as three angles, composed by sine/cosine at query time | **differs**: kuluu's placements are already axis-aligned or quaternion-authored (`ZoneAreaBox`), no angle-triple composition |
| Query result accumulated as a fixed vertical penalty times a lean/step float, only when the contact test finds nothing | **agrees in the law (re-scored W3 2026-10-25)**: one carried penalty (`kuluu/src/view_native/walker/consts.rs FALL_STEP_FRAME_PROBE`, alternate `FALL_STEP_FRAME_ALT` as sourced data) times one lean float per movement frame with no acceleration and no terminal speed, added to feet height by the airborne branch of `walker/step.rs step_vertical`. Kuluu carries the primary gate only (its contact test is a floor probe over merged geometry, not the candidate walk) and its lean float is fixed at neutral |
| Contact test = "does any registered candidate contain me", answered zero/nonzero per frame | **differs**: kuluu asks its own merged terrain for a floor height within the follow window (kuluu/src/view_native/walker/step.rs contact_floor over `dat_mzb.rs MzbCollisionGeometry::ground_nearest`; W3-rescored - the walker no longer queries the bounded `ground_step`) rather than asking all candidates for containment and taking a boolean |
| Measurement published to one scene-wide float, scaled x0.8 by drivers for mover states 5/6 before integration adds it into state-block vertical components | **agrees in law and scale (re-scored 2026-10-26 once the driver multiplier decoded +0.8 from the image; sites under Provenance)**: `walker/mod.rs Walker::probe_measurement_y` is the field the fall branch publishes and the same value integration adds to height that tick - now carrying the driver scale (`consts::MEASUREMENT_DRIVER_SCALE`, integrated step `FALL_STEP_FRAME_INTEGRATED` = -1.04/frame); kuluu has one field per walker rather than one shared scene word, applies the x0.8 to the no-contact (fall) feed only, and keeps the contact-drag feed unscaled while the record states no penalty word enters that channel on contact; coverage of the state {5,6} gate beyond those two answers remains open |

### Step up (how an upward surface ahead is walked onto)

This pass asked specifically about stair-climbing: what makes a rise in front of a walking actor
acceptable, how high, with what test, and where the resolved height comes from. The answer on this
build is that step acceptance is expressed as a vertical rate bound, not as a maximum-rise height.

- **Nothing on the movement-time terrain path rejects a rise.** There is no slope or normal-angle test
  anywhere in the query band - no plane equation, no up-dot threshold, no tallest-climb constant. The
  candidate answer either reports contact or it does not; its vertical element carries what the engine's
  own segment march measured at that pose.
- **Height change is rate-bounded by the same two words as the fall step.** When support is missed, one
  fixed minus-one-point-three world units (alternate gate: minus nought point five) times the lean float
  steps the actor down per movement frame. On contact no penalty word is stored at all and the measured
  answer persists into the state block through the measurement feed - so ascending ground raises height by
  whatever the probe measures, folded in on the same integration path that carries the fall step. The only
  magnitudes bounding any vertical change per movement frame are these two constants times lean; a rise
  taller than one frame's measure is simply measured over more frames while contact holds.
- **Stride shortens near it instead.** The wrapper answer the walker consumes for its step margin skips the
  vertical element and scales only the horizontal pair of the answer at zero point eight - walking into a
  riser cuts the next step short, it does not gate it. (Wrapper re-read this pass: the square root over the
  two horizontal slots times one stored constant, with the vertical slot never addressed.)
- **Vertical reach up is not established as a window.** The segment march's origin and length - how far
  above and around the actor pose the engine probes - were not pinned past the fixed-step loop; no constant
  bounding how high ahead of the feet geometry may be was seen. What stands: there is no reject cap, and
  height follows measured ground at the follow-word rate bound.
- **Blocking belongs to other candidates.** Zone triangles only ever answer contact-and-measure on this
  path; what stops a walker horizontally are candidate shapes registered as blockers (entity-collision
  family), not terrain faces. A stair riser is not on any blocker list, which is how an actor can walk onto
  ground higher than itself at all.

### Fall and landing (mover-probe re-read, 2026-10-25)

This pass read the mover probe handler's whole instruction stream, resolved its gates, lean getter and entity
predicates from the image across all seven mover classes, corrected the contact-test semantics recorded earlier in
this file, and settled the staging cadence that earlier sections left open. Where this section differs from wording
in an earlier one, it supersedes it; each such point names what it withdraws.

- **Handler order, every call.** The handler takes the caller's answer buffer plus a query-id word. It copies the
  four-word query vector out of that buffer into the mover-side staged block, builds a transform descriptor from an
  anchor triple kept inside the mover with a unit seed, runs the engine query over the embedded shape block (the
  walk that reads zone triangle records), then applies a subtract helper to the answer area. It decides a frame
  vertical penalty (below) and always reaches the accumulation step before returning: whatever happens to the
  gates, this call adds lean times that penalty into the **second word** of the caller's answer buffer - adding
  zero when no branch fired.
- **The three gates are uniform across mover classes.** All seven mover tables implement the gate methods with one
  and the same function each: the first accepts a state word in one through four inclusive, the second accepts only
  exactly eighty-four, the third accepts sixty-one through eighty-one inclusive. These classify movement state,
  not geometry. When a mover's own gate declines, the handler consults an entity-kind pair instead: the first pair
  accepts only entity kind five, the second only kind eighty-five, the third a fixed list of kinds from sixty-three
  upward; with no attached entity all three pairs answer no.
- **Contact branch - corrected reading.** Earlier sections described the contact test as walking every registered
  candidate and calling one method per attached shape. That wording is withdrawn for this site: the test advances
  the mover's own candidate chain (next links) to the node whose list position equals the caller's query word, then
  runs one fixed leaf chain on that single node - a readiness method on its sub-object, handle resolution through
  the collision-manager singleton, and an indexed slot inside the returned hit bank at a stride of twenty-six bytes,
  selected by the hit record's own index halfword. The handler only tests nullness: non-null means in contact this
  frame. Candidate lookups whose index word is one hundred twenty-eight or above answer not-in-contact.
- **The penalties.** No contact and a first-or-second gate path gives a fixed minus one point three world units;
  the third-gate path (its own pair plus its own no-contact answer) gives a fixed minus nought point five; every
  other combination leaves the penalty zero. Both constants live in the handler body.
- **Lean ladder, closed.** The multiplier is read through one virtual getter: it returns the attached entity's
  posture float when present and a stored global of minus one when absent. Two mover-side fields override it per
  branch exactly as recorded before (per-actor vertical measurements; reset to positive one at mode transitions).
  For an ordinary standing-to-walking actor all terms are neutral: magnitude one, so one probe frame steps the
  vertical word by the constant itself - one point three world units down on the primary path, nought point five on
  the alternate.
- **Landing is a gate flip, not a snap.** On contact nothing at all is stored into the second word this frame: no
  unit in these anchors resets the accumulated descent to a surface height when support resumes. Descent stops
  because later probes add only zero (or whatever the engine query itself wrote), and height then follows ground via
  the measurement feed recorded above. Landing, expressed in this machine's terms, is contact test answering yes.
- **Staging cadence closed.** The pending byte that gates the staged triangle walk changes value at exactly three
  kinds of sites: candidate node construction; a reset unit reached only through two named scene-load units (one
  caller chain from each); and a setter family behind one jump-table dispatcher with no direct callers, i.e. driven
  by engine command messages. No mover or frame-loop path stores to that byte at all. So triangle staging runs at
  load/registration time and on explicit invalidation, while the contact test consumes already-staged data on every
  probe call made from the scene drivers. This closes the "cadence" word left open by earlier passes; a
  per-frame-dirty theory floated in discussion is recorded here as never established.

### Step reach and actor shape

- **One constructor owns the mover's embedded shape block**, called from every mover mode installer: it installs the
  class table, zeroes the flags word, writes a float of plus two point zero into one slot, empties the candidate
  list anchor, sets per-instance bytes and fields to zero or empty, and constructs several sub-blocks before handing
  off. That plus-two value is the only vertical extent authored on the shape block at construction.
- **Flags select query mode.** Bit sixteen of that flags word selects an alternate engine-query mode (tested directly
  inside the engine entry); it is set per-instance, not authored per zone.
- **What plus two point zero is NOT:** it is not a step-up cap. No constant bounding how high ahead of the feet
  geometry may be was found anywhere on the query or contact path (see "Step up"); its use inside the narrow phase
  remains unestablished, so an implementation must treat it as construction-seeded actor extent data - candidate
  vertical extent in the widest sense - and nothing else until a read pins narrower use.
- **The shape block also anchors the mover's candidate chain**: the same anchor field is what the contact walk above
  traverses with next links, fed by the registration appends documented earlier in this record.

### Result vector (consumer pass)

The caller's answer buffer carries four float words before its tail; consumers established for each:

- **Words one and three - horizontal pair.** Consumed together only as a length: the margin wrapper roots the sum of
  their squares, scales by nought point eight, and hands movement integration a step shortening. Nothing else reads
  them as components at these anchors.
- **Word two - vertical accumulation; moves both player and camera.** For the player it is published each frame into
  one scene-wide measurement float (an integer-rounded, a raw whole-value, and an x0.8 variant keyed by query id;
  the multiplier constant decodes +0.8 from the image - earlier passes read this as "halved") which integration
  units add into state-block vertical components before commit. A scene-driver unit re-reads that same global and
  multiplies it by the very same x0.8 constant when a mover-state getter answers 5 or 6, storing the product back
  before the add (instruction sites under Provenance; per-state coverage of that gate beyond those two answers
  unresolved) - so for movers in those states the fall step lands on height as x0.8 times the probe penalty. For the camera, a camera unit adds its
  own probe answer's second word **directly** into a local vertical accumulator that feeds aim height - same word,
  separate consumer, no intermediate global on that path.
- **Word four - no consumer found.** Neither this pass nor earlier ones located any read of the fourth float; it is
  recorded as unconsumed at these anchors (flag or padding, unestablished).
- **Query-id lexicon, non-exhaustive.** Ids five and seven drive the measurement publishers (whole-value variant on
  seven); eight, nine, ten and eleven fire from one movement-state unit as a quartet of stack answers whose floats are
  compared against stored constants before it composes step-event command words - flags and event selection, not
  position; thirteen upward come from a periodic sweep unit whose ids advance by three from thirteen per counter
  state, combined through the shared vector helper and compared to thresholds (gate flags); twenty-five computes the
  answer triple's raw length for callers that only want a distance. The camera takes its id from one byte read out
  of its own accessor and skips probing when that byte reads one hundred twenty-eight.
- **Lateral camera motion does not come from these answers.** The pull-in standoff is built from separate mover
  accessor pairs (differences of returned float pairs, scaled by stored constants, with a fixed two-point-five
  substitution when a comparison fails); the only probe answer component the camera unit consumes at all is word two.

### Face classification (flag-word bits versus ground, wall and ignore)

Asked directly: which bits of the eight-byte triangle record's flag halfwords decide standable versus
horizontal-block versus ignored, on the contact path. Answer, established end to end this pass:

- **None, on the contact path itself.** Neither the mover probe handler nor its staged-node read (position-selected
candidate readiness plus handle resolution) nor the engine-query candidate loop and narrow-phase march contains a
branch on any triangle flag bit: no test of the membership bits, no test of the validity half-bit, no normal or
slope predicate. Ground-versus-wall is **structural**: it is decided by which query mode runs against which
candidate kind (terrain entries answer standable height; separately registered candidate shapes are what stop
actors horizontally) - never by per-triangle classification at query time.
- **The membership nibble law matches kuluu's bit for bit, and it lives on the build side.** A record-building
unit in the scene band assembles the terrain nibble exactly as kuluu does: take bit fifteen of each of the four
record halfwords - first index, second index, third index, normal index, in that order - shift each out to a lone
carry and or them together so the first halfword's bit lands low and the normal-index bit high. The nibble is
stored as metadata beside the built record. Its only consumers found are scene-side: one pure getter of that
field with exactly two call sites, both in the same upper band (beside the zone-draw path), and a copy into an
output record next to other scaled per-mesh fields - fishing-probe-style exposure, not movement.
- **The validity half-bit is carried, never tested.** The flag kuluu decodes as "invalid" has no test anywhere in
the collision band or the contact path; neither does kuluu's camera-transparency bit. What retires a face from
queries at staging/march time is only the degenerate sign-test on the fetched index halfwords recorded under
"Down from the entry methods" - that, and faces never staged into the set a mode queries.
- **Consequence for implementations:** standable comes from *which authored class's set* a face was staged into,
block from the separately registered shape candidates, and ignore only via degeneracy at staging. A remake must
classify structurally (two sets), not by filtering terrain triangles on flag bits during movement queries.

### Shipped-zone step classes (2026-10-25 census)

Asked directly of four shipped zones in the local install - Northern San d'Oria, Bastok Mines, West Ronfaure and
Upper Jeuno - what step classes players actually walk, and how small solid objects are authored. The pinned
install-gated measurements live as `mzb_shipped_step_classes.rs` (integration tests of the geometry crate).

- **City staircases inside terrain chunk meshes measure rise 0.5 on run 1.0.** At Northern San d'Oria collision
  geometry x from minus-two-hundred to minus-one-hundred-ninety-two at z near minus-seventy-eight, minus-
  seventy-four-and-two-thirds and minus-eighty-one-and-a-third - three parallel flights, the stair class a stall
  report was filed against - every riser face stands exactly half a yalm tall with one-yalm treads meeting floors
  on both sides. Step-connecting vertical-face histograms over all four zones peak first under a fifth of a yalm
  (fine steps and ramps), then at about 0.5, near 1.0, and in scattered classes up to past 2 (terraces): no
  shipped zone fits its stairs inside an authored wall band of only 0.4 - the shipped city stair class exceeds it.
- **Small solid objects ship as dedicated repeatedly-placed meshes.** Railing, fence-panel, post and box-prop
  collision arrives as meshes whose local bounds sit inside roughly 1.6 yalm on every axis: a 0.2-by-1.3-by-0.2
  panel type placed two hundred twenty-four times in Northern San d'Oria, another near 1.5 tall placed eighty-
  seven times; Bastok Mines carries panels about 1.0 to 1.5 tall placed fifty-seven and forty-two times and small
  posts placed nineteen to thirty-two times. These are the bake-side counterparts of this record's separately
  registered shape candidates.
- **Terrain chunk pieces never sit inside that bound.** Every mesh carrying shipped city stairs exceeds 1.6 yalm
  along at least one horizontal axis (chunk fragments from about four-by-four past six-by-eight in footprint,
  heights from one and a half up to full storeys). Bound fit separates small solid objects from terrain across all
  four zones inspected; flat ground patches that do fit the bound carry no wall-class faces, so an identity flag
  keyed on the bound is inert for them.
- **Consequence.** With object identity separating small blockers, kuluu's wall-side query suppression sits at the
  recorded follow-word magnitude - one point three yalm, the single magnitude bounding vertical change per move-
  ment frame - so a suppressed face is exactly one the vertical machine could climb; nothing authored as an object
  under 1.75 relies on band suppression to stay solid.

### Terrace lips are authored terrain, and suppression must equal acceptance reach (Bastok Mines AH census, 2026-10-26)

A walk-off report at xz(-24.15,+59.26) in Bastok Mines asked whether the DAT holds geometry past
terrace edges or the client loads none of it. Dumped raw (install-gated, print-only,
`kuluu-render/tests/mzb_bastok_terrace_edge_dump.rs`): the answer is **the DAT has the whole edge
there** - no load hole, and the wall faces are present at every column that lost support.

- Standing on the terrace floor at +3.000 with the lip ahead: columns out to 0.7 yalm answer
  +3.000 (normal.y +1.0). Between 0.8 and 1.1 ahead a vertical ray meets only steep faces -
  normal (+0.76..+0.78, **+0.20..+0.25**, -0.59..-0.60) climbing +3.306 → +3.631 → +3.941 over
  one-third of a yalm.
- From 1.1 ahead out to ~2.3 the column holds ONE surface, up-facing at **+4.167**: that is the lip's
  top, **one point one six seven** above the feet it stands beside.
- Past it the authored faces fall away (-0.75,+0.30,+0.59) through +4.137 → +2.199 → +0.113 and
  meet the lower street floor at **-0.000** from ~3.8 ahead: a three-yalm drop on the far side.
- Inside x[-27.5..-19] z[+56..+63] the face census shows these lip faces as terrain-law faces
  (`prop=false`) spanning y[+3.000..+4.167] with normal.y +0.20/+0.22/+0.23/+0.24/+0.25 and plain
  +0.000, i.e., authored terraces and their risers - not props, and nothing that should be walked
  through.

**Consequence for a remake (measured, not inferred from retail law):** two windows in the client
must read ONE ceiling. Kuluu suppressed terrain faces whose top stood within one follow word
(1.3) of the feet as "climbable" while its support acceptance refused any rise over twice the
probe offset (1.0): every face topping between +1.0 and +1.3 above the feet was simultaneously
unstandable and non-blocking, so a walker crossed it with nothing under him - exactly this report,
and the class behind "walls can be randomly walked through if I push into them" and "stairs
randomly fall down though to abyss". Kuluu now suppresses at `STEP_UP_ACCEPT_RISE_YAMLS` in
`dat_mzb.rs`, which `walker/step.rs` re-uses for acceptance, pinned by
`terrain_faces_too_tall_to_be_walked_onto_always_block_the_body`; the shipped half-yalm stair
class (rise 0.5, measured above) sits inside it and every clean stair pin walked unchanged.

### Movement-time collision: the character sweep (single-probe law, 2026-10-26 pass)

The client resolves an actor's attempted move through one pass of its
character-collision manager. Given the current position and the intended
position, both path endpoints are offset by half a yalm along the vertical
axis before testing (and a successful result has the same half restored),
with a guard nudging the endpoint by nought-point-nought-nought-one when the
two heights are equal. The pass seeds probe padding of half a yalm in all
three axes plus the path length, and collects candidate triangles from the
zone grid across the path's vertical band extended half a yalm each way:
priority-marked sections first (a flag-bit two-pass ordering), section skip
flags honoured, objects outside the vertical band skipped, triangles with the
high corner bit of the second vertex word ignored.

Resolution marches along the path in steps of nought-point-three times the
probe padding's horizontal extent (a quarter-yalm step at the standard
half-yalm padding). At every step all candidates are tested against one
current point; corrections accumulate into a single working triple that
carries from step to step, and after the march a settle loop runs over the
full candidate set up to four more times until no correction remains. The
moving body is therefore a single point carrying a half-yalm probe sphere -
no tall chain of body spheres exists anywhere in this pass.

Per-candidate test: project the current point onto the face plane; when that
projection falls outside the triangle, fall back to per-edge probe-radius
tests over all three edges at the same squared threshold. A face whose
view-space normal points away is skipped while marching. Faces form two
classes by object-space normal vertical component: a wall band between minus
and plus nought-point-seven-oh-eight inclusive, and the steep class outside.

Wall-band handling (what stops or slides a move):
- Only the nearest candidate of the class applies per step; a running
  best-distance tracker rejects farther ones.
- Out-of-reach test: squared plane distance against the probe threshold
  reduced by nought-point-nought-nought-one.
- Height rejection: if all three vertices lie at or below the current point's
  height minus tolerance nought-point-nought-nought-two on the wire vertical
  axis (which grows down), the face is ignored - a face whose top edge does
  not reach up past the body point never blocks. Branch directions and
  tolerances are cited at their instruction sites under Provenance; kuluu
  converts once into its up-positive interior frame.
- Correction: shift the working triple from the face by sqrt(threshold minus
  squared distance) along the unit point-to-plane direction, applied in full
  to the horizontal pair and multiplied by nought-point-two on the vertical -
  wall contacts leak twenty percent of the remaining probe reach into height.

Steep-class handling: its own best-distance tracker; faces wholly above the
point (all three vertices at least nought-point-three above it on the
down-grows axis) are ignored. For downward-facing normals (world up-facing
surfaces) the working triple's vertical is set to the plane intersection's
height minus half a yalm and floor contact is recorded for the frame.

On success the caller receives the corrected end point with the half-yalm
offset restored; when corrections never settle, the actor keeps its pre-move
position for that pass.

## Kuluu delta

| rule | kuluu symbol | status |
| --- | --- | --- |
| movement-time collision runs as a candidate-instance query with a caller-supplied result buffer | kuluu/src/view_native/walker/sweep.rs sweep | differs: kuluu queries its own loaded zone geometry directly (column plus wall sweep), with no candidate list or per-shape dispatch |
| the wrapper reduces a query to one margin length at 0.8 times the buffer's two vector slots, and steps are shortened by it | kuluu/src/view_native/walker/step.rs ResultVector::margin applied inside `step` ahead of the sweep (measurement from the wall-class body probe) | matches in the wrapper rule (rescored W3 2026-10-25; the pre-retail landing-band row is superseded): the intended step is shortened by scale times the measured pair before commit; kuluu's measurement for the pair is its own body intrusion, and blocking and sliding stay owned by `walker/sweep.rs` |
| candidate pose comes from 26-byte placement records with three authored angles and a translation triple | kuluu-render/src/dat_mzb.rs MzbCollisionBlock | missing: no per-instance collision placement table exists in kuluu |
| prototypes are fixed-size entries indexed by the record's id word, capped below 128 records per block | kuluu-render/src/dat_mzb.rs MzbSubMesh | missing: kuluu keeps per-submesh data with no shared prototype index |
| candidates arrive through a registry append (scene registration pushes resolved instances onto the receiver's list; the append site, both driver wrappers and their callers are now pinned) | kuluu-render/src/dat_mmb.rs ZoneMmbSpawn | differs: kuluu spawns zone models as Bevy entities and queries geometry directly; there is no per-receiver candidate list to register into (this record) |
| one wrapper walks each candidate's attached shapes and dispatches the query per shape | kuluu/src/view_native/walker/obstacles.rs DoorObstacle | differs: kuluu's only per-obstacle handling is a hand-written door box list, not a polymorphic shape set |
| shared vector primitives (normalize with length returned, add, scale, matrix copy) back every query result merge | kuluu/src/view_native/walker/sweep.rs sweep | matches: same primitives are used inline on kuluu's slide/depen math |
| wall resolution returns a slid position plus an exit reason | kuluu/src/view_native/walker/sweep.rs SweepExit | matches |
| obstacles that also carry their own floor (doors) are sampled inside the column slab | kuluu/src/view_native/walker/obstacles.rs door_floor_at | matches: no retail counterpart established at these anchors, and no reason to expect one - keep as kuluu-side authored law |
| readiness gate: registered handle, resolved object and zero pending word before a candidate is used | kuluu-render/src/dat_mzb.rs LoadedZoneGeom, kuluu/src/view_native/collision_bvh.rs ZoneCollisionBvh::query_ready | matches: kuluu gates terrain use on block-load state per zone slot, and refuses its own zone tree while geometry changed or a rebuild is in flight - the camera boom holds last frame's standoff instead of answering from a stale tree (this record) |
| index words are masked with kuluu's mask widths, but retail's query-time test consumes *staged* vertex slots sixty bytes wide (two vectors each), not raw twelve-byte-step vertices over eight-byte triangle records | ffxi-dat/src/mzb.rs TRI_INDEX_MASK, TRI_FLAGGED_INDEX_MASK, MESH_TRIANGLE_LEN | matches on the masks; differs in consumption: kuluu tests decoded triangles directly and has no staged/posed geometry, and retail's raw-to-staged loader was not read (this record) |
| the walk is reached by resolving a handle in the collision manager, not by dispatching an instance's shape list | kuluu-render/src/dat_mzb.rs MzbCollisionGeometry::nearest_wall_contact filtered by its cell index, driven from the wall probes in kuluu/src/view_native/walker/step.rs (rescored W3: the deleted column helper formerly named here queried the same merged blocks) | differs: kuluu has no handle registry step; it filters its merged blocks by grid cell and column. Cadence re-opened 2026-10-25: the mover's own per-frame contact test dispatches the same shape-test walk, so triangle records are read for moving actors; the pending-byte word "registration-side and dirty transitions only" is withdrawn (this record, contact-probe pass) |
| the mover's own per-frame probe runs its contact test over registered candidates, whose shape tests read twelve-byte-vertex / eight-byte-triangle terrain records | kuluu-render/src/dat_mzb.rs MzbCollisionGeometry::ground_nearest sampled every tick through step_vertical's `contact_floor` in kuluu/src/view_native/walker/step.rs | matches in substance (rescored W3 2026-10-25; previous "no triangle test anywhere on the query path" withdrawn): both sample zone triangles during movement, per frame in both engines; they differ in shape of answer - retail takes contact plus accumulated vertical from all candidates, kuluu takes a floor height within reach from its merged blocks (this record, contact-probe pass) |
| actor height moves through quad arithmetic inside the mover object - one commit setter for its anchor/working quads, seed selection between them, scripted relocation, an explicit position-set unit; walking forces input displacement vertical to zero | kuluu/src/view_native/walker/step.rs samples MzbCollisionGeometry::ground_nearest through `contact_floor` every tick | differs (re-opened 2026-10-25): the storage law stands, but "no geometry-derived height enters either side" is withdrawn - a measurement feed into the state block the commit reads is documented in this record's new section, so input-vertical-zero does not mean height cannot change (this record, contact-probe pass) |
| the position reported to the server is a copy-only mirror of an actor record's position triple, refreshed each tick by one mirror unit over the entity table; nothing computes it at the mirror | kuluu/src/view_native/session mirror of walker-authoritative position into the outbound position command | matches in structure: both encode wire position from an authoritative per-actor field set. Retail's writers of that set are now enumerated - spawn-placement, relocation-command, promotion, refresh and attach members, all copy-only (this record, closing pass) |
| terrain height per frame on the reported chain is wire-authored end to end | kuluu/src/view_native/walker/step.rs ground-samples MzbCollisionGeometry::ground_nearest through `contact_floor` every tick and makes that authoritative | withdrawn as a rule (2026-10-25): client-side geometry sampling during movement is established in this record, so no end-to-end wire-authored law stands. What remains: the copy-unit census (store-only writers of the reported triple) and an open question on whether probe output reaches the reported quad every frame (this record, contact-probe pass) |
| the mover command path promotes an authored target triple over the current position triple on arrival, with per-axis velocity interpolated from payload fields across a tick budget | kuluu/src/view_native/walker/step.rs integrates motion from input and speed each frame; no wire-carried target triple exists in kuluu's position law | differs: retail's relocations are commanded and time-boxed by server data, kuluu's are locally integrated; visible only where the two disagree mid-move (this record, closing pass) |
| the mover update slot mirrors committed coordinates into an adjacent transform quad with the fourth word set to one, every frame | kuluu/src/view_native/walker/step.rs authoritative position mirrored into rendered transforms per tick | matches in effect: both copy authoritative position scene-side each frame; retail's later writers of that mirror remain open (this record) |
| handler accumulates a fixed vertical penalty (minus 1.3 or minus 0.5 world units, only on the no-contact gate branches) times one positive lean/posture float into the result vector's second component | kuluu/src/view_native/walker/consts.rs FALL_STEP_FRAME_PROBE and LEAN_DEFAULT_NEUTRAL, applied per airborne frame by kuluu/src/view_native/walker/step.rs step_vertical | matches (rescored W3 2026-10-25 as the symbols moved into one module): same law - constant penalty times lean per frame, compounding because every airborne frame re-applies it, unclamped; pinned by fall_step_is_the_recorded_frame_penalty and fall_from_a_ledge_lands_on_the_floor. Scope: kuluu models one gate pair, so FALL_STEP_FRAME_ALT is carried as sourced data with no gate selecting it. The scene driver's x0.8 rescale of the published word (decoded multiplier +0.8, gated on mover state 5/6; instruction sites under Provenance) is applied to the fall feed since the 2026-10-26 rescore: height moves -1.3 x lean x 0.8 per airborne frame (~62 yalm/s at the 60 Hz movement tick), pinned by fall_step_is_the_recorded_frame_penalty (this record, result-vector consumer pass) |
| on contact the vertical component of the probe answer is left alone - no snap up or down while standing | kuluu/src/view_native/walker/step.rs step_vertical ground pass: an equal measured floor holds exactly (`VerticalDecision::Flat`); a different one inside the follow window drags at most the rate bound, never snaps from farther (the containment window in `contact_floor` refuses it) | matches (rescored W3 2026-10-25): neither implementation snaps a supported actor's height; descent resumes the frame support is missed - pinned by slope_hold_tracks_ground_at_the_follow_rate and vertical_change_never_exceeds_one_follow_word_per_frame (this record, contact-probe pass) |
| the margin wrapper skips the accumulated vertical word of the probe answer and uses two higher components times a constant | kuluu/src/view_native/walker/step.rs ResultVector::margin with consts PROBE_MARGIN_SCALE; the measured pair is filled by `probe_answer` from the wall-class body query and shortens the step inside `step` ahead of the sweep (this record's law shared with the walker record), pinned by margin_uses_the_recorded_scale_and_skips_vertical | matches (rescored W3 2026-10-25 after the column walker was removed; its former symbol names are retired): the vertical word stays skipped by construction, and a pose whose body query sits inside geometry has its step shortened by scale times the measured pair before commit; wall blocking and sliding stay owned by sweep::sweep upstream (this record, contact-probe pass) |
| probe measurement is published to one scene-wide float and added into mover state-block vertical components by integration units | kuluu/src/view_native/walker/mod.rs Walker::probe_measurement_y, published and consumed in the same tick by every branch of `step_vertical` in kuluu/src/view_native/walker/step.rs | matches in structure (rescored W3 2026-10-25): a published measurement word feeds position integration instead of height being applied anonymously at the footfall; per-walker rather than scene-wide, and every vertical branch publishes exactly what height moved - pinned by published_measurement_is_what_height_moved_by (this record, contact-probe pass) |
| floor-vs-ceiling discrimination on the query side | kuluu-render/src/dat_mzb.rs FLOOR_NORMAL_MIN | matches: kept as kuluu-authored law - this record states no floor-versus-ceiling discrimination test on the query side (record silent), and its lean-ladder section names no height values either, so kuluu's authored-normal dot threshold stands unchallenged at these anchors |
| face classification on the contact path: none by flag bits; ground versus wall is structural (query mode against terrain entry versus registered shape candidate), ignore only via degenerate index sign-tests; membership nibble assembled at build time with kuluu's exact bit law and consumed scene-side only | ffxi-dat/src/mzb.rs MzbMesh triangle decode (TRI_INDEX_MASK, TRI_FLAGGED_INDEX_MASK, TRI_SECOND_WORD_FLAG, TRI_TERRAIN_BIT composition) feeding kuluu-render/src/dat_mzb.rs FLOOR_NORMAL_MIN filtering at query time | matches on the nibble and carry-only bits (identical assembly law re-verified against retail's build-side unit); differs in consumption: kuluu filters faces by normal dot into floor versus wall sets during movement queries, retail answers from structurally separate sets with no per-triangle test at query time (this record, face classification) |
| step acceptance is a rate bound (one-point-three units per movement frame times lean) with no maximum-rise cap and no slope test; terrain never blocks horizontally | kuluu/src/view_native/walker/step.rs step_vertical ground pass accepts measured floors inside `contact_floor`'s asymmetric window - drops to FOLLOW_WORD_YAMLS_PER_FRAME (canonically `dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME`) and rises to `dat_mzb::GROUND_RISE_BAND_YAMLS` (= 2 x `dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME`, widened from this record's character-sweep acceptance ceiling STEP_UP_ACCEPT_RISE_YAMLS on 2026-10-27: the shipped big-step class measures its biggest standable-to-standable rise at 1.64 yalm - SD north forensics profile cited in "Shipped-zone step classes" - and the old 1.0 reach answered "no contact" across gap columns, dropping walkers into void columns their landing gate could never re-acquire: the abyss reports of 2026-10-25/26) with a continuity clause on far rises: an answer beyond ordinary reach (`dat_mzb::STEP_UP_ACCEPT_RISE_YAMLS`) carries the grounded pose only when the same column also answers standable floor within that ordinary reach (`contact_floor` reading `ground_raycast_all`, normal dot `WALKABLE_FACE_UP_DOT`) - support must run up through the riser, so a lone answer high overhead never lifts a walker - bounded per frame by the rate both ways; the wall side of dat_mzb nearest_wall_contact answers from its own height law (this table's body-point rejection row plus the riser-exemption row below), replacing the 2026-10-26 shared acceptance ceiling that left a dead band of heights neither supported nor blocked (see this record's Bastok Mines AH terrace-lip census 2026-10-26) - while every other face blocks across its full height to body-sphere-chain centres that reach the feet themselves (kuluu/src/view_native/walker/sweep.rs); prop-identity faces from tri_prop_blocker answer at any height. The former authored 0.4 wall band sat below the shipped city stair class and stalled players on it | matches (rescored for the character-sweep rise-reach cap 2026-10-26, on top of the shipped-class stall fix 2026-10-25): a rise within reach is walked up at the rate bound - pinned by walks_up_the_measured_shipped_sandoria_riser (the measured rise-half-yalm class), walks_up_a_tall_riser_end_to_end, rise_above_the_kuluu_band_within_one_follow_word_is_accepted and vertical_change_never_exceeds_one_follow_word_per_frame; full-height faces block horizontally - pinned by wall_height_blocks_horizontally_through_the_authored_class; the band-versus-prop split is pinned in dat_mzb by wall_query_bands_terrain_risers_and_props_answer_at_any_height, and the probe-side climb bound (`dat_mzb::MAX_GROUND_STEP_UP`, raised from nought-point-four to nought-point-five on 2026-10-27 - the user's report that San d'Oria stairs pass at half a yalm, agreeing with the measured shipped riser class) stays pinned by ground_step_climbs_a_stair_and_falls_freely and ground_step_refuses_an_out_of_reach_floor; the shipped class itself - risers half a yalm on one-yalm treads climbing foot to crest, none above one follow word - is pinned install-gated in kuluu-render/tests/mzb_shipped_step_classes.rs by shipped_city_stairs_measure_the_pinned_flight_class and stair_riser_faces_are_terrain_law_not_props. Rise acceptance beyond ordinary reach is carried by continuity, not by a wider blanket cap: a far rise answers support only with same-column floor beneath it inside that ordinary reach - pinned by vertical_change_never_exceeds_one_follow_word_per_frame (rise exactly at ordinary reach accepted at its boundary; +1.25 over void refused), walks_up_a_half_yalm_terrain_riser_while_walking and pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops (single rises inside the band climb, ghost-free, never airborne while pressed), and by phantom_support_over_void_falls_instead_of_letting_the_walker_float_on_it and wall_holds_the_pose_while_phantom_support_looms_over_it - the shipped Bastok Mines AH terrace signature of an upper-structure plane at feet+1.167 over nothing, which no longer lifts or supports poses; past the grounded band rises are refused whatever stands below (the exemption probe's band clause in dat_mzb, `a_face_taller_than_the_assist_cap_stays_solid_under_a_long_push`) (the same reach clause also bounds step-up assist support in `standable_support_at_or_below`, and since 2026-10-26 that retry additionally refuses support more than `dat_mzb::GROUND_PROBE_OFFSET_YAMLS` below the lifted feet - a pose no longer float-hops against low props on level floor; how retail composes lift with probe resolution is not further established, and kuluu bounds it conservatively at half a yalm) |
| entry methods fire on the requester loaded bit and a pending byte cleared after the pipeline runs; that byte changes only at candidate construction, one reset unit reached through two named scene-load units, and an engine-command setter family - never from mover or frame paths (cadence closed 2026-10-25) | kuluu/src/view_native/mod.rs drain_mzb_load_state and collision_bvh.rs ZoneCollisionBvh::invalidate | matches: kuluu rebuilds the zone tree when load state changed and holds readiness off until it is back, which behaves as the same handshake at block granularity; staging cadence - load-time plus invalidation, consumption every frame - now closed on both sides (this record) |
| a probe answer's vertical word feeds the camera aim-height accumulator directly, not only through the measurement global | kuluu/src/view_native/camera_collision.rs resolve_camera | differs: kuluu's camera owns its standoff via its own raycast against the zone tree and never reads walker/probe answers for height (this record) |
| contact answer selects one candidate by list position equal to the caller's query word, gated by that node's readiness bit and a collision-manager handle resolution | kuluu/src/view_native/walker/step.rs step_vertical answering from merged zone geometry through `contact_floor` (W3-rescored away the deleted column helper) | differs: kuluu has neither per-position candidate selection nor handle resolution; it queries merged zone geometry directly (this record) |
| instance pose applied as a row-major three-by-four affine with a translation column per placement record | kuluu-render/src/dat_mzb.rs build_collision_geometry (placed instances baked per zone block) | matches in the transform law; differs in timing: kuluu bakes poses once at load rather than composing them per candidate query (this record) |
| small solid objects are separately authored blocker candidates, never terrain faces (structural two-set law; shipped data carries them as dedicated repeatedly-placed meshes - see "Shipped-zone step classes") | kuluu-render/src/dat_mzb.rs build_collision_geometry tri_prop_blocker with PROP_OBJECT_MAX_EXTENT_YAMLS, consumed by nearest_wall_contact | matches in structure (2026-10-25): faces baked from submeshes inside the bound are authored blockers answering every wall query at any height while terrain-law faces stay band-suppressed; pinned by planter_blocks_walk_move and wall_query_bands_terrain_risers_and_props_answer_at_any_height, the repeatedly-placed shipping of such meshes install-gated in kuluu-render/tests/mzb_shipped_step_classes.rs by small_solid_objects_ship_repeatedly_within_the_prop_bound; differs in mechanism: identity is a mesh-size bound at bake time, not registry membership (this record, shipped-class census) |
| segment march advances by fixed steps against a millionths comparison and writes interpolated hit points into the caller buffer | kuluu/src/view_native/walker/sweep.rs body_sweep | matches at interface: kuluu marches in radius-fraction steps then bisects, also producing fractional blocking rather than one analytic time-of-impact (this record) |
| bit fifteen of an indexed halfword reshapes what its staged slot feeds to the test: clear passes both vectors held in that slot, set passes only the first vector twice so the second contributes nothing | ffxi-dat/src/mzb.rs MzbMesh triangle decode reads those same bits as terrain-membership and validity data and always consumes every index identically | differs: a query-time fetch-shape selector on retail against pure data flags on kuluu; mechanism established by direct read of the branch, no data sweep needed (this record) |
| the moving body is one point padded by a half-yalm probe sphere, marched along the path in quarter-yalm steps with carry-over corrections and up-to-four settle retries | kuluu/src/view_native/walker/sweep.rs sweep chains three spheres of radius BODY_RADIUS nought-point-four at heights LOWER/MIDDLE/UPPER_CENTER_OFFSET (kuluu/src/view_native/walker/consts.rs) over a bisection march | differs: chain-of-spheres versus single marching point; different probe radius and no settle-retry loop (this record, character-sweep pass) |
| wall-band faces whose highest vertex sits at or below the body point (tolerance nought-point-nought-nought-two) never block | kuluu-render/src/dat_mzb.rs MzbCollisionBlock::nearest_wall_contact rejects terrain-face tops at `WALL_TOP_BODY_POINT_TOL_YAMLS` (nought-point-nought-nought-two, up-positive side of the body point since kuluu converts retail's down-grows axis once) above feet + `GROUND_PROBE_OFFSET_YAMLS` (half a yalm probe padding over the feet) + `lift_yamls`, which only a step-up assist retry raises with its own lift; prop-identity faces and door obstacles are never suppressed | matches on the rejection rule (rescored 2026-10-27, replacing the earlier authored band measured from the feet): same anchor as retail's single marching point - body point over the probe padding, lifted retries carry their lift. The shipped walkway rail whose cap tops nought-point-six-eight-seven over the walkway (Bastok Mines 334 census 2026-10-27, this record) now stops rather than being swallowed - pinned by stop_at_the_shipped_walkway_rail_and_never_mount_it and at query level in wall_query_bands_terrain_risers_and_props_answer_at_any_height; the shipped city stair class still walks up - now under this table's riser-exemption row rather than the retired shared acceptance ceiling (this record, character-sweep pass + terrace-lip census) |
| zone triangles only ever answer contact-and-measure; what stops a walker horizontally are candidate shapes registered as blockers, never terrain faces ("Blocking belongs to other candidates", this record's "Step up") | kuluu-render/src/dat_mzb.rs MzbCollisionBlock::nearest_wall_contact_riser_aware (reached from MzbCollisionGeometry::nearest_wall_contact): a non-prop terrain face steps aside when it is nearly vertical (`RISER_EXEMPT_NORMAL_MAX_ABS_Y` nought-point-two, set between the measured SD big-step facade normal y 0.00 and the Bastok walkway sawtooth caps at normal y nought-point-six-six-seven which must stay blocking), rises out of this pose's own floor level (bottom edge no higher than `STEP_UP_ACCEPT_RISE_YAMLS` plus tolerance over the feet), and standable floor answers in the column just beyond it - strictly above pose level and within the grounded rise band `GROUND_RISE_BAND_YAMLS`, probed at body radius + `RISER_PROBE_PAST_FACE_YAMLS` (nought-point-two) back along the face normal; block-only queries pass never-true so height-rejection units are unaffected | differs (kuluu-authored approximation added 2026-10-27, not retail law): retail's rule is that terrain never blocks at all and blockers come from registered candidates kuluu lacks, so kuluu can only exempt measured riser-class faces among its authored stand-in faces - big-step facades climb while rails, fences and trusses still stop (`fence_rails_stop_the_walker_and_are_never_mounted`, `a_face_taller_than_the_assist_cap_stays_solid_under_a_long_push`; same-level floor beyond a vertical face is a curb seen from its deck side and stays blocking). Pressed deep into merged multi-riser quads the exemption flickers (sd_north_big_steps battery status, forensics `sd_bigstep_forensics` in kuluu/src/view_native/walker/tests_shipped_stairs.rs); full parity needs this record's registered-candidate architecture rows (this record, "Step up") |
| only faces inside the wall band participate in a wall answer - steep-class undersides (normal vertical component at or past minus nought-point-seven-oh-eight) never correct the body | kuluu-render/src/dat_mzb.rs MzbCollisionBlock::nearest_wall_contact skips normals outside (-`FLOOR_NORMAL_MIN`, `FLOOR_NORMAL_MIN`) before any distance test, both implementations | matches (exclusion added 2026-10-27; before it a down-facing slab underside could answer the wall query and stop poses walking under slabs): pinned in wall_query_bands_terrain_risers_and_props_answer_at_any_height by the down-facing-slab assertion (this record, character-sweep pass) |
| the probe margin measures accumulated push-out that retail's candidate pass resolves to zero every frame, so a pose merely touching geometry contributes nothing | kuluu/src/view_native/walker/step.rs probe_answer floors the measured intrusion at zero after subtracting `PINCH_PENETRATION_SLOP` (shared with sweep's graze-vs-pinch discrimination) | matches in effect (kuluu-internal mechanism, 2026-10-27): kuluu's own sweep halts at the contact shell, so a grazing pose holds persistent intrusion and no candidate pass resolves it out; without the dead band every slide along a shipped wall lost ~forty-six percent of its length to margin shortening (Bastion legs 0.857/1.600 measured before the fix) - pinned by slides_along_an_angled_wall_face_while_touching_it (this report's Bug-A census; no new instruction sites read) |
| no travel-versus-normal approach gate is stated in this band: wall answers are plane distance plus face height, and a pose sliding along a face keeps its step | kuluu/src/view_native/walker/sweep.rs body_sweep tests the approach dot scaled by segment length against `INTO_FACE_APPROACH_SLOP` (nought-point-nought-nought-one, the same width as the record's out-of-reach test subtraction) instead of an unscaled sign test; accumulated pinch penetration still stops | matches in effect (kuluu-internal mechanism, 2026-10-27): the unscaled sign test made post-bisection dot noise stop slides and re-block every iteration along faces not exactly perpendicular to travel - the felt "caught on walls" report; with the scaled test only real closing motion blocks - pinned by slides_along_an_angled_wall_face_while_touching_it (legs 1.589/1.600) and slide_along_the_shipped_bastion_wall_at_minus_30 (no new instruction sites read) |
| wall-contact correction applies sqrt(threshold minus distance-squared) in full horizontally but only nought-point-two of it into the vertical axis | kuluu/src/view_native/walker/sweep.rs slides hold pose height exactly; all pose raising lives in the step-up assist of kuluu/src/view_native/walker/step.rs | missing: no wall-to-height leak anywhere in kuluu's movement resolution (this record, character-sweep pass) |
| the steep class keeps its own nearest tracker and sets the body vertical to a floor face's plane minus half a yalm (ignoring faces wholly nought-point-three above the point) | kuluu/src/view_native/walker/step.rs step_vertical answers floors once per tick through contact_floor with FOLLOW_WORD_YAMLS_PER_FRAME reach | differs in mechanism, matches in effect while standing: both land on measured planes within a short vertical window; retail resolves inside the movement pass, kuluu after it (this record, character-sweep pass) |
| nearestness among competing floors is measured to the body point (half a yalm above the feet), so when a slab floor and step-up treads share a column the tread within reach wins over the floor under it | kuluu-render/src/dat_mzb.rs ground_walkable selects against GROUND_PROBE_OFFSET_YAMLS (this change 2026-10-26; before, nearestness was measured to the feet and shipped flights with a floor slab running underneath answered the slab at every column - Garfield walked into such a flight in Bastok Mines without seeing its treads) | matches after the rescore: same comparison centre as retail's probe, prop exclusion unchanged (this record, character-sweep pass) |
| body extent during a support query: one point padded by the half-yalm probe sphere (the moving body itself in this band) | kuluu/src/view_native/walker/step.rs `contact_floor` ring block over kuluu-render/src/dat_mzb.rs MzbCollisionGeometry::column_floor_candidates, gated per candidate by dat_mzb::STEEP_SUPPORT_WHOLLY_ABOVE_IGNORE_YAMLS (nought-point-three, cited from this record's character-sweep steep-class rule) | differs in mechanism; authored stand-in added 2026-10-28 for a measured defect class: with only the infinitesimal pose column, a climbing pose drops into the sliver pockets between successive tread tops that ride on a continuing slab and falls to the buried floor - measured mid-flight at the Bastok Mines AH shell (xz(-20,+68.6), sink 0.51; xz(-21.3,+69.6) stand-point line, repeated drop-bounce-reclimb = "refuses to see the start of the stairs") and once more in SD north big-steps z-lanes (sink exactly 1.000). Four ring columns at the probe-padding radius hold the level while the axis crosses; candidates from non-pose columns are accepted only when their face does not stand wholly above feet + nought-point-three, which keeps deck tops behind walls and phantom canopies out of the answer - pinned by shell_seams_between_tread_plates_do_not_drop_the_pose (re-keyed 2026-10-28 evening, see tightening below) and ring_sample_never_holds_a_deck_wholly_above_the_pose (kuluu/src/view_native/walker/step.rs), with the untouched pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops as the ghost guard (this record, character-sweep pass). Same-day tightening after USER's "feels a bit high on the steps" and "W+D ... jitter like an update pause" reports: ring candidates are additionally capped at dat_mzb::RING_SUPPORT_HOLD_LIFT_CAP_YAMLS (nought-point-zero-six-two-five; authored, set between plane-settle noise ~1e-4 and the smallest shipped riser class ~0.23 measured in the AH terrace grid dump) above the feet - ring holds the occupied level, it never pre-lifts onto taller ground sideways - and sampling widened from four axis rays to eight so diagonals match axes (measured: a drifting diagonal could stand at a plate corner where all four axis rays miss). Measured on shipped Bastok flights before/after: pose riding 0.746 yalm above its own support straight-line, grounded one-tread sink 0.253 on the -x diagonal while straight/+x lines held; after: flap nought-point-nought-nought-four or less and no settled float past the cap on all five drive lines, pinned by approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps (kuluu/src/view_native/walker/tests_shipped_stairs.rs). Second tightening the same day after USER reported the W+D jitter reduced but not gone ("the WD jitter seems less. but not enough less. so double whatever you did"): `RING_SUPPORT_HOLD_LIFT_CAP_YAMLS` halved to nought-point-zero-three-one-two-five, and a flat-ground control measurement added - driving the real walker over shipped San d'Oria (231) and Bastok Mines (234) ground whose standable answer never moves, forty-eight drives at 0.4/0.7/1.6/2.4 yalm-per-second along three headings each, commits a single height: worst per-tick change nought-point-zero-zero-zero-one-two-two yalm, i.e. the committed pose is provably level on flat ground and any residual plate wobble lives downstream of this function (`running_on_flat_shipped_ground_commits_one_height`, kuluu/src/view_native/walker/tests_shipped_stairs.rs). Consequence for the battery: a ground-answer profile chain that does not simulate step-assist can no longer see slab-under-skin risers, so the same-day bastok_mines_ah_terrace_steps_x box was retired with its anticipation-detection recorded in place (kuluu/src/view_native/walker/tests_shipped_stairs.rs) - the class stays pinned end to end by the stand-point climb pin. Third same-day amendment after USER's stopped-float report (Bastok Mines xz(+1.15,+72.77), 2026-10-28/29: "we are still stair floating when stopped", ".26 too high (same as riser/tread, but there IS not tread under us.. and we are stopped"): the ring hold is now gated on held movement INTENT (`step_vertical`'s `intent_held`, any non-zero intent for this tick) - an idle stopped pose gets only its own axis column plus doors, so a pose that walked past a plate edge settles onto that flat instead of being re-held by body-point every tick above the plate it left (forensics at those coords: plates end ~z+72.42, axis column answers floor nought, pose floats at exactly the plate top +nought-point-two-six for 120+ ticks); held keys count as crossing even when a tick jams against geometry, because under the travelling-only gate one jammed shell-diagonal tick sank nought-point-five-one-four yalm mid-flight (battery pin `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps` caught that before shipping). Pinned by `a_stopped_pose_past_a_plate_edge_is_not_held_up_by_the_seam_ring` (kuluu/src/view_native/walker/step.rs): stopped releases to the slab within one bounded follow-word step, walking carries the hold past the plate edge while a ring column still reaches |
| the kuluu wall-sweep approximation of registered blockers exempts a near-vertical terrain face as a stair riser only when it is topped by its own floor: the standable column behind the face must answer flush with the face's top edge within one probe padding, or the face meets that floor mid-extent and is wall-with-a-ledge, solid however long the push | kuluu-render/src/dat_mzb.rs MzbCollisionBlock::nearest_wall_contact_riser_aware gated by dat_mzb::RISER_FLUSH_TOP_SLACK_YAMLS (nought-point-five = one probe padding GROUND_PROBE_OFFSET_YAMLS) | differs from no rule at all, authored 2026-10-28 at record silence after USER's "Im stuck on this rounded edge, that i should not have been able to climb in the first place" report: at Bastok Mines xz(-52.15,+19.15) the pipe side faces t24632/t24633 (authored normal (+0.98, nought, nought-point-nineteen), extent y[+nought..+two]) were being exempted because a standable cap floor at +nought-point-five answered behind them - mid-extent, one-point-four-five below the face top - and the pose then rode the cap via ring support while wedged against adjacent faces (panel: feet +nought-point-five-eight-one with the column answering nought-point-nought-nought-zero beneath it). The pre-existing bottom-edge rule alone cannot separate this from a real riser because at Bastok both measure identically; no authored bit separates them either - measured 2026-10-28 across DAT 334 (53,325 triangles) and DAT 331: the second-word nought-times-four-thousand flag is unset zone-wide in both, extending ffxi-dat's four-zone measurement. Shipped-data effect measured before/after with the exemption toggled: every CLEAN battery box unchanged (SD pin stairs, market flights, big-steps z, AH flights, shell stand-point and z boxes), one synthetic test depends on the exemption at all (pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops - its riser is flush-topped so it keeps climbing under the narrowed rule); pinned by the_bastok_pipe_edge_at_minus_52_plus_19_is_never_mounted and a_wall_whose_ledge_meets_it_mid_extent_is_never_mounted_by_pressing (kuluu/src/view_native/walker/tests_shipped_stairs.rs, walker/step.rs) |
| the one-mount ceiling (2026-10-28/29 amendment to the row above, USER law): no single mount - neither this exemption nor the step-up assist's standable support - raises the pose onto anything standing more than `MAX_GROUND_STEP_UP` plus a tenth-probe slack (`STEP_MOUNT_RISE_MAX_YAMLS`, 0.55) above that pose's own floor; a face topping higher, or a cap/floor answer higher, is wall whatever stands behind it | kuluu-render/src/dat_mzb.rs STEP_MOUNT_RISE_MAX_YAMLS (and STEP_MOUNT_TOP_SLACK_YAMLS), enforced in MzbCollisionBlock::nearest_wall_contact_riser_aware and kuluu/src/view_native/walker/step.rs standable_support_at_or_below | differs in mechanism, law set by USER (Bastok Mines barrel-cap parapet at xz(+45.18,+84.98): wall faces t5330/t5331 y[+0..+0.9] with standable cap lids +1.0 behind - normal.y 1.00, barrel and crate caps - satisfied both gates and were climbed then dropped the walker: "this wall is well above .5 so you are climbing it. then falling down because its not a surface"). Retail approximates blocking with registered candidates, not authored faces, so no retail constant stands at either bound; shipped stair flights measure <= 0.5 rise per stage (city-stair census), making the ceiling inert on real stairs. Pinned by a_standable_shelf_beyond_step_class_is_never_mounted; pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops re-keyed from climb-outcome to wall-hold under this law (the larger class was separately retracted as bridge-terrace geometry); fence_rails_stop_the_walker_and_are_never_mounted flat-cap assertion inverted to refused |
| a pose embedded in a wall escapes away from the face, never along an authored normal that happens to point into it | kuluu/src/view_native/walker/sweep.rs depenetrate (escape side chosen by measured chain distance on both sides; holds when neither measures farther) | differs in mechanism, authored: no retail counterpart (retail has no wall-embed of a chain - blockers are registered candidates and the sweep resolves to zero intrusion every frame). The blind raw-normal push was a walk-through channel in kuluu's own approximation: measured +0.15 yalm through the barrel-cap parapet face on the tick after slide contact (KULUU_SHELF_TRACE drive, same Bastok xz(+45.18,+84.98) report as the row above), landing the walker onto cap support - USER's "so you are climbing it. then falling down because its not a surface" completed this way after the mount gates alone held. Pinned end-to-end with the ceiling by a_standable_shelf_beyond_step_class_is_never_mounted |
| no slope-proportional vertical offset exists in retail's resolution: the body sits on the measured plane | kuluu/src/view_native/walker/step.rs slope_push_up applied to the grounded answer in step_vertical (SLOPE_PUSH_UP_MAX_YAMLS = nought-point-one, cap reached at SLOPE_PUSH_UP_FULL_TILT; exactly zero below SLOPE_PUSH_UP_TILT_EPSILON tilt and whenever the tick's winner is not this column's own face) | differs - authored by USER instruction 2026-10-28 ("it needs an offset based on slope to push us UP slightly and not clip into the steps while walking.. so just add the offset, not change how it works"): rides on top of the accepted support answer without touching acceptance, ranking or the fall branch; flat floors bit-identical (running_on_flat_shipped_ground_commits_one_height unchanged), pinned by slope_push_up_is_zero_on_flat_and_caps_at_the_steepest_walkable_tilt and grounded_pose_settles_on_a_sloped_face_at_the_answer_plus_the_push_up |
| while in motion the pose anticipates a forward hard edge along a crossing-linked ramp: lift = weight(d-hat) times (rise plus uplift), weight clamped nought-to-one as one minus d-hat over the window; standable support is scanned along the committed travel direction at nought-point-two-yalm strides out to nought-point-eight, the nearest riser lip wins and its distance lands half a stride back from the probe that saw it; full lift arrives exactly at the lip where the contact flip takes over, arrival and support merging (no hover, no landing overshoot). The gate is purely walk-versus-stopped: walking means this tick carries movement intent AND the previous tick actually travelled at least half its intended step - pressed against geometry is also not motion, so a blocked press never pre-climbs and rebounds; stopped ticks fall back to their own flat | kuluu/src/view_native/walker/mod.rs Walker::in_motion plus Walker::moved_last_tick (the player-state tracker) and Walker::look_latch, kuluu/src/view_native/walker/step.rs motion_lookahead_lift over the LOOKAHEAD_STRIDE_YAMLS nought-point-two to LOOKAHEAD_FAR_YAMLS nought-point-eight window with lookahead_ramp_weight, LOOKAHEAD_RISE_CAP_YAMLS (= dat_mzb::MAX_GROUND_STEP_UP; a first riser above it silences the channel entirely, leaving the assist path untouched), LOOKAHEAD_MIN_RISE_YAMLS one-sixty-fourth noise guard, RISE_UPLIFT_YAMLS nought-point-zero-five carried inside the blend, LOOK_UNFADE_PER_FRAME_YAMLS nought-point-zero-five collapse bound capped by how far the pose sits above support, STEP_MOVED_MIN_RATIO half, and a reachability gate (one extra sweep::sweep along travel per moving tick) that breaks the scan at the first blocked probe | differs in mechanism, authored by USER instruction 2026-10-28 ("it needs to fall", "YOU FALL in STOPPED.. you rise the slope in WALKING", their diagram's red continuous rise across each step): retail has no separate look-ahead channel - the effect approximates this record's padded-probe continuity (body point half a yalm over the feet marched along the path), so rises arrive early and small instead of as one contact-flip pop. Measured under the ramp law: SD axis/diagonal flap 0.05 to nought-point-zero-zero-zero-zero; Bastok shell -z flap at most nought-point-zero-one-four; the former thirty-seven float-runs of five-plus consecutive ticks became short continuous ramp profiles cresting at +nought-point-three-three mid-slope (the USER's red line); synthetic flush-riser drive lifts nought-point-four ahead of the lip, per-tick dy peaks at nought-point-one-one-three against the pre-ramp contact-flip pop of nought-point-four, and the pose lands exactly on the tread with the uplift faded; the W+D seam class stays bounded by one uplift per frame. Known watch (measured 2026-10-28): the scan has no counterpart of the body-side slab-seam continuity, so a floating-slab sliver under the San d'Oria viaduct can still mount and un-fade a single lift - sd_north_big_steps_z lane minus-nought-one-seven-nine-point-five sinks exactly one look-ahead lift at z=-168.36; seam suppression inside the scan is the named follow-up. Pinned by motion_lookahead_lift_reads_forward_columns_within_the_window_only and in_motion_lookahead_rises_before_a_flush_riser_and_stopped_walkers_fall_back (kuluu/src/view_native/walker/step.rs) and the re-scoped approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps (kuluu/src/view_native/walker/tests_shipped_stairs.rs) |

## Provenance

Method: generated C recompile of `FFXiMain.dll`, build 2026-10-05, image base `0x10000000`; looked up
per [vendor/rea/INSTALL.md](../../../../vendor/rea/INSTALL.md) as `f_<VA>` with grep over
`research/xi-on-anything/generated/all`, plus `tools/newbuild.py dis --label 2026-10-05` (used to verify
`0x10072fb0` and the call site at `0x1002aa80`, and to confirm no square-root use in that band). Full
working notes: `research/xi-on-anything/movement-map.md` sections 3 and 3b (untracked). Findings carry
provenance "generated C, build 2026-10-05".

Handler and engine:

- Query handler `[vtable+0x1c8]` = `f_100d4800`, shared by the seven interface vtables (`0x1032d710`,
  `0x1032e890`, `0x1032ecb0`, `0x1032f0d0`, `0x1032f4f0`, `0x10330f40`, `0x103313e8`). Shape sub-object
  at `this + 0x674` (block copy through `f_10026e90`; translation triple `[esi+0x698]/[0x69C]/[0x6A0]`
  pushed through `f_10026e30` with a 1.0 quaternion seed; flags byte-block at `[this+0x6A4]`, engine
  mode bit is `0x10` of the word at block plus 48). Engine thunk `f_1002c3e0` -> `f_1002c400`.
  Post-processing chain (re-read to the instruction this pass): penalty local `[esp+0x2c]` zeroed at site
  `0x100d4886`; gate virtuals `[vtable+0x34c]` (site `0x100d488e`) and `[vtable+0x358]` (site `0x100d48af`)
  with run predicate `f_10084390` feeding posture helpers `f_10084350`, `f_10084370`; a chain that passes
  reaches the engine probe `f_1002c5c0` (this = shape sub-object, `ebp = this + 0x674` from `lea` site
  `0x100d4861`; args = caller-selector spill `[esp+0x28]` and zero; probe sites `0x100d48d5`, `0x100d4912`)
  whose zero result writes the -1.3 dword `0xbfa66666` at site `0x100d48de`. Fallback chain: gate virtual
  `[vtable+0x36c]` (site `0x100d48ec`, nonzero skips via `jne` site `0x100d48f4`), posture helper
  `f_10084330`, re-probe - zero writes the -0.5 dword `0xbf000000` at site `0x100d491b`. Lean float: accessor
  `[vtable+0x110]` (call sites `0x100d4927`, `0x100d4951`, `0x100d4970`, `0x100d4984`) and actor posture
  floats `[esi+0x75c]` (sites `0x100d493a`, `0x100d4978`) and `[esi+0x754]` (site `0x100d4964`), selected by
  the compare-against-float pattern at dword `0x103295d8`, read from the image this pass as f32 +0.0; the
  comparison idiom (`fnstsw`; `test ah,5`; `jp`) jumps when greater-or-unordered, so the accessor wins while
  positive, then `[esi+0x75c]`, else `[esi+0x754]`. The chosen float times the penalty local (`fmul
  [esp+0x2c]` site `0x100d498a`) accumulates into **caller result buffer offset +4** - `fadd/fstp [edi+4]`
  sites `0x100d4992` and `0x100d4995`, where edi is ebx, the caller's buffer argument loaded at site
  `0x100d4804` (`mov ebx,[esp+0x20]`) and reassigned from shape pointer to buffer at site `0x100d4873`. The
  wrapper margin reads offsets +0 and +8 of that same buffer. Handler ends `ret 8` at site `0x100d499f`.
- Movement-side wrapper `f_100aaef0`: 0x2c-byte result buffer, dispatches `[vtable+0x1c8]`, returns f32
  `[0x10329a34]` (0.8) times the square root of `[buf+0]^2 + [buf+8]^2`. Camera-side call sites of the
  same handler family: `0x1001f344`, `0x1001f4fa`, `0x10020044`.
- Engine `f_1002c400`: mode argument compared to 2, else flag `[shape+0x30] & 0x10`; candidate list head
  from `f_1002b5a0` (`lea eax,[this+0x44]`), nodes `{dword obj, dword next}`; chain-walk sites
  `0x1002c42c..0x1002c439` and `0x1002c4d6..0x1002c4e3`; batch path forwards to `f_1002a750`; scale
  triple read from `[shape+0x28]/[0x2c]` and `[shape+0x24]` with quaternion-unit f32 `[0x1032961c]`;
  vector merge helpers `f_10026f20` (add) and `f_10027310` (scale).

Readiness:

- Predicate `f_10071060(this=S)`: manager singleton dword `[0x1047d128]`; containment predicate
  `f_100741e0(manager, *[S])` accepts pointers inside `[manager+8, manager+0x40004]` excluding the
  entry's own linked-node span; then requires `**[S]` non-null; then `f_10071f80(***[...] ) == 0`, where
  `f_10071f80(x)` reads dword `[f_10072240(x) + 0x1c]` and `f_10072240` walks the ring through node field
  `+0x28` until `[[node+0x28]] == node`.
- Getter `f_10072fb0(arg)`: returns dword at `[arg]`, or null when arg is null (image-verified: bytes
  `8b442404 85c0 7405 8b00 c20400` at `0x10072fb0`).

Placement records and accessors:

- Accessors: element `f_100351f0(x)` = record block base + 4 + 26*x (sites `0x100351fc..0x1003520a`);
  one-past-end `f_10035250` uses the count word at `[base+0]` (sites `0x1003525a..0x10035269`); block
  resolution `f_10035210(this=X)` reads `[X+4]`, requires readiness, then base Q = `*[*[X+4]]` and the
  record-block header A = Q + 0x34 + 30 * (word at `[Q+0x32]`) - sites `0x1003522a..0x10035249`.
- Consumers: `f_1002a780`, `f_1002a830`, `f_1002a8b0`, `f_1002a900`, `f_1002a9b0` (the last returns the
  id word and rejects indices at or above `0x80`). Id-word accessor with the scale-by-64 arithmetic is
  `f_1002a9e0` (`shl eax,6` over `[owner+0x14]`); batch path `f_1002a750`.
- Angle setters: `f_10027b80`, `f_10027bd0`, `f_10027c20` (each takes sine and cosine of its argument);
  translation setter `f_10027cf0` copies three dwords to `+0x30/+0x34/+0x38`; matrix multiply with the
  descriptor is `f_10027d10`. Descriptor ctor `f_10027990`, reset `f_100279b0` (dwords at `+0`,
  `+0x14`, `+0x28` set to f32 1.0; zeros through `+0x3c`).
- Per-candidate walk `f_1002a700` (see the census section below for what it dispatches to and why that is
  not a shape test).

Two-level walk, registration and the ninth-entry census:

- Engine-side wrappers over the receiver's list at `[shape+0x44]` (`f_1002b5a0` = `lea eax,[ecx+0x44]`):
  per-candidate wrapper `f_1002a700`, called only from `f_1002c270` (site `0x1002c284`) which itself walks
  the list; "prototype of element N" getter `f_1002c600` is the sole consumer of `f_1002a9e0`
  (`shl eax,6` over `[owner+0x14]`). Inside `f_1002a700`: sub-list getter `f_10029e70`
  (`lea eax,[ecx+0x20]`), then per attached shape `mov eax,[ecx]; call dword ptr [eax + 0x20]` with five
  arguments; the node-removal path uses `[obj_vtable + 0x18]` (see `f_1002b640`).
- Append helper `f_1002b630`, wrapped by `f_1002b6a0`; its callers are scene registration paths
  `f_100cbde0` (sites `0x100ccdc1`, `0x100ccdf5`), `f_100cf210` (`0x100cf24b`), `f_100cf750` (`0x100cf79a`)
  and `f_100d0b50` (`0x100d0c02`). Those paths resolve the instance through manager dword `[0x1047d128]`
  with registry codes `0x29`/`0x2a` (helpers `f_10072fb0`, `f_10072430`, `f_100742b0`, `f_100745c0`) and
  hand the resolved entry's `[entry + 0x14]` array to the list through `f_10029e50` / `f_10029f70`.
- Ninth-entry census (read-only data scan of `FFXiMain.unpacked.dll`, base `0x10000000`, sections
  `.rdata@0x10329000` and `.data@0x1034f000`; runs of eight or more dwords pointing into `.text`, matched
  against a per-function index of all 21891 recompiled functions for floating-point use): tables whose
  entry at `+0x20` touches FP are
  `0x1032a308` (entry `f_1001bc80`; installed by constructors `f_1001b82a`, `f_1005e825`; body lerps two
  frame values with the same f32 `[0x10329fd8]` = 30.0 used by the walker speed clamp, i.e. animation rate
  interpolation), `0x1032a990` (`f_10037400`), `0x1032ae64` (`f_10042020`), `0x1032b890` (`f_100604b0`, a
  505-entry table), the controllable-interface family `0x1032d710` / `0x10330f40` / `0x103313e8`
  (entry `f_100c63d0`, the walker/scene driver named in the movement map), `0x1032db40` (`f_100a4ed0`),
  `0x1032dfc8` (`f_100a57f0`), `0x1032e890` (`f_100abbd0`, no FP, helper call only) and `0x103306a0` /
  `0x10330aa0` (`f_100c47f0`). None reads vertices, planes or extents.
- Shared vector primitives: normalize-with-length `f_100274b0` (square root of the sum of squares over the
  three components, then it scales the vector in place by a reciprocal; the guard compares that length with
  zero and takes either the reciprocal of unit f32 `[0x1032961c]` or loads f32 `[0x1032a4cc]` - which constant
  is used on which side was not resolved further) with siblings `f_100275a0`, and further square-root sites at `0x100275c9`, `0x100275f6`, `0x1002760b`,
  `0x10027653`, `0x10027692`; distance helpers `f_10032da0` (callers: camera unit `f_1001ee60` at site
  `0x100204c8`, and `f_10034620`) and `f_10032e70` (callers `f_1002ac60`, `f_10034c70`); add
  `f_10026f20`, scale `f_10027310`, matrix copy/multiply `f_10026eb0` / `f_10027d10`. No square-root or
  plane-distance instruction exists in the accessor, readiness or walk functions themselves.

Depth-two closure walk (the two-level ruling-out above):

- Direct-call closures were built for every entry named in the census over an index of all 21891 recompiled
  functions. Interface-family seed `f_100c63d0`: closure 173 nodes; floating-point-bearing callees are the
  camera unit `f_1001ee60` (6 square roots, 104 arithmetic ops), `f_100cd040`, `f_100c8930`, `f_100496f0`,
  the scene driver `f_100a57f0` (whose own closure contains the walker `f_100a6240`, 7 square roots and 91
  arithmetic ops), the normalize helper `f_100274b0` and the probe wrapper `f_100aaef0`. Seed `f_100abbd0`
  closes at two nodes. Seeds `f_10037400`, `f_10042020`, `f_100604b0` and `f_1001bc80` carry at most one
  FP-bearing callee (`f_1001c260`). Indexed sixteen-bit loads: zero in every closure.
- Whole-image signature scan for a triangle traversal (functions with eight or more indexed word loads
  feeding thirty or more arithmetic ops): exactly one hit, `f_102beba8` in funcs_048.c (21 word-indexed
  loads, 47 arithmetic ops, no square root). Its only call site is `0x102c6dac`, inside `f_102c6d9f`, which
  is itself called from `f_102bd4d1` at `0x102bd4dd`.
- `f_102beba8` picks one of three masks (`0xff`, `0x55`, `0x11`) from a byte at `[rec+0x127]` and multiplies
  it into the word at `[rec+0x140]`, storing to `[rec+0x13a]`, `[rec+0x13c]` and `[rec+0x13e]`; entry
  conditions read a flag byte `[rec+0x126]` and two bits of the dword at `[rec+0x70]`. That is bit-depth
  scaling, not geometry.
- Ownership of that cluster is settled by its strings: read-only data `0x103c7d00..0x103c8100` holds an
  image-decoder error catalogue - `0x103c7e08` "Missing PLTE before IDAT", `0x103c7e90` "Ignoring extra
  png_read_update_info() call; row buffer not reallocated", `0x103c807c` "Width too large for libpng to
  process image data." - and the caller `f_102bd4d1` passes `0x103c7e08` and `0x103c7e90` to handlers
  `f_102bc97e` and `f_102bca0a`. Functions from about `0x102bc000` to `0x102c7xxx` are statically linked
  libpng/zlib.
- Image data for that check was read through pefile's own RVA-to-offset map over
  `research/xi-on-anything/generated/FFXiMain.unpacked.dll`, not by hand-computed PE offsets.

The decode found through kuluu's constants (the section above; it also retracts two claims made earlier in
this record):

- Search over all 21891 recompiled functions for the fetch law - a word load masked by `0x3FFF` or `0x7FFF`
  feeding a tripled index (`lea eax,[eax+eax*2]`) and then a four-fold step (`lea eax,[base+eax*4]`): 24
  functions with two or more such fetches. Collision band: `f_10023700`, `f_10024020`, `f_10024b60`,
  `f_10024c50`, `f_10024e30`. Upper band (role not established): `f_10167f50`, `f_10168470`, `f_101693b0`,
  `f_10169b40`, `f_1016a2d0`, `f_1016a990`, `f_1016b1d0`, `f_1016dfd0`, `f_10171930` (about 26k body lines),
  `f_10182700`, `f_10184fe0`, `f_10185ca0`.
- Sample fetch sequence, site `0x1016b4bb`: `mov ax,[ebp+6]; and eax,0x7fff; lea eax,[eax+eax*2];
  lea eax,[edx+eax*4]; mov edx,[eax]`; the two lower index words go through `and eax,0x3fff` at
  `0x1016b50e` / `0x1016b526`. Eight bytes of index words per triangle (`[ebp+2]`, `[ebp+4]`, `[ebp+6]`),
  twelve-byte vertices. These are the same masks and strides as
  `ffxi-dat/src/mzb.rs TRI_INDEX_MASK / TRI_FLAGGED_INDEX_MASK / MESH_TRIANGLE_LEN`.
- Wiring of the collision-band family: entry table in read-only data at `0x1032a430`, stored into `[obj]` by
  constructors `f_10025010` (call site inside `f_10029f70`, already named above as a registration path) and
  `f_100250c0` (inside factory `f_100250a0`). Table entry at `+0x1C` is `f_10025030`; the adjacent entry at
  `+0x20` is `f_10025060`. Both read manager dword `[0x1047d128]`, resolve through `f_10072fb0`, then call into
  the resolved object: sites `0x1002504f`/`0x10025056` (`f_10072fb0` then `f_100224d0`) and
  `0x1002507a`/`0x10025081` (`f_10072fb0` then `f_100231c0`).
- Down from there (whole-image re-grep of callers for every hop, this pass): `f_10025030` calls `f_100224d0`
  (site `0x10025056`, its only caller in the image); `f_100224d0` calls `f_10023290` (site `0x1002254d`, only
  caller); `f_10023290` calls `f_10023700` (site `0x100234ab`, only caller) and `f_10024900` (site `0x100235ad`,
  only caller); `f_10023700` calls `f_10023bd0` (site `0x10023bad`, only caller); all four `f_10024020` call
  sites are inside `f_10023bd0`. An earlier pass wrote that `f_10023290` calls `f_10023bd0` directly - wrong, the
  hop goes through `f_10023700`; and with every link single-caller, the triangle walk is statically reachable
  only through entry slot +0x1C. `f_10024900` remains the only caller of the near-identical trio `f_10024b60`,
  `f_10024c50`, `f_10024e30`. Shared primitives seen on these paths: vector add `f_10026f20`, move-block write
  `f_10027120`, scale `f_10027550`/`f_10027530`, normalize-with-length `f_100274b0`.
- Gating and pipeline detail (follow-down pass): the table VA is also stored into `[obj]` by `f_10025260`.
  Entry method `f_10025030`: inside `f_100224d0`, test bit `0x10` of dword `[arg+0x30]` gates the pipeline
  branch (branch-over at site `0x1002252f`); after calling `f_10023290` the code clears byte `[obj+0x3c]`
  when set (sites `0x10022552..0x10022559`). Entry method `f_10025060`: inside `f_100231c0`, calls the
  pose-offset builder `f_100231f0` only when byte `[obj+0x3c]` is zero AND bit `0x10` of `[arg+0x30]` is
  set (sites `0x100231d3..0x100231e5`); it also mirrors dword `[obj+0x40]` rounded up to sixteen into global
  scratch dword `0x10456e40`. An earlier pass called `f_100231f0` a "camera-skip builder"; the body (this
  pass) is staging arithmetic, not surfaces: loops over counts read as words at `[rec+0]` and `[rec+2]`
  through header pointer `[esi+0x42]`, addressing sixty-byte slots of global scratch `[0x10456e40]` by the
  x3/x5/*4 chain (sites `0x10023203..0x1002320f`), and per element doing two vector adds (`f_10026f20`) of
  the requester translation triple (`lea ebp,[arg+0x34]`, sites `0x1002321f`, `0x10023259`) into slot offsets
  `-0xc` and `0` of a cursor advanced by sixty (site `0x1002323b`).
- Instance pipeline fields (inside `f_10023290`, this pass): pose records at pointers read from object fields
  (`[obj+0x48]`, `[obj+0x4e]`) with counts in header words; prototype references load as bytes, masked by
  `and edx,0x7f` and scaled by sixty-four against global dword `[0x1045ec20]` (candidate loop site
  `0x10023352`). Pose application helper `f_100235c0` computes M times v plus t row-major over three rows at
  `[m+0]/[m+0x10]/[m+0x20]` (stride four floats, i.e. a 4x4 read as rows) with translation column at
  `+0x30/0x34/0x38`, writing the transformed triple to the caller's buffer.
- Narrow-phase march (`f_10024020`, corrected re-read this pass): the flag parameter at `[esp+0x4c]` carries
  an index halfword with a bit-`0x8000` selector (bit-test `and ecx,0x8000` site `0x1002402e`; index read via
  `and eax,0x7fff`). Slot step is `lea [eax+eax*2]` then `lea [eax+eax*4]` addressed as `[base + reg*4]` -
  **sixty bytes per index, not twelve**: the previous reading of twelve-byte-step fetches in this function
  (written there as adjacent-slot reads) was wrong, and the raw twelve-byte law belongs to the other readers
  (sample sites `0x1016b4bb`, `0x1016b50e`, `0x1016b526`). With bit `0x8000` set the first vector at slot
  offset `+0xc` is passed twice (push sequence sites `0x1002403f..0x10024074`, including
  `lea esi,[ecx+eax*4+0xc]` site `0x1002404e`); with it clear the offsets `+0xc` and `+0x18` are consumed
  (sites `0x10024076..0x100240af`). Per-lap hit-point writes `out.xyz += dir.xyz * t` at sites
  `0x100241d2..0x100241f4`; remaining-length float decremented by `[ebp]` against the same epsilon dword
  `0x1032a42c` (9.9999997e-07) at site `0x100241b5`, loop-back while above zero at `0x10024208`; callee
  cleans twenty-eight bytes (`ret 0x1c`). Edge-test sub-helpers taking the vector pointers and stack
  outputs/scalars: `f_10024220` (call site `0x100240b6`), then `f_10024330` (site `0x100240d8`) and
  `f_100243c0` (site `0x100240f3`); classifier `f_10024410` returns 0..4, branched at sites
  `0x10024144..0x1002415c`; their internals were not opened.
- Degenerate rejection lives in the walk around it (caller `f_10023bd0`): index words read with
  `mov ax,[edi]`, `test ah,ah; js skip` at sites `0x10023e85..0x10023e8a`, re-checked for the second word at
  sites `0x10023eb5..0x10023ebb`; face records are ten-byte groups (`[edi]` and `[edi+2]` halfwords feed
  `f_10024020`, whose own record-pointer step of +10 is shown in the sibling below).
- Posed-face materialisers (sibling trio, `f_10024c50` read this pass; siblings `f_10024b60`, `f_10024e30`
  recorded near-identical by the constant search): record pointer steps **ten bytes** (`add ebp,0xa`, site
  `0x10024d20`); three indices masked via `and eax/esi/edi,0x7fff` at record offsets +0/+2/+4 (sites
  `0x10024c5f..0x10024c79`) address the same sixty-byte staged slots (`lea [eax+eax*2]`, `lea [eax+eax*4]`,
  addressing `[ebx + reg*4]`, sites `0x10024c76..0x10024cb5`), taking each slot's vector at offset `+0xc`; per
  vertex a block move (`f_10027120`) and scale (`f_10027550`), plus three adds of the placement-record
  translation triple (offset +0x30, sites `0x10024cec`, `0x10024cfd`, `0x10024d10`).
- Registration drive and both candidate walkers (this pass). Append inside `f_10029f70`: proxy handle stored,
  then `mov ecx,ebx` site `0x10029fbe`, append via `call f_10029f00` site `0x10029fc3` (helper `f_10029eb0`
  walks `{obj,next}` nodes rooted at `[receiver+0x20]` and returns the address of the last next-field). The
  only callers of `f_10029f70` are sites `0x100cf29e` (inside `f_100cf210`) and `0x100cfa89` (inside
  `f_100cfa50`); an inline-member factory sits at site `0x100cf245` (`lea ecx,[esi+0x674]; call f_1002b6a0`,
  the 0xc8-byte sub-object alloc through `f_1002c890`, construct `f_10029d30`, self-register via append helper
  `f_1002b630`).
- The +0x1C walk: wrapper `f_1002c360` (list root via `f_1002b5a0`; per-node `call f_1002a3b0` site
  `0x1002c384`; the loop body inside `f_1002a3b0` dispatches `call dword ptr [eax + 0x1c]` at site
  `0x1002a6c1`, advances nodes at sites `0x1002a6c4..cf`, and issues four release calls through the vtable at
  `[global 0x1045666c]+0xc` slot `+0xb8`). The only caller of `f_1002c360` is `f_1002b6e0`, which stages a
  query descriptor copy through `f_10026e90` and touches the global object `[0x1045666c]`; in turn
  `f_1002b6e0` is called exactly twice, both inside scene-registration function `f_100cbde0`, whose own
  callers are thin wrappers `f_100cbd60` (two tail calls) and `f_100cbd90`. The walk's tail also re-issues two
  `[global 0x1045666c]+0xc` vtable calls (ids `0x18`, `0xb`) and sets bit 2 (`or al,4`) on `[edi+0x30]`, the
  requester state word.
- The +0x20 walk: driver `f_1002c270`; its per-node dispatch is inside `f_1002a700` (`call dword ptr
  [eax + 0x20]` site `0x1002a737`, list head via `f_10029e70`, `ret 8`). Callers of `f_1002c270`: scene driver
  `f_100c63d0` - the controllable-interface entry named in the movement map, reached through its tail-call
  wrapper `f_100abbd0`, installed at table `0x1032e890` per the census above (call site inside `f_100c63d0`
  recorded at funcs_013 line 69350) - and `f_100ccf50`, which has no direct callers in any compiled function
  body: it is reachable only through indirect dispatch (a table.c entry).
- Walker position integration and its output pointer (this pass, for the vertical-store hunt): `f_100a6240`
  ends by loading a caller-supplied triple pointer from its stack argument (`mov eax,[esp+0xec]`, site
  `0x100a7769`) and adding the accumulated step into each component - three add/store pairs at sites
  `0x100a7774..0x100a7789` covering all three axes; a sibling triple-add lives earlier in the same function
  (sites around `0x100a6f3c/46`). The pointer argument passes through unchanged from wrapper `f_100a5ec0`
  (`mov ecx,[esp+8]; push ecx`, call site `0x100a5edf`; the function itself ends `ret 4`). Scene-side probe
  consumer `f_100cd040` calls the wrapper (site `0x100cd07c`) and consumes only the returned margin float - it
  never reads a buffer back. So no vertical store from walk or probe was found; height integrates with X and Z
  in one add.
- The two candidate classes seen from that walk, read through their tables this pass. Collision proxy: entry
  slot +0x20 is `f_10025060`, whose resolved backend method `f_100231c0` has an entire body of one scratch
  mirror (`([arg+0x40]+0xf) & ~0xf` into dword `[0x10456e40]`, sites `0x100231c4..0x100231cd`) plus, gated by
  pending byte zero AND requester bit `0x10`, the single call `f_100231f0` (site `0x100231e5`; whole body spans
  sites `0x100231c0..0x100231ea`) - no edge into any test exists in it. Scene-inline member: ctor `f_10029d30`
  installs vtable `0x1032a710`; image-read slots give `+0x1C` = `f_10029880` and `+0x20` = the shared stub at
  site `0x10001370` (`ret 4`). The per-node call itself pushes exactly two words - walk receiver and zero (push
  sites `0x1002c281/283`) - so nothing about actors, positions or directions crosses this boundary.
- Engine modes of `f_1002c400`: mode argument compared with 2 at site `0x1002c40c`; the mode-two branch reads
  the placement record via element accessor `f_1002a8b0` (site `0x1002c464`), scales through `f_10027310`
  (site `0x1002c490`) and merges by vector add `f_10026f20` (site `0x1002c49f`); the other branch validates
  nodes then forwards to batch path `f_1002a750` (call site `0x1002c4ed`), whose body is descriptor copy
  `f_10026e10` (site `0x1002a759`) followed by placement element accessor `f_1002a780` (site `0x1002a769`).
  Neither branch contains any dispatch into the candidate walkers, which is how "the mover probe answers from
  placement records" was established.
- Why the instance-table census missed this: the census kept only tables whose entry at `+0x20` performed
  floating-point work. The table at `0x1032a430` has no floating-point in its `+0x20` entry (`f_10025060`
  resolves a handle and tail-calls), so it never entered the census; the geometry lies two hops below, behind
  the manager resolve. Image references for these addresses were located by scanning the unpacked image for
  little-endian dwords equal to each function address through pefile's section map.

Still unresolved, and why: the geometry each registered instance points at (vertex arrays, planes, box or
capsule extents) is a pointer loaded from runtime state by the DAT block loaders - per INSTALL.md step 5 that
is the honest stop. The productive next lookup is those loaders (which DAT block ids and which writes fill an
instance's geometry pointers), not further dispatch-table work; the second open item there - whoever writes
the actor position triple - was closed the following pass, below.

### Position quads, the commit setter, walking vertical zero, band naming (follow-up pass)

Commit setter and quads:

- Commit setter `f_100a57c0` - copies a quad from its argument into receiver fields: stores at sites
  `0x100a57c6` (`[ecx+0xD4]`), `0x100a57cf` (`+0xD8`), `0x100a57d8` (`+0xDC`), `0x100a57e1` (`+0xE0`).
  Called from 36 sites; enclosing functions: `f_1008f740` (11x), `f_10085e40` (2x), and singly by
  `f_10014390`, `f_10047590`, `f_10047960`, `f_10057fb0`, `f_1005e9c0`, `f_1005eef0`, `f_1008cd90`,
  `f_100929d0`, `f_10092c20`, `f_10092f90`, `f_100a4ed0`, `f_100a57f0`, `f_100c3900`, `f_100c47f0`,
  `f_100c8ed0`, `f_100c9330`, `f_100c9810`, `f_100c9c10`, `f_100ca310`, `f_100ca730`, `f_100caa60`,
  `f_100caf90`, `f_10242cd0`.
- Anchor quad `[obj+0xC4]/+0xC8/+0xCC/+0xD0`; working quad `[obj+0xD4]/+0xD8/+0xDC/+0xE0`; scratch quad
  `[obj+0x5C4]..` (same object; written by the script applier below and copied to the `+0x5FC` block, whose
  dword at `+0x608` gets f32 1.0 at site `0x100aa08f`).
- Update unit `f_100a57f0`; stored as a vtable function at image VA `0x1032dfe8`. Direct callers: `f_100a4ed0`
  (funcs_011.c:41578; itself stored at image VA `0x1032db60`), `f_100ac4e0` (funcs_012.c:5998; stored at
  `0x1032fd78`), `f_100c47f0` (funcs_013.c:47628).
- Seed select inside `f_100a57f0`: guard tests sites `0x100a585b..0x100a5871` (`[actor+0x88]` bit 1; script
  bytes `[esi+0x103]`, `[esi+0x104]`). Anchor seed: loads `mov eax,[esi+0xc4]` site `0x100a5873`, `+C8` at
  `0x100a5879`, `+CC`, then `+D0` load at `0x100a5889`; temps stored sites `0x100a5885/8f/93`. Working seed
  (movement engaged): sites `0x100a589d..0x100a58bd` reading `[edi]`, `[esi+0xD8]/DC/E0` where `edi` was set by
  `lea edi,[esi+0xd4]` site `0x100a5816`. Slot seed: four vcall sites `[edx+0x1bc]` at `0x100a58c7`, `0x100a58d7`,
  `0x100a58e8`, `0x100a58f9`; shared impl `f_100a4740` = `lea eax,[ecx+0x5FC]`. Fourth word from `[block+0xC]`,
  stored at temp site `0x100a5902`.
- Entry backup copy: `lea edi,[esi+0xd4]; lea eax,[esi+0x5b4]; push edi; push eax; call f_10026e90` sites
  `0x100a5816..0x100a5824`. Helper direction confirmed from its body: arg one is destination - loads `[esp+4]`
  as dst base at site `0x10026e94`, copies four dwords (`mov [ecx],edx` sites `0x10026e9a..0x10026eab`).
- Orientation snapshot copy `[esi+0x44..0x50] -> [esi+0xE4..0xF0]` four dwords, guarded by the bit test at
  `0x100a5829/2f/32`, sites `0x100a5837..0x100a5855`; parametrization of that four-float block (quaternion vs
  Euler) not established - the walker's five `fstp [esi+0xE8]` stores sit inside it.
- Commit call inside `f_100a57f0`: `lea edx,[esp+0xc]; mov ecx,esi; push edx; call f_100a57c0` sites
  `0x100a5c2d..0x100a5c34`; tail vector op reading `[esi+0xD4]..` against `[esi+0x5B4]` at `0x100a5d37..0x100a5d40`.
- Mode branches in `f_100a57f0`: script-move pairs (helper then walker wrapper `f_100a5ec0`) - `f_100a9fc0` at
  `0x100a5a46` with wrapper at `0x100a5a52`; `f_100aa1f0` at `0x100a5a6d` then `0x100a5a79`; further pairs
  through `0x100a5ae2/0x100a5afd`. Remote branch gated by virtuals `[vtable+0x360]`, `+0x36c`, `+0x34c`, `+0x358`,
  `+0x378` (sites `0x100a5b12..0x100a5b52`) then interpolation helper `f_100aab70` at site `0x100a5b5b`. Wrapper
  `f_100a5ec0`: skips everything when `[esi+0x88] bit 0` set (site `0x100a5ec3`), else vcall `[vtable+0x310]`
  must return zero (sites `0x100a5ece..0x100a5ed6`) before calling walker `f_100a6240` at site `0x100a5edf`; the
  walker's only call site in the image. Tail-call wrapper `f_100a5dc0`: resolves an object through singleton
  `mov ecx,0x1047d600; call f_10081550` (site `0x100a5dd5`) and tail-jumps the wrapper at site `0x100a5e03`.
- Remote interpolation helper `f_100aab70`: accumulator reads `[ebx]/[esi+0x134]/[esi+0x138]` added to the temp
  argument, sites `0x100aabc5..0x100aabe9`; dt timer pair `[esi+0x144]`/`[esi+0x148]` read-update-rewrite sites
  `0x100aab7d..0x100aab91`; gate float `[esi+0x14C]` compared to f32 zero at site `0x100aabed`.
- Walking vertical zero: `mov dword ptr [esp + 0x20], 0` (displacement triple's vertical slot) sites `0x100a649f`
  and `0x100a76ae`, the latter immediately after `lea eax,[esp+0x18]`/`push eax` (site `0x100a76ab`) in the run
  that reaches gate `f_100a8770` (call site `0x100a7760`) and then the tail add sites unchanged from earlier
  sections (`0x100a7769..0x100a7789`).
- Mirror unit `f_100c47f0` (stored at image VAs `0x103306c0` and `0x10330ac0`): calls the update unit at site
  `0x100c47f8`; mirror copies `[esi+0xD4]->[esi+0x734]` site `0x100c480b`, `[+D8]->[+738]` site `0x100c481a`,
  `[+DC]->[+73C]` site `0x100c4832`, and f32 1.0 to `[+0x740]` site `0x100c4826`. Other classes repeat the
  triple at the same offsets (accessor/flag family `f_100a48a0`..`f_100a4a00` operates on `[ecx+0x734]` with
  or/and-not/bit-test idioms, so that word doubles as flags somewhere - offset reuse; role unresolved).
- Position-set unit `f_1008cd90`: external vector to scratch quad (copy-helper call site `0x1008d88f`, dst =
  `[handle+0x5C4]` materialized by `add ecx,0x5c4` at `0x1008d887`, handle fetched from `[esi+0xA0]` site
  `0x1008d881`), commit call `f_100a57c0` with receiver `[esi+0xa0]` at site `0x1008d89e`, anchor copy via
  helper call site `0x1008d8b1` (dst `[handle+0xC4]` materialized by `add edx,0xc4` at site `0x1008d8aa`).
  Sole caller enclosing function is `f_1008e9a0` (funcs_010.c:48756).
- Upper band naming via the install's hook labels in `research/xi-on-anything/meta/builds.json` (build
  2026-10-05, read through json): `scene_zone_draw` = function start `f_1017c9c0`; `scene_zone_draw_end` =
  `f_1017de20`; map-cull hook returns resolve by table bisect to `f_1017c8b0`, `f_1017c9c0`, `f_1017de20` (x2),
  `f_10183da0`; model-cull hooks into `f_10183490` and `f_10183760`. Census members in the band contain no
  allocator (`cexi_malloc`) or resource-manager (`scene_res_mgr` global) references - checked across all of
  them - so they are draw-side consumers of raw twelve-byte-step vertices, not loaders.

### Position command, controller class and its writer set (wire-side pass)

- Command allocator convention re-verified from the body of `f_100fc3c0`: first pushed argument is the
  sub-opcode id, buffer returned per sender slot. Across all of `generated/all` exactly one call site pushes a
  position-command immediate: `push eax; push eax; push 0x15` at `0x10098540..0x10098542`, inside encoder
  `f_100983f0`; commit `push 0; push 0x20; push pkt; call f_100fc4d0` at `0x100985fb..0x10098600`. Body size
  matches the server header for that id (`vendor/server/src/map/packets/c2s/0x015_pos.h`).
- Encoder call chain (per-frame tick of the local player's controller class, direct calls only; no image data
  reference to `f_100983f0` or `f_10096640` exists - both verified with a byte-aligned dword scan across every
  PE section of `FFXiMain.unpacked.dll`, so the chain is direct-call only): task-table member at image VA
  `0x10329d1c` = `f_100106c0`; it gates on `[0x104568fc]` byte 9 and calls `f_10096640` (site `0x100106cc`);
  which at site `0x10096b08` sets `ecx = edi` and calls the encoder. The task-table neighbourhood (`0x10329cc0..`)
  is a run of function pointers, consistent with a frame-task registration block.
- Encoder field reads (quoted sites): state-block position copy - `[edi+0xd4]` at `0x1009842c`, `[[..]+0x260]+0x140`
  materialized `0x10098436/0x1009843c`, copy-helper call site `0x10098444`; orientation block same pattern at
  `+0x150`, sites `0x1009844f..0x10098461`. Magnitude sanity sum: `fld [esp+0x14] / fadd [esp+0x10] /
  fadd [esp+0xc]` compared against the constant at `0x103295d8`, sites `0x10098513..0x1009852a`. Packet stores:
  `[esi+4]=eax←[esp+0x10]`, `[esi+8]=ecx←[esp+0x14]`, `[esi+0xc]=edx←[esp+0x18]` (sites `0x10098563..0x1009857c`,
  all three after the single `push edx` at `0x1009855a`, so they read the same triple the sum tested); clock word
  `[esi+0x12]`; facing byte via `f_100dd970` to `[esi+0x14]`; mode bitfield writes to `[esi+0x15]`; facetarget
  `[esi+0x16]` from `f_101597f0`; timestamp `f_100dc120` to `[esi+0x18]`. Skip gates: state words equal to two
  event states, required bit of word `[edi+0x120]`, flag byte `[[edi+0xa0]+0xf8]` selects the alternative seeding
  branch, and a global disable byte.
- Alternative-seed branch and command history: locals reseeded from controller fields via helper call sites
  `0x10098489/0x10098497`; snapshot copy `[edi+4..]→[edi+0x24..]` gated by bit 13 of `[edi+0x12c]`, sites
  `0x100984ad..0x100984c2`.
- Controller state block construction: factory `f_100bd1a0` (allocation through `cexi_malloc`, request size
  matching the class), its pointer stored to `[ctrl+0xD4]` at site `0x1008ee01` inside unit `f_1008edc0` (which zeroes the field when
  the allocation fails, site `0x1008ee1f`) and cleared by unit `f_1008ee30` at site `0x1008ef37`; the created
  object self-references through its own fields at `+0x25C` and `+0x260`, sites `0x100bd1c8/0x100bd1ce`,
  which is why readers dereference `[obj+0xD4]+0x260` to reach plain fields of that same block. One creation site
  exists image-wide, so this controller block is a singleton per local-player object.
- Writer set of the transmitted position block (all straight dword stores through the state-block self pointer):
  `f_1008cd90` sites `0x1008d155/0x1008d164/0x1008d173/0x1008d182` (fourth word) copying from a handle object's
  field set at `+0x34..+0x40`, or for another entity id from `[id*4+0x10480af0]` record fields `+0x34/+0x38/
  +0x3c/+0x40`; mirror unit `f_100bcdd0` reading the entity-table record's first position triple at
  `+4/+8/+0xc` (orientation from the record set one field-group further on) with store sites
  `0x100bcdf6/0x100bce05/0x100bce14/0x100bce25` for the position quad and `0x100bce41/0x100bce50/0x100bce5f/
  0x100bce70` for the orientation quad, plus a float pair at sites `0x100bce89/0x100bce8f`; unit `f_1009ee60` (called by thin wrappers `f_1009ee40`, `f_1009f2d0`) stores
  the same eight words from an argument record at `[a0+0x10..]` and `[a0+0x20..]`. Image-wide, no other function
  stores to those offsets behind a state-block base (census over all `funcs_*.c`).
- Copy-helper confinement: the shared four-word copy helper has 283 call sites image-wide; filtering by argument
  windows carrying a `+0x260` base leaves exactly three functions, all in this controller band - `f_1008f740`
  (ten sites, moving quads between the state block and fields of the scene object behind `[ctrl+0xA0]`, at those
  fields' `+0x34/+0x44/+0xC4` blocks), `f_10092f90` (seven sites, same field pairs plus the script-applier blocks
  at handle offsets in the `0x5C4/0x61c` family), and the encoder `f_100983f0` itself. Direction of every helper
  call was checked against its body (`mov eax,[esp+8]; mov ecx,[esp+4]`: source is the first pushed argument,
  destination the last). The state-block reads in `f_10092f90` also feed a mover-side commit through the setter at
  site `0x10093060`.
- Negative established this pass (finite census, all files): no function anywhere stores a coprocessor result into any
  component of the transmitted POSITION quad behind a state-block base; the three writers listed above move those four
  words by integer-word copy only. Float stores to offsets that collide with these names exist elsewhere and belong
  to other classes (checked individually - one script-parameter unit writes `+0x140/+0x144/+0x148` of its own object
  from scaled integers). Within this block family the float traffic is orientation-side only: two facing units write
  orientation components, one computing an angle with the four-quadrant arc-tangent helper after resolving both
  entities through the entity table (`f_100b6700`, coprocessor store into a component of the orientation quad), and
  the mirror unit additionally copies `[rec+0x9c]` (float pair sites `0x100bce89/0x100bce8f`) into the separate block
  field above. Note the block is reached two ways - through another object's pointer field, or directly from a class
  that carries its own self-reference at the same offsets (the mirror unit uses `mov ecx,[esi+0x260]`), which is why
  the writer census had to require both a state-block read and a qualifying store.
- Additional writers of the orientation quad, for completeness: `f_100b6870` zeroes/overwrites orientation components
  from registers (a reset path alongside the facing unit).
- Census hazards hit again this pass, recorded so later passes do not repeat them: built-in `re.match` anchoring
  silently discarded every instruction regex until switched to `search`; `funcs_011.c` carries CRLF endings while
  `funcs_010.c` does not
  (line-window scans must normalize); instruction-comment offsets are lowercase hex while generated-C argument
  lines use uppercase suffixes, so a single-case pattern drops half of any census; and a window sized in *lines*
  rather than instructions under-reads context because each instruction expands to about eight source lines.

### Authoritative actor-record writer census (terrain-height closing pass)

Record class and table. Table base `0x10480af0`, indexed `[id*4 + base]`. Constructor `f_1008a120` stores vptr
`0x1032d69c` then tail-calls real init `f_1008a350`; the vtable region at that address holds only a handful of
slots, so this class's behaviour lives in plain member functions in the `0x1008a000..0x1009exxx` band. Spawn
placement unit `f_10094190` (single call site inside the entity-band handler run beside bulk refresh `f_10096640`)
calls the constructor at site `0x10094931`, stores `[ecx*4+0x10480af0] = new record` at `0x10094947`, and writes
the triple from stack arguments first: `[esi+4]=[esp+0x24]`, `[esi+8]=[esp+0x28]`, `[esi+0xc]=[esp+0x2c]` at sites
`0x1009494e/0x10094955/0x1009495c`. Init member `f_1008a350` zeroes the triple with the working blocks
(representative store `[esi+0xc]=ebx` at site `0x1008a3a4`, with `ebx=0`).

Writer census. Image-wide regex over all 21,891 functions for dword stores (`mov dword ptr` / `fstp`) to same-base
offsets {4,8,c} yields 649 (function, base) pairs; requiring entity membership - the record flag words at
+0x120/+0x124 with the refresh masks (`and [X+0x120], 0xfffffe7f` style), constructor-identity bases,
table-derived or scene-back-pointer-derived `this` - leaves exactly seven members: `f_1008a350` (init);
`f_10094190` (spawn-placement); `f_1008b170` (attach-record copy - stores `[ecx+4..0x20]` from
`[[ecx+0xa0]+0x34..0x50]` at sites `0x1008b236/0x1008b23c/0x1008b242`, and mirrors the same four words to globals
`0x10485f88..94` and `0x1048557c..88`; called from entity-band handler site line 92057 and from bulk refresh site
`0x10096970`); `f_1008ee30` (refresh mirror - stores `[esi+4/8/c]` from `[[esi+0xd4]+0x260]+0x140..148` at sites
`0x1008ee93/0x1008ee9c/0x1008eea5`, plus orientation from `+0x150..158` into record `+0x14..0x1c`; called from
seven command sites: the bulk-refresh loop over 2,304 table slots with mover-flag gates (per-slot call pair
`call f_1008ee30 ... call f_10092900`, site line 103154; loop body sites `0x10096a??` band), teardown
`f_100ae3f0` (site `0x100ae63e`), and handlers at site lines 19341, 128899, 129390, 130526, 142883, 144821; also
clears the state-block pointer at site `0x1008ef37`); `f_1008cd90` (relocation-command member - windows below);
`f_100929a0` and `f_100933c0` (the two promotion members). Promotion, plain: `f_100929a0` copies
`[X+0x24/28/2c] -> [X+4/8/c]` at sites `0x100929a3..0x100929af`, then tail-jumps into the fan-out member at site
`0x100929c4`; its six callers include two scene-side setters (below). Promotion, gated: `f_100933c0` tests record
flags (`[esi+0x120] bit 9`, `[esi+0x1d0] bit 10`) and copies the same triple at sites
`0x100933e0/0x100933e6/0x100933ee` (default branch) or writes a computed word at `0x10093415` after helper
`f_100929d0`; called from leave-zone teardown site `0x100ae62b`, immediately before the `-1.0` float write to
`[record+0x204]` at site `0x100ae632`. No coprocessor (`fstp`) store writes a component of the triple behind any
entity-qualified base (finite census; re-checked after the decimal-offset fix below).

Fan-out member (pass-through pin). `f_10092c20`, gated on record flag bit 9 and an animation-state switch, copies
record `+4/8/c` through the shared four-word helper into: attach-record `+0x34..+0x3c` (`lea eax,[aux+0x34]` site
`0x10092c60`), anchor block `[aux+0x5fc]` with fourth word set to 1.0 at site `0x10092c7d`, script-applier block
`[aux+0x5c4]` (site `0x10092c8d`), the mover commit through setter call site `0x10092ca4` (`f_100a57c0`: writes
`[obj+0xd4..0xe0]` from an argument quad, sites `0x100a57c6..0x100a57e1`), and the reverse copy of `[aux+0xc4]`
into record `+0xc4` at site `0x10092cb0`; orientation triples fan similarly (`[aux+0x44..] from record
`+0x14..`, sites `0x10092cc1..0x10092cc9`) with epsilon clamps against constants at `0x10329d28/2c/30`. Callers of
the fan-out include relocation windows (sites `0x1008d470/0x1008d4d0/0x1008d6ee/0x1008d754`) and external command
sites (`f_1007ad19` band, `0x1009eda4`).

Scene-side setters feeding the record. The scene object class whose committed quad `+0xd4..0xe0` mirrors to
`[obj+0x734..0x740]` keeps a back-pointer to its actor record at field `+0x70`: setter unit at `0x100c3ff0` writes
an argument triple into `[record+0x24/28/2c]` (sites `0x100c4001/0x100c4007/0x100c400d`) and calls the promotion
member at site `0x100c4010`, then mirrors its own transform quad `[obj+0x734..0x740]` from the argument quad (sites
`0x100c4018..0x100c4033`) and copies a further block through the helper (site `0x100c403d`); the sibling setter
(`0x100c4050`, call-site line 44387 context) writes `[record+0x34/38/3c]` (sites
`0x100c4061/0x100c4067/0x100c406d`, promotion at site `0x100c4070`) then mirrors to `[obj+0x744..]`. Both take the
triple from their caller's argument buffer - no computed component.

Relocation-command windows (`f_1008cd90`). Six arithmetic windows write record `+4/8/c` or the scratch blocks
feeding them: target-minus-current divided by elapsed into per-axis rate words `[X+0x54/58/5c]`, then rate-scaled
adds - sites `0x1008d359..0x1008d3a8` (straight-copy branch at `0x1008d3ad..0x1008d3bb` when the tick test fails),
plus sibling windows near `0x1008d420-0x1008d470` and `0x1008d4a8-0x1008d4f8`, a subtraction pair set near
`0x1008d5c0-0x1008d620`, and two more add-scaled windows near `0x1008d618-0x1008d64a` and `0x1008d6b0-0x1008d750`;
a counter compare against `0x1f` gates one window (site `0x1008d717`), after which fixed authored offset floats at
globals `0x104859e8/0x104859ec/0x104859f0` are added to all three components (sites `0x1008d720..0x1008d751`).
Door/event transition save/restore: gate on animation-state words 2 or 7 and event ids `0x308` / `0x309`; store
record `[esi+4..] -> globals 0x10482f88..0x10482f90` (orientation to `0x10480990..0x1048099c`) at sites
`0x1008d79a..0x1008d7df`; restore into the triple at sites `0x1008d802..0x1008d81b`. Reset inside bulk refresh
zeroes those globals (sites `0x1009690e..0x10096954`). Command dispatch context: the handler band's opcode switch
jump table at `0x1009f29c` (dispatch site `0x1009eebc`); one handler there (`f_1009ee60`) also copies a wire
argument record's two quads into both record sets and the state block (sites including
`0x1009efa5..0x1009efdc`).

Teardown note. `f_100ae3f0` - earlier flagged as a possible per-frame follower - is leave-zone/teardown: its two
call sites are the teardown path gated on global `0x10487e48` (site line 82885) and an in-game state handler at
`0x100ae972` behind `call f_100a0660; cmp al,1`. Its scene-query calls (`push 0x42480000` = 50.0 scalar into
singleton `0x10631f24` methods `f_10181290` / `f_10181300`, sites `0x100ae4b0/0x100ae518`) feed replacement values
for the record's target-triple writes at sites `0x100ae53e..0x100ae551`; these are exit-time adjustments to the
target block, never touching `+4/+8/+c` themselves.

Registration-unit correction. `f_100ab750` (mover band) computes `[ecx+0x70] = ecx + 0xa0c` with an id tag from
global counter `0x10487fb4` (`0x4000000 | id & 0xffffff`) - it does not store a record pointer into a handle, and
no reciprocal `[handle+0x70] = entry` write was found image-wide; the handed-over claim of that link does not
reproduce on this build.

Census hazard added: small memory offsets print in decimal form in generated C - `[ecx + 4]`, `[esi + 8]`, but
`[esi + 0xc]`. A hex-only pattern silently drops every one- and two-digit-offset store; a first census under this
pattern returned zero triple writers before the parser accepted both forms (belongs beside the lowercase/uppercase
note above).

### Mover mode machinery: gating virtuals, accumulator triple, and quad feeds (mode-machinery pass)

Slot table. Read statically from the unpacked image at `vtable + slot` for all eight class tables
(`0x1032d69c`, `0x1032d710`, `0x1032e890`, `0x1032ecb0`, `0x1032f0d0`, `0x1032f4f0`, `0x10330f40`,
`0x103313e8`). The seven mover classes are byte-identical across all probed slots; only the scene-entry table
(`0x1032d69c`) differs:

| slot | mover classes (all seven) | scene-entry table `0x1032d69c` |
| --- | --- | --- |
| +0x110 | `f_100849a0` | `f_100d09b0` (readiness-scaled float scalar: branches on a virtual at the same slot and constants `0x103295d8`, `0x10331398`) |
| +0x34c | `f_100827a0` | `f_100a9810` -> thunk `f_100a98d0` (tag constant `0xc7ef`) |
| +0x358 | `f_100827f0` | `f_100a9870` -> same thunk (tag `0x11323`) |
| +0x360 | `f_10082820` | `f_100a98b0` -> same thunk (tag `0x19031`) |
| +0x36c | `f_10082860` | `f_100d67b0` (virtuals `+0x2a4`, `+0x3e4`; helper `f_100d6850`; collision-manager call `f_10072fb0`) |
| +0x378 | `f_100828b0` | `f_100a4640` (object-identity predicate versus singleton through `f_10081550`) |

Shared implementations are type-code range predicates on a field: `f_100827a0` tests `[ecx+0xa4] in {1..4}`,
`f_100827f0` equals `0x54`, `f_10082820` in `{5..0x24}`, `f_10082860` in `{0x3d..0x51}`, `f_100828b0` in
`{0x25..0x34}`; each returns the bool in `eax`. In the update routine every one gates a branch only (`test
eax,eax; jne ...` at sites `0x100a5a0a/16`, `0x100a5b18/26/34/42/50`) - none receives or returns a float.

Update routine and quad feeds (`f_100a57f0`). Working quad at `[esp+0xc..0x18]`; commit through the setter at
site `0x100a5c34`. Stores into the quad's second component (slot +4), whole-function list:
`0x100a58af` (`[esp+0x10]=ecx`, seeding from stored block `[X+0xc8]` - orientation-seed branch); `0x100a58b5`
(`[esp+0x14]`, same family); the mode-helper fills described below; and re-seeds inside `f_100aab70` at sites
`0x100aac6f/0x100aabd3..dc` (accumulator sums). Nothing else in the 405-instruction body stores to `+4`; the
predicated virtual-call runs (`0x100a5b12..50`) and the post-commit transform mirror touch no quad component.
Mode feeds: `[X+0x103]` handler `f_100a9fc0` - coprocessor adds into `[dest],[dest+4],[dest+8]` at sites
`0x100aa05c/0x100aa069/0x100aa077` (and `0x100aa183..9a`) scaling script-applier blocks `[actor+0x5c4]/[+0x5fc]`
(fourth word set to 1.0 at site `0x100aa08f`) by progress `(maxdist - dist) * scale` with the distance from
virtual +0x1c8 (query id `0x19`, via `f_100d4b90`). `[X+0x104]` handler `f_100aa1f0` - straight dword copies of
anchor words at sites `0x100aa228/0x100aa230`, then coprocessor adds from the anchor pair into dest at sites
`0x100aa35e..6b`. `[X+0x102]` handler `f_100aab70` - accumulator triple (`[actor+0x130/134/138]`) added
component-wise into dest at sites `0x100aabd3/abd6 (via fadd)/abe3`, inside a loop bounded by an angular-blocked
sweep (`f_100aab30` with constant `0x3f4f5c28` ~ 0.8036 rad over block `[actor+0x150]`) plus vector helpers;
consumed-delta write-back happens through the copy/rotate helpers, individual sites not traced further this
pass. `[X+0x101]` handler `f_100aadf0` - copies accumulator block `[actor+0x120..]` into dest via the helper at
sites `0x100aae3c-43`. Walk path: virtual +0x310 gate (`f_100a46c0`; scene-entry variant `f_10084cc0`) then the
integration unit; its displacement triple is trig-scaled from heading and speed, its vertical slot carries the
stored delta or zero (zero stores at sites `0x100a75ae [esp+0x20]=0` style windows: `mov dword ptr [.. + 0xc], 0`
halfword writes observed in-sequence), consistent with the previously pinned vertical-zero law.

Accumulator writer census (`[X+0x130/134/138]`). Both print forms (decimal `+ 304` etc. and hex) over all 21,891
functions: 68 functions store behind non-stack bases. Zero coprocessor stores exist at these offsets image-wide -
all writes are integer-word copies or read-modify-write flag packing. Two base families share the offsets:
(a) actor-record flag words - xor/shl/and bit packing from payload words (`f_1009ad20`, `f_1009adb0`,
`f_1009ade0`, `f_1009af60`, `f_1009d0f0`, `f_1009cee0`, `f_100b1150`, `f_100bac90`, record-side factories
`f_100ab1b0/360/590`), plus init zeroing (`f_1008a350`) and small accessors (`f_10092780/f_10092800` - shift-9
mask RMW, `shr eax, 9 ... or ebx, 0xfffe01ff; and` at sites `0x100927da..eb`); (b) mover-side words seeded at
construction from parameter-default globals blocks (`f_100a89e0`: `[esi+0x130/134/138] = [0x1035bc0c/10/14]`,
sites `0x100a8af2..0x100a8b0f`; companion orientation defaults at `[0x1035bcc0..cc]` into `[mover+0x120..0x12c]`
from arming unit `f_100aad40`). The mover accumulators' only float-semantics reader is the mode handler above;
the sole writer that computes their contents is the arming unit `f_100aa940` (desired-minus-current of mover
position via virtuals +0x1bc, rotation helpers `f_10027510/f_100272e0`, sites `0x100aaa35-0x100aaaa3`; sets mode
byte at site `0x100aaace`). Arming units: `f_100aa940` (accumulator mode) called from three handlers including
two in a 7,400-instruction command processor band (`f_10057fb0`, call sites `0x10062...` region of funcs_005);
orientation/applier arming helpers `f_100aa380/f_100aa3b0` (set mode bytes +0x104/+0x103) called from the same
command processor (`f_10057fb0`, sites in its 70k-line body); mover factories `f_100ab1b0/f_100ab360` construct a
record via `f_1008a120`, store it to the first free table slot (`[edi*4+0x10480af0]` sites `0x100ab1ff/0x100ab3af`
with search loops from `0x3ff`), and their object-tail field `[esi+0x70]` is then used to set record fields
(sites `0x100ab21c..46`, `0x100ab3dc..fa`) - establishing mover-side record attachment through the tail field, not
through `f_100ab750`. Geometry involvement: none. No writer or feed in this pass reads the twelve-byte-vertex /
eight-byte-triangle structures or staged sixty-byte slots; the only collision-adjacent call seen (`f_100d67b0`,
scene-entry slot +0x36c) routes to the collision manager for candidate registration bookkeeping and stores no
position component.

Census note: mode bytes live at `[mover+0x101..0x104]`; the arming census caught unrelated classes through offset
collisions (`f_101ade??` band, `f_102ed9??`) which were inspected and excluded.

### Contact probe, fall step and measurement feedback (contact-probe pass)

Probe virtual `f_100d4800` sits at slot +0x1c8 in all seven mover tables (`0x1032d710`, `0x1032e890`, `0x1032ecb0`,
`0x1032f0d0`, `0x1032f4f0`, `0x10330f40`, `0x103313e8`; read from `.rdata` with pefile; image data scan shows those
seven as its only code references) - the same slot the scalar wrappers dispatch through, which is why earlier passes
saw "[vtable + 0x1c8]" calls without naming the body. Body: `lea edi,[esi+0x678]` result base; query-quad copy
helpers `f_10026e90` (four-word move) / `f_10026eb0`; vector merge `f_1002c3e0` called with `[esi+0x674]`, `[esi+0x688]`,
result base, query id and mode word; gates `call [eax+0x34c]` / `+0x358` / `+0x36c` paired with predicates
`f_10084390` (entity kind via `[obj+0x70]+0x170`), `f_10084350`, `f_10084370`, `f_10084330`; contact test
`f_1002c5c0` (this = `[esi+0x674]`) with the pending-index quad at `[esp+0x2c]` written as `0xbfa66666` (-1.3, site
`0x100d48de`) or `0xbf000000` (-0.5, site `0x100d491b`) only when the test returns zero; lean ladder
`call [edx+0x110]` with `[esi+0x75c]` / `[esi+0x754]`; accumulation sites `fld [esp+0x2c]`, `fmul`, `fadd [edi+4]`,
`fstp [edi+4]` (`0x100d498a..0x100d4995`).

Contact test internals: list-head accessor `f_1002b5a0` = `lea eax,[ecx+0x44]` (base `[esi+0x674]`, i.e. mover +
`0x6b8` - inside the staged sixty-byte slot family); per-entry next walk at `[node+4]`; dispatch `call dword ptr
[eax+0x20]` per shape (`f_1002c5d2..0x1002c5ef`); leaf `f_1002a9b0` -> `f_100351f0` -> `f_10035210`, the last reading
indexed triangle halfwords: `mov cx, word ptr [eax+0x32]`, then stride arithmetic `lea ecx,[ecx+ecx*2]; lea
ecx,[ecx+ecx*4]; lea eax,[eax+ecx*2+0x34]` - the same twelve-byte vertex / eight-byte-triangle strides as
`f_10023290`; bounds: index word `>= 0x80` returns zero (`0x1002c5c6..0x1002c5cd`, `0x1002a9c6..0x1002a9d1`).
Registration-side pending byte test/clear pair: sites `0x10022552` / `0x10022559`; stores to that same field from
candidate constructors `0x10025025`, `0x1014141b`, `0x10141455`, `0x1014149f`, `0x1011994e` (image-wide
`mov byte ptr [reg+0x3c], 1` census); no store from the commit unit was seen, hence the open cadence word.

Scalar wrappers of the same slot: margin helper `f_100aaef0` (`push 0x2c`, then `[esp]`/`[esp+8]` squares, fsqrt,
`fmul dword ptr [0x10329a34]`; reads words three and four of the answer, never word two); triple-length helper
`f_100d4b90` (`push 0x19`, dot-with-self via `f_10027530`, fsqrt); global publishers `f_100aaf30` (`push 5`; `mov eax,
[esp+4]` -> `[0x10456da8]`) and `f_100d6670`, which sits at slot +0x3f8 in all seven mover tables: branches store
answer word two as integer (`0x100d66a0..0x100d66ab`, `[esp+0xc]`), whole (`0x100d66dc..0x100d66e7`, query id 7) or
rescaled by `[0x10329a34]` (decoded +0.8 from the unpacked image; recorded as "halved" before decoding) - `fld [esp+8]; fmul dword ptr [0x10329a34]` (`0x100d672a..0x100d6718`) - plus a forward-only user
`f_100c9310`. Direct virtual sites of the slot (whole-image census, 21): `f_1001ee60` @`0x1001f344/0x1001f4fa/0x10020044`,
`f_10021280` @`0x10021322`, `f_100aa4e0` @`0x100aa610/0x100aa623/0x100aa63c`, `f_100aaef0` @`0x100aaf00`,
`f_100aaf30` @`0x100aaf3c`, `f_100c9310` @`0x100c9326`, `f_100c9330` @`0x100c9760`, `f_100cff40`
@`0x100d01f4/0x100d0205` (ids 0xd and 0xf, lengths only), `f_100d4b90` @`0x100d4b9c`, `f_100d4bc0`
@`0x100d4c5a/0x100d4c6b/0x100d4c7c/0x100d4c8d` (ids 9, 8, 0xb, 0xa), `f_100d6670` @`0x100d669a/0x100d66d6/0x100d6704/0x100d6724`.
Measurement consumers of `[0x10456da8]`: writes `f_1001e600` @`0x1001e67f`, the publishers above; reads
`f_1001ee60` @`0x1001f464/0x10020f16` (the first adds it into `[esp+0xa8]`, which is state-block +4 of `lea ebx,
[ebp+0x5fc]`, and writes the sum back at `0x1001f471`), scaling drivers `f_100c5440` @`0x100c549c..0x100c54a8` (
when its mover-state getter `f_10084420` answers 5 or 6 (calls @`0x100c5486/0x100c5492`, compares @`0x100c548b..0x100c549a`) it reloads `[0x10456da8]`, multiplies by dword `[0x10329a34]` (+0.8 decoded), stores the product back, then zeroes companion `[0x10456dac]`; corrected from "half-scale" 2026-10-26) and `f_100c54c0` @`0x100c551c..0x100c5528` (same pattern); both call the
+0x3f8 slot via `call dword ptr [edx + 0x3f8]` (sites `0x100c547e`, `0x100c54fe`) on a mover reached through
`f_10081550`/`f_10081600`. Second state-block vertical feed: `fld [0x10456da8]` into the block's +4 with the two
horizontal components adjusted by measured differences at sites in `f_100c8280` (`0x100c837f..0x100c839b`, via
`lea edi,[esi+0x5fc]` and helpers `f_10027cf0`/`f_10028200`).

Lean field writers (image-wide, both print forms): mover-side stores `[reg+0x75c]` at `0x100c4573`, `0x100c60da`,
`0x100ca66c` (`fstp [ebx+0x75c]` after `fadd`/`fmul` of constants), `0x100ca7f4`, `0x100cf563` and `0x100cf889`
(`= 0x3f800000`, mode reset); `[reg+0x754]` at `0x100c4561`, `0x100c60fa`, `0x100ca656` (`fst dword`),
`0x100cf52b/0x100cf55d/0x100cf58a/0x100cf64d/0x100cf6a6/0x100cf8c8`, sources `f_10084540` (shared actor getter,
tail-jump to `f_1009aec0`, default `[0x103295d8]`) and `f_10084500` (entity bits). Getter behind mover slot +0x110:
`f_100849a0` (`fld [[ecx+0x70]+0x204]`, default `[0x1032a3f0]`).

Committed-Y feed, quoted: commit unit `f_100a57f0` seeds its stack quad at sites `0x100a58c3..0x100a5906` - four
separate `call dword ptr [edx + 0x1bc]` accessor reads, `[eax]`, `[eax+4]`, `[eax+8]`, `[eax+0xc]` - the accessor
being slot +0x1bc = `f_100a4740`, which is simply `lea eax,[ecx+0x5fc]`; commit call site `0x100a5c33` -> setter
`f_100a57c0`. State-block Y (`[mover+0x604]`) census, non-stack bases: stores at `0x100c5f75` (from global block
`0x1035d6c0..cc`, inside `f_100c5d60`; that unit also arms a `[esi+0x75c]` store nearby), and the two floa
t feeds
`f_100c8280` @`0x100c839b` and `f_1001ee60` @`0x1001f471` (the latter through `[ebp+0x5fc]`). No coprocessor store
from the probe answer's word two into committed coordinates was found; helper carrying it, if any, is unresolved.

Mover-family anchor sites used above: constructor tails `f_100d7240` (sites `0x100d7376..0x100d73af`, seeds
`[esi+0x5fc]` and `[esi+0x678]`, sets `[esi+0x608]` / `[esi+0x9e4]` to one), `f_100c52a0`; per-frame mover bodies
`f_100cbde0` (staging/registration calls into `f_1002b6e0` with `[esi+0x674]` bases at sites `0x100ccdc1`,
`0x100ccdf5`), `f_100cf210`, `f_100c63d0`. Unresolved from generated C: none new - every function named above is a
direct call target or read out of `.rdata` tables. Runtime-state pointers remain only the engine query-unit
dispatch inside `f_1002c3e0`, and the class behind entity field `[obj+0x70] + 0x204` whose getter (`f_100849a0`) was
read but whose class was not classified this pass.

### Step acceptance and probe anchor (step-up pass)

- Margin wrapper re-read end to end: `f_100aaef0` dispatches `[vtable+0x1c8]` with a stack buffer (`lea edx,[esp]`,
  size word `0x2c`, sites `0x100aaf00..04`), then computes `sqrt(buf[0]^2 + buf[8]^2) * [0x10329a34]` - the
  multiply at site `0x100aaf1a` against f32 `[0x10329a34]` = zero-point-eight; buffer offset +4 is never addressed
  (whole body, sites `0x100aaef0..0x100aaf23`).
- Probe answer accumulation target re-pinned: handler `f_100d4800` loads the caller buffer at site `0x100d4804`
  (`mov ebx,[esp+0x20]`) and folds `penalty * lean` into it through `fadd/fstp [edi+4]` sites `0x100d4992/5`, where
  `edi` is set from `ebx` at site `0x100d4873`; the descriptor translation triple is copied from mover fields
  `[esi+0x698]/[0x69C]/[0x6A0]` through setter `f_10026e30` at sites `0x100d481a..0x100d4847` with the
  f32-one seed push at site `0x100d4827`.
- Per-frame writer of those anchor fields: scene driver `f_100c63d0`, float stores at sites `0x100c80ae`
  (`fstp [esi+0x698]`) and `0x100c811b` (`fstp [esi+0x6a0]`), each fed from a ladder chain over accessor
  `[vtable+0x110]` and actor posture floats `[esi+0x754]`, `[esi+0x758]`, `[esi+0x75c]`, `[esi+0x760]`; the
  same driver also refreshes the staged answer blocks at `[esi+0x678]` / `[esi+0x688]` through copy helper
  `f_10026e90` (sites `0x100c7fef..0x100c803b`) and dispatches query virtuals `[vtable+0x1bc]` /
  `[vtable+0x1c0]` beside them. How the ladder values compose into the probe anchor is not further resolved.
- Contact walk re-walked: `f_1002c5c0` list-head call `f_1002b5a0`, chain nodes `{obj,next}` walked at sites
  inside its body, per-candidate id getter `f_1002a9b0` rejecting indices at or above `0x80` (sites
  `0x1002a9c6..0x1002a9d1`) - no vertical-height comparison anywhere in the walk.
- Constant census for this pass: across handler, wrapper, engine entry (`f_1002c400`) and contact walk, the only
  f32 constants touched are zero-point-eight `[0x10329a34]`, one-seeds, the millionths epsilon reused by the march,
  and the two penalty dwords already recorded. Nothing resembling a maximum-rise or slope threshold appears.

### Gates, contact selection, lean ladder and cadence (mover-probe re-read)

- Gate virtuals resolved from all seven mover tables (`0x1032d710`, `0x1032e890`, `0x1032ecb0`, `0x1032f0d0`,
  `0x1032f4f0`, `0x10330f40`, `0x103313e8`) by pefile reads: slot +0x34c is `f_100827a0` everywhere
  (`[ecx+0xa4]` in `[1,4]`), slot +0x358 is `f_100827f0` (== `0x54`), slot +0x36c is `f_10082860`
  (`[obj+0xa4]` in `[0x3d,0x51]`), and the lean getter slot +0x110 is `f_100849a0` everywhere
  (`fld dword ptr [[ecx+0x70]+0x204]`, fallback `fld dword ptr [0x1032a3f0]`; image value at `0x1032a3f0` is
  `bf800000` = minus one point zero).
- Entity-kind pairs: handler calls the mover-side wrappers `f_10084350` (site `0x100d48a2`), `f_10084370`
  (`0x100d48c3`), `f_10084330` (`0x100d4900`), each fed by the shared entity-kind getter `f_10084390` (sites
  `0x100d489a`, `0x100d48bb`, `0x100d48f8`) and each a null-guarded tail jump on `[ecx+0x70]` to the entity-side
  comparators `f_10095680` (== 5), `f_100956a0` (== `0x55`), `f_10095790` (cmp chain from `0x3f` upward);
  no attached entity answers zero (`xor al,al`).
- Handler flow sites in `f_100d4800`: second stack argument read at `0x100d4804` (buffer) and used as the id word
  from `[esp+0x28]` at `0x100d48cc`/`0x100d4909`; staged-copy helper call `f_10026e90` @`0x100d4815`; transform
  descriptor build via `f_10026e30` (seed push `0x3f800000` @`0x100d4827`, triple stores `0x100d482f/39/43`, call
  @`0x100d4847`); engine query wrapper `f_1002c3e0` @`0x100d486d`; subtract helper `f_100270a0` @`0x100d487a`
  with `edi` re-pointed to the caller buffer at `0x100d4873`; penalty local zeroed @`0x100d4886`;
  gate call sites `0x100d488e`, `0x100d48af`, `0x100d48ec`; branch structure: first pair true → contact at
  `0x100d48cc` (jne @`0x100d48a9`), second pair false → third gate at `0x100d48e8` (`je 0x100d48ea` site
  `0x100d48ca`); contact leaf calls `f_1002c5c0` @`0x100d48d5` and @`0x100d4912`, each followed by
  `test eax,eax`/nonzero-skip (`0x100d48da/dc`, `0x100d4917/19`; first site skips to the third-gate branch,
  second skips accumulation); stores `-1.3` @`0x100d48de`, `-0.5` @`0x100d491b`; lean ladder body
  `0x100d4923..0x100d4984` (getter calls `0x100d4927/0x100d4951/0x100d4970/0x100d4984`, fields `[esi+0x75c]`
  loaded at `0x100d493a`/`0x100d4978`, field `[esi+0x754]` at `0x100d4964`, float-zero comparisons against
  `[0x103295d8]`); accumulation `fmul dword ptr [esp + 0x2c]` @`0x100d498a`,
  `fadd/fstp dword ptr [edi + 4]` @`0x100d4992/0x100d4995`; `ret 8`.
- Contact walk body (whole, `f_1002c5c0`, twenty-five instructions): anchor fetch via `f_1002b5a0`
  (`lea eax,[ecx+0x44]`) @`0x1002c5c2`; null-anchor early-zero return `0x1002c5c7..cd`; **position-skip loop**
  advancing next-links until the counter equals the second stack argument (`0x1002c5d8..e7`: `mov edi,edx`,
  `inc edx`, `cmp edi,esi` with `esi` from `[esp+0xc]` @`0x1002c5d3`, cursor via `lea eax,[ecx+4]; mov ecx,
  [ecx+4]`); single leaf call `f_1002a9b0` @`0x1002c5f0` with `this=[eax]` and the first stack argument pushed
  (@`0x1002c5e9`). **No `[eax+0x20]` dispatch exists in this body** - the earlier provenance line attributing
  per-shape dispatch to sites inside `f_1002c5c0` is withdrawn. Leaf chain: `f_1002a9b0` bumps `ecx` by eight
  (@`0x1002a9b6`), keeps the index cap (index word `>= 0x80` → zero, sites `0x1002a9c6..0x1002a9d1`) and calls
  `f_100351f0`; `f_100351f0` gates nullness of the sub-call result (@`0x100351f5`) and offsets the returned pointer
  by `+26*id + 4` (`lea edx,[ecx+ecx*2]; lea ecx,[ecx+edx*4]; lea eax,[eax+ecx*2+4]`, sites
  `0x100351fc..0x10035206`); innermost `f_10035210` requires `[node+0xc]` non-null (@`0x10035217`), runs the
  readiness method `f_10071060(this = node base + 0xc)` (@`0x1003521d`, `test al` @`0x10035222`), resolves the
  handle `[node]` against the collision-manager singleton `[[0x1047d128]]` via `f_10072fb0` (push sites
  `0x1003522a/32`, call @`0x10035233`) and returns the hit bank indexed by its own halfword: `mov cx, word ptr
  [eax + 0x32]` then `+26*cx` offset from `[eax + 0x34]` (`lea ecx,[ecx+ecx*2]; lea ecx,[ecx+ecx*4]; lea eax,
  [eax+ecx*2+0x34]`, sites `0x1003523b..0x10035245`).
- Pending-byte (staging gate) setter census, complete as of this pass: candidate-node construction stores at
  `0x10025025`; the reset unit is `f_101198d0` (store site `0x1011994e` inside its body), reached from exactly one
  caller site `0x10118027` inside `f_10117f40`, itself called from exactly one site `0x1015e4a1` inside
  `f_1015e350`; the command family dispatcher is `f_101417c0` (jump table at `0x10141830`, indexed word minus
  one, nine arms) tail-calling setter siblings including `f_101413f0` (@`0x101417d7`), `f_10141400`
  (@`0x101417e4`), `f_10141430` (@`0x101417f1`), `f_10141470` (@`0x101417fe`), `f_10141700`, `f_10141730`; that
  dispatcher has no direct call sites (image grep) and appears only in the address/function table. The test/clear
  pair inside the staged walk stays at `0x10022552/0x10022559`.
- Shape-block constructor (`f_1002b4b0`, whole body twenty-seven instructions): first vptr store
  `mov dword ptr [esi], 0x103299f0` @`0x1002b4b6` overwritten by final `[esi] = 0x1032a75c` @`0x1002b4ee`; flags
  word zeroed `mov dword ptr [esi + 0x30], eax` (eax from xor) @`0x1002b4f4`; extent seed
  `mov dword ptr [esi + 0x40], 0x40000000` (+2.0 float) @`0x1002b4f7`; candidate anchor zeroed `[esi + 0x44]`
  @`0x1002b4fe`; trailing fields `[esi+0xa8]/[esi+0xac]` zero, bytes `[esi+0xb0]/[esi+0xb8]` zero,
  `[esi+0xb4]` zero; sub-block constructors `f_10033710` (+0x48 @`0x1002b4bc`), `f_1001d170` (+0x74
  @`0x1002b4c4`), `f_10025bf0` (+0x8c), `f_10025260` (+0x90), `f_10038a20` (+0x9c). Mode-installer call sites
  (this = mover + 0x674): `0x100c52ab`, `0x100c556b`, `0x100c56eb`, `0x100c588b`, `0x100c59ba`, `0x100c5d3e`.
  Query-mode bit test inside the engine entry: `test byte ptr [esi + 0x30], 0x10` @`0x1002c4b2`.
- Result-vector consumer sites. Camera vertical feed: in `f_1001ee60`, probe dispatch `[edx+0x1c8]` @`0x1001f344`
  with id byte from accessor `[vtable + 0xc4]` masked to `0xff` (`and eax,0xff` @`0x1001f32b`; skip-to-alternate
  when equal to `0x80` @`0x1001f335`); answer word two consumed at `fld dword ptr [esp + 0xa0]` /
  `fadd dword ptr [esp + 0x198]` / `fstp dword ptr [esp + 0xa0]` (sites `0x1001f34a/51/58`). Camera standoff
  inputs: accessor pairs `[vtable+0x3cc]` (call site `0x1001f398`) and `[vtable+0x3c8]` (`0x1001f3ed`),
  each consumed as `fld [eax+4]; fsub [eax]`; fixed substitution `mov dword ptr [esp + 0x10], 0x40200000`
  (two point five) @`0x1001f3cc` when a comparison against `[0x10329ea0]` fails; scale multiply by
  `[0x10329a30]` @`0x1001f432`.
- Movement-state quartet: in `f_100d4bc0`, probe dispatches with stack buffers push ids `9`, `8`, `0xb`, `0xa`
  (push sites `0x100d4c56/67/78/89`), each result float later compared against `[0x1032a3d8]` before the unit
  composes step-event command words from bytes at `[mover+0x879]`, `[mover+0x87a]` and a string-match chain over the
  global `0x1065cb04` (helper `f_101880b0`, compare constants as raw dwords `0x6e696172`, `0x6c757173`,
  `0x746c6f62`; seed dwords `0x73657366`/`0x72657366` stored at sites `0x100d4db0..0x100d4df2`) - the chain's
  semantics beyond flag-setting are not claimed. Periodic sweep: `f_100aa4e0` computes ids as `3*counter + 13` (`lea edi,[ecx+ecx+0xd]` @`0x100aa604`,
  variants at `0x100aa61c/0x100aa62b..0x100aa635`) with dispatch sites `0x100aa610/623/63c`; answers combined by
  the shared vector helper `f_10027550` (calls @`0x100aa671`, `0x100aa685`, `0x100aa69c`) and compared against
  `[0x1032a22c]` (@`0x100aa6a1..`). Length-only users (`f_100cff40` ids `0xd`/`0xf`, `f_100d4b90` id `0x19`) and
  the publishers (`f_100aaf30` mode five, `f_100d6670` modes including seven) are as sited in earlier subsections.
### Face classification sites (flag-word versus ground/wall/ignore)

- Absence-of-branch census: whole collision-band grep for `and|test` against `0x8000`, `0x4000`, `0xc000`,
  `shr/sar … 0xf` returns exactly one hit (`and ecx, 0x8000` @`0x1002402e`, the already-recorded staged-index
  fetch-shape selector inside `f_10024020`) and no others in any function on the probe path (`f_100d4800`,
  `f_1002c3e0`/`f_1002c400`, `f_1002a700`, `f_10025060`, `f_1002c5c0` and leaf chain), i.e. no membership-bit or
  validity-bit branch exists on the contact path.
- Build-side nibble assembly (record-building unit `f_1016b1d0`, funcs band containing the zone model builders;
  called from scene-band units at sites `0x1016919a`, `0x10169240`, `0x10169276`, `0x101813c0`, `0x101818d2`):
  four halfword loads `[ebp+0]/[+2]/[+4]/[+6]` then per-word `shr …, 0xf` isolation and `or` composition, first
  halfword's bit ending up lowest of the nibble (`mov dx,[ebp+6]; shr edx,0xf; add edx,edx; mov ax,[ebp+4];
  shr eax,0xf; or edx,eax; …` sites `0x1016b7bc..0x1016b7e6` and the duplicate pass at
  `0x1016bbcc..0x1016bbf6`); stored as per-record metadata `[obj + 0x34148]`. Masked index fetches in the same
  unit at sites `0x1016b4bb`, `0x1016b4e7`, `0x1016b50e`, `0x1016b526` (`and eax, 0x7fff` / `and eax, 0x3fff`).
- Nibble consumers (whole-image grep of the field): writers `0x1016783a`, `0x101678fa`, `0x10169395` (zero),
  `0x101699fe` and the assembly sites; readers: getter `f_10168fc0` (`mov eax,[ecx+0x34148]; ret`) with exactly
  two call sites `0x1018190d`, `0x10181997` (scene band), and copier `f_10168f40` which writes the nibble, a
  scalar-scaled companion field (`[+0x3414c]` times `[0x1032ae50]`) and `[+0x34150]` into an output record - no
  call chain reaches the movement anchors from these consumers.
- Validity/camera-transparency bits: image-wide, no test of `0x4000` appears in any collision-band function;
  degenerate rejection at march time stays as recorded (sign tests on fetched index halfwords, "Down from the
  entry methods").

### Re-read carry-over open items (mover-probe re-read)

- Unresolved from that pass: none new. The engine query unit's interior dispatch inside `f_1002c3e0`
  remains runtime-state; the hit bank that `f_10072fb0` returns per handle is a runtime allocation - its record
  layout (halfword index at +0x32, slot stride twenty-six starting +0x34) is read from code, not asserted static.
  The exact predicate semantics behind entity kinds eighty-four / eighty-five and the sixty-three-upward list are
  kind numbers only; their names are not claimed.

### Character sweep pass (2026-10-26, spec section "Movement-time collision: the character sweep")

Generated C for build 2026-10-05 (`research/xi-on-anything/generated/all`, procedure per
`vendor/rea/INSTALL.md`), cross-read against `research/XIClient/src/XIClient/source/World/Zone/Terrain/CollisionManager.cpp`
(`KO_CharaCollision`, `SetThings`, `KO_ExecSpCorrect`, `InterpolatePosition`) whose constants and branch sites all landed:

- Entry, offsets, padding seed, grid/candidate collection: `f_1016e930` (endpoint vertical offset reads of the
  half-yalm constant at sites `0x1016e95d`, `0x1016e981`; success-side restore at `0x1016eb83`, `0x1016eba4`).
- March loop, step and threshold setup: `f_1016f0e0` — step = probe-padding x times `[0x1032b15c]` (site
  `0x1016f0f7`); threshold store `[esi+0x3412c] = 0x3e800000` at `0x1016f13b`; corrected triple writes
  `[edi+0x341ac..+0x341b4]` from the settle passes.
- Narrow test (the two-class law): `f_1016f320` — march-time back-facing gate reads `[edi+0x34124]` at
  `0x1016f372`; out-of-reach epsilon subtraction `[0x1032a22c]` at `0x1016f4a5`; wall-band comparisons against
  `+0.708` `[0x1033663c]` at `0x1016f4bd` and `-0.708` `[0x10336638]` at `0x1016f4d3`; wall height-rejection
  tolerance `[0x10336648]` at `0x1016f4fe`; nearest trackers `[edi+0x34130]` load `0x1016f4e4` / store
  `0x1016f5fc`, and steep-class tracker `[edi+0x34134]` load `0x1016f613` / store `0x1016f69c`; the correction triple is
  computed from `sqrt([0x3412c] - d2)` (`fsqrt` sites `0x1016f576`, `0x1016f5cb`) with the vertical factor
  `[0x1032a39c]` at `0x1016f5e1`; floor-contact branch sets `[edi+0x34128]`/`[edi+0x34126]` and writes
  `[edi+0x341b0]` at `0x1016f686..0x1016f692`; steep-class height rejection reads `[0x1032b15c]` (the same
  0.3) at `0x1016f62d`.
- Constants decoded from the unpacked image (`research/xi-on-anything/generated/FFXiMain.unpacked.dll`, PE
  RVAs = generated addresses minus the module base): `[0x103295d8]=0.0`, `[0x10329a08]=0.5`,
  `[0x1032a22c]=0.001`, `[0x1032a39c]=0.2`, `[0x1032b15c]=0.3`, `[0x10336638]=-0.708`,
  `[0x1033663c]=+0.708`, `[0x10336648]=0.002`.
- Not resolved this pass: which callers arm the priority-section ordering flags and the per-object `something2`
  identity exclusion; whether any caller passes a padding other than half-a-yalm (all sites read here used the
  0.5 constants). The XIClient reconstruction's height-rejection branch wording was verified against the
  instruction addresses above, not assumed.
