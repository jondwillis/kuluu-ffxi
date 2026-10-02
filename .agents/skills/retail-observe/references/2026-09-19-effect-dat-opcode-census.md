# Effect-DAT opcode census, 2026-09-19

Block-count and payload-shape observations from a static scan of the retail
effect-DAT corpus of an unidentified client build (the particle-generator and scheduler blocks the effect
files ship). Kuluu's parsers used to carry these figures inline in their
comments; they live here so the code keeps only the citation and the
behavioral WHY.

## Method and naming

Static scan of the effect DATs an install actually ships, walked through the
same `action_dat_file_id` dispatcher the runtime uses. Reproduce with:

```
FFXI_DAT_PATH=<install> cargo test -p kuluu-render --test effect_instruction_census
```

(the test self-skips without an install). Install row: `ROW_TBD` (the
`KNOWN_CLIENTS` row the figures were scanned against is still open; the
counts below are provisional corpus-shape observations, not a pin to one build).
Do not use these counts as a release-wide coverage percentage or assign them
to a KNOWN_CLIENTS row without recovering the original scan provenance or
repeating the scan against an explicitly identified install.

Two corpora are distinguished throughout:

- **parser-accepted corpus** — the blocks the parsers actually decode
  (the "Shipped census" figures in `ffxi-dat/src/particle_gen.rs`).
- **raw probe walk** — a broader walk that also counts blocks the parser
  drops; only called out where it diverges from the accepted corpus.

## Coverage and evidence limits (reviewed 2026-09-21)

`Decoded` in the generator opcode sink reports parser recognition. It does
not certify that the renderer implements the opcode or reproduces its retail
appearance. In `ffxi-dat/src/particle_gen.rs`, `ParticleGeneratorDef::parse`
accepts several updater families without arming runtime behavior:

- Child-generator updaters (section 3, 0x25/0x33) and the child expiration
  handler (section 4, 0x01).
- Color-transform application (section 3, 0x0B) and RGB progress updates
  (0x18 through 0x1A).
- Velocity rotation (section 3, 0x26/0x2F), point-list position (0x34), and
  specular progress updates (0x36/0x37/0x3B).

Some payloads are retained in `ParticleGeneratorDef` for later use; others
are only consumed. These are known rendering gaps in the reviewed PR800
stack, not evidence that the corresponding retail instructions are no-ops.
An empty dropped-opcode log therefore establishes neither complete runtime
coverage nor visual parity. Other accepted instructions may already have
runtime equivalents, so an empty parser match arm alone is not proof of a gap.

Names and behavior descriptions attributed to `research/xim` come from a
community reimplementation. XIM is not a retail decompile. The XIClient
reconstruction under `research/XIClient`, direct client observations, and
build-identified binary or DAT measurements are distinct evidence sources;
apply the authority ranking in `research/AGENTS.md` to each claim. This
static census does not substitute for a runtime comparison with retail.

## Particle generator, section 1

| opcode | name | census |
| --- | --- | --- |
| 0x11 | AssociationUpdater | config word is followPosition(0x1), followFacing(0x2), factor(>>2); the shipped values are 0x0003fd (follow position only, factor 255) and 0x0003ff (both, factor 255) |

## Particle generator, section 2 (initializers)

| opcode | name | census |
| --- | --- | --- |
| 0x03 | VelocityVarianceSetup | 6311 blocks, all size_words=4, every one after its generator's 0x02 |
| 0x08 | RelativeVelocitySetup | 12461 blocks, all size_words=2, every one after its generator's 0x02 |
| 0x0A | RotationVarianceInitializer | 27736 blocks, all size_words=4, payloads are radian angles (±π, ±π/2, …); 2423 all-zero |
| 0x10 | ScaleVarianceInitializer | 2669 blocks, all size_words=4, [0, 2] per axis, every one behind a 0x0F base scale |
| 0x17 | ColorVarianceSetup | 7654 blocks, all size_words=2, alpha byte always 0, every one behind a 0x16 base color |
| 0x19 | ColorTransformSetup | 15815 blocks, all size_words=3, alpha always 0, every one behind a 0x16 base color |
| 0x29 | KeyFrameValueSetup (scale.z) | 3684 blocks, all size_words=4, first payload word always zero, config always single-cycle |
| 0x2A | KeyFrameValueSetup (color.r) | 4431 blocks, all size_words=4, first payload word always zero |
| 0x2B | KeyFrameValueSetup (color.g) | 5933 blocks, all size_words=4, first payload word always zero |
| 0x2C | KeyFrameValueSetup (color.b) | 3302 blocks, all size_words=4, first payload word always zero |
| 0x32 | HazeOffsetInitializer | 1150 sec2 0x32 blocks in the parser-accepted corpus |
| 0x3B | IncrementalRotationApplier | 19903 blocks, all size_words=4, payloads are radian angles, 1389 all-zero |
| 0x3D | OscillationSetup | 441 blocks, all size_words=1, every one precedes its 0x3E/0x40 acceleration setup |
| 0x3E | OscillationAccelerationSetup (X) | 113 blocks, all size_words=3, every one behind a 0x3D marker |
| 0x3F | OscillationAccelerationSetup (Y) | 34 blocks, all size_words=3, every one behind a 0x3D marker |
| 0x40 | OscillationAccelerationSetup (Z) | 347 blocks, all size_words=3, every one behind a 0x3D marker |
| 0x41 | RelativeVelocityVarianceSetup | 9199 blocks, all size_words=2 |
| 0x44 | ChildGeneratorSetup | 1009 sec2 0x44 blocks in the parser-accepted corpus, 1041 in the raw probe walk |
| 0x45 | ParentPositionCopyConfig | 9698 sec2 0x45 blocks, all size_words=1; 1189 generators carry it, 164 of them referenced as a child by a sec2 0x44 link |
| 0x46 | ParentVelocityConfig | 252 sec2 0x46 blocks in the parser-accepted corpus |
| 0x47 | ParentRotateConfig | 543 sec2 0x47 blocks in the parser-accepted corpus |
| 0x48 | ParentColorConfig | 37 sec2 0x48 blocks in the parser-accepted corpus |
| 0x49 | ParentScaleConfig | 141 sec2 0x49 blocks in the parser-accepted corpus |
| 0x4A | ParentTexCoordConfig | 115 sec2 0x4A blocks in the parser-accepted corpus |
| 0x4E | FixedPointPositionVarianceSetup | 46 sec2 0x4E blocks in the parser-accepted corpus |
| 0x4F | FixedPointPositionVarianceSetup (twin) | 442 sec2 0x4F blocks in the parser-accepted corpus |
| 0x51 | KeyFrameValueSetup (velocity.y) | 10 sec2 0x51 blocks in the parser-accepted corpus |
| 0x53 | ChildGeneratorSetup (twin) | 392 sec2 0x53 blocks in the parser-accepted corpus |
| 0x54 | PointListPositionSetup | 50 sec2 0x54 blocks in the parser-accepted corpus |
| 0x56 | BatchingSetup | 387 sec2 0x56 blocks in the parser-accepted corpus |
| 0x59 | KeyFrameValueSetup (specular rot.x) | 235 sec2 0x59 blocks in the parser-accepted corpus |
| 0x5A | KeyFrameValueSetup (specular rotation.y) | 719 blocks, all size_words=4, first payload word always zero, 719/719 behind a 0x55 SpecularParams record |
| 0x5B | KeyFrameValueSetup (specular rotation.z) | 214 sec2 0x5B blocks in the parser-accepted corpus |
| 0x5D | KeyFrameValueSetup (specular color.g) | 14 sec2 0x5D blocks in the parser-accepted corpus |
| 0x5F | KeyFrameValueSetup (specular color.a) | 133 sec2 0x5F blocks in the parser-accepted corpus |
| 0x67 | ReverseDisplacementSetup | 307 blocks, all size_words=2, payload always 0.0 |
| 0x69 | KeyFrameValueSetup (velocity dampener) | 2 sec2 0x69 blocks in the parser-accepted corpus |
| 0x72 | ProjectionBiasInitializer | 24180 blocks, all size_words=3, param0 [-26, 1], param1 [-8.6, 2.5] (14388 zero), zero co-occurrence with 0x30 |
| 0x79 | ParentRotateConfig (twin) | 15 sec2 0x79 blocks in the parser-accepted corpus |
| 0x82 | CameraShakeSetup | 4032 blocks, all size_words=6, first payload word always zero, 46 distinct track ids, 4032/4032 generators also carry the section-3 0x5F CameraShakeUpdater |

## Particle generator, section 3 (updaters)

| opcode | name | census |
| --- | --- | --- |
| 0x02 | PositionUpdater | 92938 blocks, all size_words=1, 92937 of them in generators carrying a sec2 0x02 base velocity; the 8 velocity-carrying generators without the block are never position-stepped by retail |
| 0x0B | ColorTransformApplier | 66990 blocks, all size_words=1 |
| 0x0C | ColorTransformModifier | 2379 blocks, all size_words=3 |
| 0x0D | SpriteSheetFrameUpdater | 7492 sec3 0x0D blocks, paired 1:1 with the 7492 sec2 0x1D initializers |
| 0x0E | NoOpParticleUpdater | 71013 blocks, all size_words=1 |
| 0x15 | scale.x ProgressValueUpdater | n=57310, all size_words=1, every one behind its sec2 scale track |
| 0x16 | scale.y ProgressValueUpdater | n=59425, all size_words=1, every one behind its sec2 scale track |
| 0x17 | scale.z ProgressValueUpdater | n=8320, all size_words=1, every one behind its sec2 scale track |
| 0x18 | color.r ProgressValueUpdater | n=11559, all size_words=1, every one behind its sec2 color track |
| 0x19 | color.g ProgressValueUpdater | n=15450, all size_words=1, every one behind its sec2 color track |
| 0x1A | color.b ProgressValueUpdater | n=9580, all size_words=1, every one behind its sec2 color track |
| 0x1B | color.a ProgressValueUpdater | 64963 blocks, all size_words=1; the shipped corpus has zero generators carrying the 0x2D alpha track without this updater |
| 0x25 | ChildGeneratorBasicUpdater | n=9488, all size_words=1, every one behind its sec2 child link |
| 0x26 | VelocityRotator | 8524 blocks, all size_words=4 |
| 0x29 | OscillationApplier (X) | 113 blocks, all size_words=4, every one in a generator carrying the 0x3D marker |
| 0x2A | OscillationApplier (Y) | 30 blocks, all size_words=4, every one in a generator carrying the 0x3D marker |
| 0x2B | OscillationApplier (Z) | 347 blocks, all size_words=4, every one in a generator carrying the 0x3D marker |
| 0x2C | VelocityDampener | 51845 blocks, all size_words=3 |
| 0x2F | VelocityRotationUpdater | all size_words=1 |
| 0x33 | ChildGeneratorUpdater | n=1656, all size_words=1, every one behind its sec2 child link |
| 0x34 | PointListPositionUpdater | 292 blocks, all size_words=1, paired 1:1 with the sec2 0x54 setup (same 22 files) |
| 0x36 | specular rotation.y ProgressValueUpdater | n=1481, all size_words=1, 100% paired with the sec2 0x5A |
| 0x37 | specular rotation.z ProgressValueUpdater | all size_words=1, every one behind its sec2 specular track |
| 0x3B | specular color.a ProgressValueUpdater | all size_words=1, every one behind its sec2 specular track |
| 0x44 | dampening-factor ProgressValueUpdater | 14 blocks, all size_words=1, every one behind a sec2 0x69 |
| 0x5F | CameraShakeUpdater | 1451 four-word blocks, 8 three-word, every one behind a sec2 0x82 |

## Scheduler (MAPSCHEDULOR scene resolution)

- `ZONE_SCENE_PARTNERS` (ffxi-dat/src/scheduler.rs): hand-built from one scan
  of the retail corpus, which turned up 27 "another zone's model DAT" pairs;
  retail's loader rule for the partner fallback is unknown, and the five
  shipped pairs are the observed clean instance/entrance cases.
- `NON_MODEL_SCENE_CARRIERS` (ffxi-dat/src/scheduler.rs): hand-built from the
  same corpus scan; retail's loader rule for these is unknown. The
  `zz-walk-errors` probe re-checks that all five walk clean.
- Global scene file anchors (test `zone_camera_route_name_spells_the_hex_zone_prefix_and_decimal_index`): ex1a plays the 1c* routes, ex1b the 2c* routes, mov2 the c1* through c4* routes; each assert's zone id is that route's hex prefix.
- Chamber of Oracles (168) MAPSCHEDULOR keys live in zone 168's own model DAT (ROM/2/11.DAT): the corpus scan's dominant rule (test `zone_scene_resolves_in_the_zones_own_model_dat`).

## Zone static particle emitters (kuluu-render/src/zone_particles.rs)

- A repeated NAME alone is not a repeat: 4,562 corpus-wide sit at distinct
  positions (Manaclipper's g000 under saki/, sira/ and shik/) and every one
  of them runs in retail. The exact-repeat collapse keys on (name, mesh, base
  position), not name alone.

## Event VM (ffxi-event/src/vm.rs)

- `OP_INC` is the loop counter that, while it was only being skipped by
  width, left ~1200 corpus events spinning until the opcode budget killed
  them.
