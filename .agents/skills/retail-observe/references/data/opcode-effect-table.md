## Provenance

opcode effect table - derived by `scripts/opcode-effect-table.py` from this build's generated C;
do not hand-edit: every cell is one of the handler's own instructions, with its VA. One row per
dispatch slot, so an opcode cannot be described as having no effect without that claim being
visible in the instruction list beneath it.

counts: {"slots": 219, "with stores": 133, "with calls": 193, "jump tables": 23, "thunk slots resolved through a jump": 1, "no body found": 0}

store-free and call-free handlers (verified by hand): 0x04, 0x6D

### 00 - f_100afc80 (25 instructions)
- yield byte writes: 100afcde
- stores:
  - `100afc90` [relative] mov word ptr [edx + ecx + 0x26], ax
  - `100afca1` [relative] mov word ptr [edx + ecx + 0x24], 0xff
  - `100afcb4` [relative] mov dword ptr [edx + ecx + 0x3c], 0xbf800000
  - `100afccb` [relative] mov dword ptr [edx + ecx], eax
  - `100afcda` [relative] mov byte ptr [edx + ecx + 0x3a], al
  - `100afcde` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### 01 - f_100afcf0 (7 instructions)
- exec set (jump form): 100afcfa:=ax
- stores: none
- calls: 100afcf5->f_100afa90

### 02 - f_100afd10 (131 instructions)
- advance: 100afda5:8, 100afdc8:8, 100afdef:8, 100afe16:8, 100afe7e:8
- exec set (jump form): 100afd68:=ax
- jump table at: 100afd3c
- stores: none
- calls: 100afd16->f_100af4b0, 100afd1f->f_100af4b0, 100afd47->f_100af4b0, 100afd52->f_100af4b0, 100afd63->f_100afa90, 100afd76->f_100af4b0, 100afd81->f_100af4b0, 100afd91->f_100af4b0, 100afd9c->f_100af4b0, 100afdb4->f_100af4b0, 100afdbf->f_100af4b0, 100afdd7->f_100af4b0, 100afde2->f_100af4b0, 100afdfe->f_100af4b0, 100afe09->f_100af4b0, 100afe25->f_100af4b0, 100afe30->f_100af4b0, 100afe3d->f_100af4b0, 100afe48->f_100af4b0, 100afe55->f_100af4b0, 100afe64->f_100af4b0, 100afe71->f_100af4b0

### 03 - f_100afec0 (11 instructions)
- advance: 100afed4:5
- stores: none
- calls: 100afec5->f_100af4b0, 100afecf->f_100af310

### 04 - f_100b0120 (2 instructions)
- advance: 100b0120:3
- stores: none
- calls: none

### 05 - f_100b0130 (8 instructions)
- advance: 100b013c:3
- stores: none
- calls: 100b0137->f_100af310

### 06 - f_100b0150 (8 instructions)
- advance: 100b015c:3
- stores: none
- calls: 100b0157->f_100af310

### 07 - f_100b0170 (18 instructions)
- advance: 100b0192:5
- stores: none
- calls: 100b0176->f_100af4b0, 100b0181->f_100af4b0, 100b018d->f_100af310

### 08 - f_100b01a0 (18 instructions)
- advance: 100b01c2:5
- stores: none
- calls: 100b01a6->f_100af4b0, 100b01b1->f_100af4b0, 100b01bd->f_100af310

### 09 - f_100b01d0 (21 instructions)
- advance: 100b01fb:5
- stores: none
- calls: 100b01d6->f_100af4b0, 100b01e1->f_100af4b0, 100b01f6->f_100af310

### 0A - f_100b0210 (22 instructions)
- advance: 100b023d:5
- stores: none
- calls: 100b0216->f_100af4b0, 100b0221->f_100af4b0, 100b0238->f_100af310

### 0B - f_100b0390 (12 instructions)
- advance: 100b03a5:3
- stores: none
- calls: 100b0395->f_100af4b0, 100b03a0->f_100af310

### 0C - f_100b03b0 (12 instructions)
- advance: 100b03c5:3
- stores: none
- calls: 100b03b5->f_100af4b0, 100b03c0->f_100af310

### 0D - f_100b03d0 (18 instructions)
- advance: 100b03f2:5
- stores: none
- calls: 100b03d6->f_100af4b0, 100b03e1->f_100af4b0, 100b03ed->f_100af310

### 0E - f_100b0400 (18 instructions)
- advance: 100b0422:5
- stores: none
- calls: 100b0406->f_100af4b0, 100b0411->f_100af4b0, 100b041d->f_100af310

### 0F - f_100b0430 (18 instructions)
- advance: 100b0452:5
- stores: none
- calls: 100b0436->f_100af4b0, 100b0441->f_100af4b0, 100b044d->f_100af310

### 10 - f_100b0460 (19 instructions)
- advance: 100b0484:5
- stores: none
- calls: 100b0466->f_100af4b0, 100b0471->f_100af4b0, 100b047f->f_100af310

### 11 - f_100b0490 (19 instructions)
- advance: 100b04b4:5
- stores: none
- calls: 100b0496->f_100af4b0, 100b04a1->f_100af4b0, 100b04af->f_100af310

### 12 - f_100b04c0 (10 instructions)
- advance: 100b04d2:3
- stores: none
- calls: 100b04c3->f_103149e0, 100b04cd->f_100af310

### 13 - f_100b04e0 (20 instructions)
- advance: 100b0504:5
- stores: none
- calls: 100b04e6->f_100af4b0, 100b04f0->f_103149e0, 100b04ff->f_100af310

### 14 - f_100b0590 (18 instructions)
- advance: 100b05b3:5
- stores: none
- calls: 100b0596->f_100af4b0, 100b05a1->f_100af4b0, 100b05ae->f_100af310

### 15 - f_100b05c0 (27 instructions)
- advance: 100b05f3:5
- stores: none
- calls: 100b05c6->f_100af4b0, 100b05d1->f_100af4b0, 100b05ee->f_100af310

### 16 - f_100b0600 (29 instructions)
- advance: 100b064d:7
- stores:
  - `100b060d` [relative] mov dword ptr [esp + 4], eax
  - `100b0628` [relative] mov dword ptr [esp + 4], eax
- calls: 100b0608->f_100af4b0, 100b0623->f_100af4b0, 100b063e->f_10311c6c, 100b0648->f_100af310

### 17 - f_100b0660 (28 instructions)
- advance: 100b06ab:7
- stores:
  - `100b066d` [relative] mov dword ptr [esp + 4], eax
  - `100b0688` [relative] mov dword ptr [esp + 4], eax
- calls: 100b0668->f_100af4b0, 100b0683->f_100af4b0, 100b069c->f_10311c6c, 100b06a6->f_100af310

### 18 - f_100b06c0 (27 instructions)
- advance: 100b070d:7
- stores:
  - `100b06cd` [relative] mov dword ptr [esp + 4], eax
  - `100b06e4` [relative] mov dword ptr [esp + 4], eax
- calls: 100b06c8->f_100af4b0, 100b06dd->f_100af4b0, 100b06fe->f_10311c6c, 100b0708->f_100af310

### 19 - f_100b0720 (21 instructions)
- advance: 100b074a:5
- stores: none
- calls: 100b0726->f_100af4b0, 100b0731->f_100af4b0, 100b073b->f_100af310, 100b0745->f_100af310

### 1A - f_100b0570 (12 instructions)
- stores: none
- calls: 100b0581->f_100afa90, 100b0589->f_100b0510

### 1B - f_100b0760 (33 instructions)
- exec set (jump form): 100b07e5:=dx
- yield byte writes: 100b07c8
- stores:
  - `100b077a` [relative] mov word ptr [eax + ecx + 0x26], dx
  - `100b078b` [relative] mov word ptr [eax + ecx + 0x24], 0xff
  - `100b079e` [relative] mov dword ptr [eax + ecx + 0x3c], 0xbf800000
  - `100b07b5` [relative] mov dword ptr [eax + ecx], edx
  - `100b07c4` [relative] mov byte ptr [eax + ecx + 0x3a], dl
  - `100b07c8` [exec-field] mov byte ptr [ecx + 0x25a], 1
  - `100b07d1` [relative] mov word ptr [ecx + 0x244], ax
- calls: none

### 1C - f_100b0870 (44 instructions)
- advance: 100b08fd:3
- yield byte writes: 100b08ef
- stores:
  - `100b089a` [relative] mov dword ptr [esp + 4], eax
  - `100b08ef` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b0893->f_100af4b0, 100b08bf->f_10014cf0

### 1D - f_100b2100 (71 instructions)
- advance: 100b21e9:3
- stores:
  - `100b2108` [global] mov byte ptr [0x10488208], 1
  - `100b2113` [relative] mov byte ptr [esp + 4], 0x96
  - `100b2118` [relative] mov byte ptr [esp + 5], 2
  - `100b211d` [relative] mov byte ptr [esp + 6], 0
  - `100b2169` [global] mov word ptr [0x10482f40], ax
  - `100b2173` [global] mov word ptr [0x104855aa], cx
  - `100b21e2` [relative] mov word ptr [edx + 0x164], ax
- calls: 100b215d->f_10312031, 100b21ba->f_100af4b0, 100b21c2->f_100964a0, 100b21d0->f_100ad5e0

### 1E - f_100b2d20 (54 instructions)
- advance: 100b2dc3:5
- stores: none
- calls: 100b2d34->f_100afac0, 100b2d3c->f_100afb00, 100b2db2->f_100afac0, 100b2dbe->f_100b8810

### 1F - f_100b2ea0 (205 instructions)
- advance: 100b3060:2, 100b312e:8
- yield byte writes: 100b308a
- stores:
  - `100b2f4a` [relative] mov dword ptr [esp + 0x14], eax
  - `100b2f96` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b2f9a` [relative] mov dword ptr [esp + 0x14], edx
  - `100b2fd9` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b2fdd` [relative] mov dword ptr [esp + 0x18], edx
  - `100b308a` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b3098` [relative] or dword ptr [eax + 0x124], 0x20000
  - `100b30c2` [relative] mov dword ptr [esp + 0x18], eax
  - `100b30e9` [relative] mov dword ptr [esp + 0x18], eax
  - `100b3110` [relative] mov dword ptr [esp + 0x18], eax
  - `100b3190` [relative] mov dword ptr [esp + 0x14], edx
  - `100b3194` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b319e` [relative] mov dword ptr [esp + 0x10], eax
- calls: 100b2f4e->f_10014cf0, 100b2f9e->f_10014cf0, 100b2fe1->f_10014cf0, 100b30b4->f_1008af20, 100b30bd->f_100af4b0, 100b30e4->f_100af4b0, 100b310b->f_100af4b0, 100b31bb->f_10181290, 100b31cb->f_10181300

### 20 - f_100b34c0 (7 instructions)
- advance: 100b34d5:2
- stores:
  - `100b34d0` [global] mov byte ptr [0x10482f18], al
- calls: none

### 21 - f_100b34e0 (8 instructions)
- yield byte writes: 100b34f6
- stores:
  - `100b34ea` [global] mov byte ptr [0x104804c2], al
  - `100b34f1` [global] mov byte ptr [0x104808f0], al
  - `100b34f6` [exec-field] mov byte ptr [ecx + 0x25a], al
- calls: none

### 22 - f_100b3500 (16 instructions)
- advance: 100b352a:2
- stores: none
- calls: 100b3525->f_1008e4b0

### 23 - f_100b2dd0 (53 instructions)
- advance: 100b2e11:1
- yield byte writes: 100b2dfa, 100b2e02
- stores:
  - `100b2deb` [global] mov byte ptr [0x104804c2], al
  - `100b2df0` [global] mov byte ptr [0x104808f0], al
  - `100b2df5` [global] mov byte ptr [0x10487034], al
  - `100b2dfa` [exec-field] mov byte ptr [esi + 0x25a], al
  - `100b2e02` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b2e35` [relative] mov word ptr [eax + 0x14e], 0
  - `100b2e8a` [relative] mov word ptr [edx + 0x164], 0
- calls: 100b2e73->f_1002c8f0, 100b2e7e->f_100ce160

### 24 - f_100b2270 (176 instructions) [thunk] f_100b2270 -> f_100b25f0
- advance: 100b2824:7
- yield byte writes: 100b2656, 100b267d
- jump table at: 100b2706, 100b27aa
- stores:
  - `100b260c` [relative] mov byte ptr [esp + 8], 0x99
  - `100b2611` [relative] mov byte ptr [esp + 9], 2
  - `100b2616` [relative] mov byte ptr [esp + 0xa], 0
  - `100b2633` [global] mov word ptr [0x10482f40], cx
  - `100b263a` [global] mov word ptr [0x104855aa], cx
  - `100b264f` [global] mov byte ptr [0x104897f4], 1
  - `100b2656` [exec-field] mov byte ptr [ebp + 0x25a], 1
  - `100b2674` [global] mov dword ptr [0x1047d3e8], eax
  - `100b267d` [exec-field] mov byte ptr [ebp + 0x25a], 1
  - `100b26a5` [relative] mov dword ptr [esp + 0x1c], esi
  - `100b26a9` [relative] mov dword ptr [esp + 0x18], esi
  - `100b26b6` [relative] mov dword ptr [esp + 0x28], eax
  - `100b26ca` [relative] mov dword ptr [esp + 0x20], eax
  - `100b26ce` [relative] mov dword ptr [esp + 0x1c], 1
  - `100b26dc` [relative] mov byte ptr [edi], al
  - `100b26e3` [relative] mov byte ptr [edi], al
  - `100b270f` [relative] mov byte ptr [edi], 0
  - `100b2764` [relative] inc dword ptr [esp + 0x18]
  - `100b2778` [relative] mov dword ptr [esp + 0x14], edx
  - `100b277c` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b2780` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b27cb` [relative] mov byte ptr [edi], 0
  - `100b281d` [global] mov byte ptr [0x1047d3ec], 1
- calls: 100b261b->f_100ae3a0, 100b2624->f_100af4b0, 100b262c->f_100964a0, 100b2647->f_100ad750, 100b266d->f_1015eaa0, 100b269c->f_1014e210, 100b26ad->f_100af4b0, 100b26c5->f_100af4b0, 100b271f->f_1014df50, 100b2741->f_1014df90, 100b275f->f_1014e0f0, 100b27db->f_1014df50, 100b27f9->f_1014df90, 100b2817->f_1014e0f0

### 25 - f_100b2940 (79 instructions)
- advance: 100b2990:1, 100b2a48:1
- yield byte writes: 100b29df, 100b2a18, 100b2a4f
- stores:
  - `100b299d` [global] mov dword ptr [0x104887f8], 0xfe
  - `100b29cd` [global] mov byte ptr [0x104804c2], bl
  - `100b29d3` [global] mov byte ptr [0x104808f0], bl
  - `100b29d9` [global] mov byte ptr [0x10487034], bl
  - `100b29df` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b29ed` [global] mov dword ptr [0x104887f8], eax
  - `100b2a18` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b2a4f` [exec-field] mov byte ptr [esi + 0x25a], bl
- calls: 100b2958->f_1014e0a0, 100b297d->f_1014e1c0, 100b29a7->f_100ae3a0, 100b29ac->f_100b2a60, 100b29c1->f_1015e790, 100b29c8->f_1013bf40, 100b29f2->f_100ae3a0, 100b29f7->f_100b2a60, 100b2a0c->f_1015e790, 100b2a13->f_1013bf40, 100b2a22->f_100b2a60, 100b2a3c->f_1015e790, 100b2a43->f_1013bf40

### 26 - f_100afc70 (2 instructions)
- yield byte writes: 100afc70
- stores:
  - `100afc70` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### 27 - f_100b3930 (41 instructions)
- advance: 100b3991:7
- yield byte writes: 100b3985
- stores:
  - `100b3985` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b3944->f_100afac0, 100b394c->f_100afb00, 100b3978->f_100b39a0

### 28 - f_100b3e50 (131 instructions)
- advance: 100b3fbb:7
- yield byte writes: 100b3f4a, 100b3fd9
- stores:
  - `100b3f26` [relative] mov dword ptr [esp + 0x10], ebx
  - `100b3f4a` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b3fb6` [relative] mov byte ptr [edx + esi + 0x3b], 0
  - `100b3fd5` [relative] mov byte ptr [ecx + esi + 0x3b], bl
  - `100b3fd9` [exec-field] mov byte ptr [esi + 0x25a], bl
- calls: 100b3e87->f_100afac0, 100b3e8f->f_100afb00, 100b3ea3->f_100b3660, 100b3eb9->f_100b3660, 100b3f38->f_100b36d0, 100b3f65->f_100afac0, 100b3f6d->f_100afb00, 100b3f9b->f_100b39a0

### 29 - f_100b3ff0 (200 instructions)
- advance: 100b424e:7
- yield byte writes: 100b40f1, 100b41dc, 100b426c
- stores:
  - `100b40cd` [relative] mov dword ptr [esp + 0x10], ebx
  - `100b40f1` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b41a7` [relative] mov dword ptr [esp + 0x10], ebx
  - `100b41d7` [relative] mov byte ptr [ecx + esi + 0x3b], 2
  - `100b41dc` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b4249` [relative] mov byte ptr [eax + esi + 0x3b], 0
  - `100b4268` [relative] mov byte ptr [edx + esi + 0x3b], bl
  - `100b426c` [exec-field] mov byte ptr [esi + 0x25a], bl
- calls: 100b402e->f_100afac0, 100b4036->f_100afb00, 100b404a->f_100b3660, 100b4060->f_100b3660, 100b40df->f_100b36d0, 100b410c->f_100afac0, 100b4114->f_100afb00, 100b4128->f_100b3660, 100b413e->f_100b3660, 100b41b9->f_100b36d0, 100b41f7->f_100afac0, 100b41ff->f_100afb00, 100b422e->f_100b39a0

### 2A - f_100b4280 (69 instructions)
- advance: 100b433a:6
- yield byte writes: 100b432d
- stores:
  - `100b432d` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b4295->f_100afac0, 100b429d->f_100afb00, 100b42af->f_100b3660, 100b42ba->f_100b3660, 100b4323->f_100b3690

### 2B - f_100b1e80 (85 instructions)
- advance: 100b1f8e:7
- stores:
  - `100b1e96` [relative] mov byte ptr [esp + 0xc], 0x96
  - `100b1e9b` [relative] mov byte ptr [esp + 0xd], 2
  - `100b1ea0` [relative] mov byte ptr [esp + 0xe], 0
  - `100b1ea9` [relative] mov word ptr [eax + 0x164], 0
  - `100b1ecd` [global] mov byte ptr [0x10488208], al
  - `100b1f28` [global] mov word ptr [0x10482f40], di
  - `100b1f2f` [global] mov word ptr [0x104855aa], di
  - `100b1f85` [relative] mov word ptr [edx + 0x164], di
- calls: 100b1eb2->f_100afac0, 100b1eba->f_100afb00, 100b1f1b->f_10312031, 100b1f61->f_100af4b0, 100b1f69->f_100964a0, 100b1f77->f_100ad5e0

### 2C - f_100b4c20 (75 instructions)
- advance: 100b4cf9:0xd
- stores:
  - `100b4ca2` [relative] mov dword ptr [esp + 0x10], edx
  - `100b4ca9` [relative] mov dword ptr [esp + 0x10], esi
- calls: 100b4c36->f_100afac0, 100b4c3e->f_100afb00, 100b4c59->f_100afac0, 100b4c61->f_100afb00, 100b4ceb->f_100afac0

### 2D - f_100b4f10 (76 instructions)
- advance: 100b4feb:0xd
- stores:
  - `100b4f92` [relative] mov dword ptr [esp + 0x10], edx
  - `100b4f99` [relative] mov dword ptr [esp + 0x10], edi
- calls: 100b4f26->f_100afac0, 100b4f2e->f_100afb00, 100b4f49->f_100afac0, 100b4f51->f_100afb00, 100b4fe0->f_100afac0

### 2E - f_100b55d0 (11 instructions)
- advance: 100b55e0:1, 100b55f4:1
- stores:
  - `100b55d9` [global] mov byte ptr [0x104886b8], 1
  - `100b55ea` [global] mov byte ptr [0x104886b8], al
  - `100b55ef` [global] mov byte ptr [0x10485a70], al
- calls: none

### 2F - f_100b35e0 (37 instructions)
- advance: 100b3647:6
- stores:
  - `100b3641` [relative] mov dword ptr [eax + 0x120], ecx
- calls: 100b35f4->f_100afac0, 100b35fc->f_100afb00

### 30 - f_100b5620 (3 instructions)
- advance: 100b5627:1
- stores:
  - `100b5620` [global] mov byte ptr [0x10488100], 0
- calls: none

### 31 - f_100b5630 (237 instructions)
- advance: 100b5766:2, 100b594c:0xa
- yield byte writes: 100b57b0
- stores:
  - `100b56ee` [relative] mov dword ptr [esp + 0x14], eax
  - `100b577f` [relative] mov dword ptr [esp + 0x18], edx
  - `100b57a7` [relative] mov word ptr [ecx + esi + 0x38], ax
  - `100b57b0` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b57be` [relative] or dword ptr [eax + 0x124], 0x20000
  - `100b57f5` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b57f9` [relative] mov dword ptr [esp + 0x14], edx
  - `100b5838` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b583c` [relative] mov dword ptr [esp + 0x18], edx
  - `100b5875` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b5879` [relative] mov dword ptr [esp + 0x18], edx
  - `100b58c6` [relative] mov dword ptr [esp + 0x18], eax
  - `100b58ed` [relative] mov dword ptr [esp + 0x18], eax
  - `100b5914` [relative] mov dword ptr [esp + 0x18], eax
  - `100b5947` [relative] mov word ptr [ecx + esi + 0x38], ax
  - `100b59ae` [relative] mov dword ptr [esp + 0x14], edx
  - `100b59b2` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b59bc` [relative] mov dword ptr [esp + 0x10], eax
- calls: 100b56f2->f_10014cf0, 100b578b->f_10014cf0, 100b5794->f_10311c6c, 100b57fd->f_10014cf0, 100b5840->f_10014cf0, 100b587d->f_10014cf0, 100b58b8->f_1008af20, 100b58c1->f_100af4b0, 100b58e8->f_100af4b0, 100b590f->f_100af4b0, 100b5936->f_100af4b0, 100b59d9->f_10181290, 100b59e9->f_10181300

### 32 - f_100b5a10 (14 instructions)
- advance: 100b5a35:3
- stores:
  - `100b5a1b` [relative] mov dword ptr [esp + 4], eax
- calls: 100b5a16->f_100af4b0

### 33 - f_100b5a40 (22 instructions)
- advance: 100b5a80:2
- stores:
  - `100b5a7a` [relative] mov dword ptr [eax + 0x120], edx
- calls: none

### 34 - f_100b5a90 (57 instructions)
- advance: 100b5b0a:3
- yield byte writes: 100b5ab9, 100b5b12, 100b5b58
- stores:
  - `100b5aa9` [global] mov dword ptr [0x10489910], 0x12c
  - `100b5ab3` [global] mov byte ptr [0x104898f9], bl
  - `100b5ab9` [exec-field] mov byte ptr [edi + 0x25a], bl
  - `100b5ac7` [global] mov dword ptr [0x10489910], 0
  - `100b5afa` [global] mov byte ptr [0x104898f9], bl
  - `100b5b00` [global] mov dword ptr [0x10489918], 0
  - `100b5b12` [exec-field] mov byte ptr [edi + 0x25a], bl
  - `100b5b42` [global] mov dword ptr [0x10489918], 0
  - `100b5b4c` [global] mov dword ptr [0x10489910], eax
  - `100b5b51` [global] mov byte ptr [0x104898f9], bl
  - `100b5b58` [exec-field] mov byte ptr [edi + 0x25a], bl
- calls: 100b5adb->f_1018b620, 100b5ae4->f_100b5c30, 100b5aec->f_100af4b0, 100b5af2->f_1018b3a0, 100b5b27->f_10092900

### 35 - f_100b5b70 (53 instructions)
- advance: 100b5bdd:3
- yield byte writes: 100b5b99, 100b5be5, 100b5c21
- stores:
  - `100b5b89` [global] mov dword ptr [0x10489910], 0x12c
  - `100b5b93` [global] mov byte ptr [0x104898f9], bl
  - `100b5b99` [exec-field] mov byte ptr [edi + 0x25a], bl
  - `100b5ba7` [global] mov dword ptr [0x10489910], 0
  - `100b5bcd` [global] mov byte ptr [0x104898f9], bl
  - `100b5bd3` [global] mov dword ptr [0x10489918], 0
  - `100b5be5` [exec-field] mov byte ptr [edi + 0x25a], bl
  - `100b5c15` [global] mov byte ptr [0x104898f9], bl
  - `100b5c1b` [global] mov dword ptr [0x10489910], eax
  - `100b5c21` [exec-field] mov byte ptr [edi + 0x25a], bl
- calls: 100b5bb7->f_100b5c30, 100b5bbf->f_100af4b0, 100b5bc5->f_1018b3a0, 100b5bfa->f_10092900

### 36 - f_100b5c50 (96 instructions)
- advance: 100b5db4:7
- stores:
  - `100b5c5d` [relative] mov dword ptr [esp + 8], eax
  - `100b5c80` [relative] mov dword ptr [esp + 8], eax
  - `100b5ca3` [relative] mov dword ptr [esp + 8], eax
  - `100b5d15` [relative] mov dword ptr [esp + 0xc], ecx
  - `100b5d1b` [relative] mov dword ptr [esp + 8], edx
  - `100b5d2d` [relative] mov dword ptr [esp + 0x10], edx
  - `100b5d4f` [relative] mov dword ptr [esp + 0x10], edx
  - `100b5d61` [relative] mov dword ptr [esp + 8], edx
- calls: 100b5c58->f_100af4b0, 100b5c7b->f_100af4b0, 100b5c9e->f_100af4b0, 100b5d48->f_10181290, 100b5d77->f_10181300, 100b5d9d->f_10092f90, 100b5daf->f_1008f2a0

### 37 - f_100b5dd0 (105 instructions)
- advance: 100b5f5d:9
- stores:
  - `100b5ddd` [relative] mov dword ptr [esp + 8], eax
  - `100b5e00` [relative] mov dword ptr [esp + 8], eax
  - `100b5e23` [relative] mov dword ptr [esp + 8], eax
  - `100b5e46` [relative] mov dword ptr [esp + 8], eax
  - `100b5ebe` [relative] mov dword ptr [esp + 0xc], ecx
  - `100b5ec4` [relative] mov dword ptr [esp + 8], edx
  - `100b5ed6` [relative] mov dword ptr [esp + 0x10], edx
  - `100b5ef8` [relative] mov dword ptr [esp + 0x10], edx
  - `100b5f0a` [relative] mov dword ptr [esp + 8], edx
- calls: 100b5dd8->f_100af4b0, 100b5dfb->f_100af4b0, 100b5e1e->f_100af4b0, 100b5e41->f_100af4b0, 100b5ef1->f_10181290, 100b5f20->f_10181300, 100b5f46->f_10092f90, 100b5f58->f_1008f2a0

### 38 - f_100b6180 (9 instructions)
- advance: 100b6193:3
- stores:
  - `100b618d` [global] mov word ptr [0x104808fc], ax
- calls: 100b6185->f_100af4b0

### 39 - f_100b6140 (15 instructions)
- advance: 100b616b:3
- stores:
  - `100b614b` [relative] mov dword ptr [esp + 4], eax
- calls: 100b6146->f_100af4b0

### 3A - f_100b2c80 (48 instructions)
- advance: 100b2d12:7
- stores:
  - `100b2cdd` [relative] mov dword ptr [esp + 0x10], ecx
  - `100b2ce6` [relative] mov dword ptr [esp + 0x10], edx
- calls: 100b2c94->f_100afac0, 100b2c9c->f_100afb00, 100b2cef->f_100dd9d0, 100b2d03->f_10311c6c, 100b2d0d->f_100af310

### 3B - f_100b2b90 (63 instructions)
- advance: 100b2c64:0xb
- stores:
  - `100b2bd4` [relative] mov dword ptr [esp + 0x10], eax
  - `100b2bd8` [relative] mov dword ptr [esp + 0x14], eax
  - `100b2c15` [relative] mov dword ptr [esp + 0x10], ecx
  - `100b2c19` [relative] mov dword ptr [esp + 0x14], edx
- calls: 100b2ba4->f_100afac0, 100b2bac->f_100afb00, 100b2c23->f_10311c6c, 100b2c2d->f_100af310, 100b2c3c->f_10311c6c, 100b2c46->f_100af310, 100b2c55->f_10311c6c, 100b2c5f->f_100af310

### 3C - f_100b02a0 (33 instructions)
- advance: 100b02e3:7
- stores: none
- calls: 100b02a7->f_100af4b0, 100b02b7->f_100af4b0, 100b02c5->f_100af4c0, 100b02de->f_100af330

### 3D - f_100b02f0 (34 instructions)
- advance: 100b0335:7
- stores: none
- calls: 100b02f7->f_100af4b0, 100b0307->f_100af4b0, 100b0315->f_100af4c0, 100b0330->f_100af330

### 3E - f_100b0250 (28 instructions)
- advance: 100b027a:7
- exec set (jump form): 100b028e:=ax
- stores: none
- calls: 100b0256->f_100af4b0, 100b0265->f_100af4c0, 100b0289->f_100afa90

### 3F - f_100b61a0 (27 instructions)
- advance: 100b61d3:7
- stores: none
- calls: 100b61a6->f_100af4b0, 100b61b1->f_100af4b0, 100b61ce->f_100af310

### 40 - f_100b61e0 (46 instructions)
- advance: 100b623e:9
- stores: none
- calls: 100b61e8->f_100af4b0, 100b61f3->f_100af4b0, 100b6216->f_100af4b0, 100b6227->f_100af4b0, 100b6239->f_100af310

### 41 - f_100b6250 (36 instructions)
- advance: 100b629a:9
- stores: none
- calls: 100b6257->f_100af4b0, 100b6262->f_100af4b0, 100b6285->f_100af4b0, 100b6295->f_100af310

### 42 - f_100b5600 (8 instructions)
- advance: 100b5616:1
- stores:
  - `100b560a` [global] mov byte ptr [0x104886b8], al
  - `100b5611` [global] mov byte ptr [0x10485a70], al
- calls: none

### 43 - f_100b63a0 (31 instructions)
- advance: 100b63f2:2
- exec set (jump form): 100b63d1:=cx
- yield byte writes: 100b63d8
- stores:
  - `100b63d8` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b63eb` [global] mov byte ptr [0x104855a8], 1
- calls: 100b63e2->f_100a0510

### 44 - f_100b6400 (18 instructions)
- advance: 100b6417:5
- exec set (jump form): 100b642a:=ax
- stores: none
- calls: 100b6405->f_100af4b0, 100b640b->f_100b3660, 100b6425->f_100afa90

### 45 - f_100b44c0 (3 instructions)
- stores: none
- calls: 100b44c5->f_100b4390

### 46 - f_100b6440 (136 instructions)
- advance: 100b650d:2, 100b6534:2, 100b6557:2, 100b655f:2, 100b665c:6, 100b6664:2
- jump table at: 100b6483
- stores:
  - `100b64b3` [relative] mov dword ptr [esp + 0xc], edx
  - `100b64be` [relative] mov dword ptr [esp + 0x10], eax
  - `100b64c2` [relative] mov dword ptr [esp + 8], ecx
  - `100b64d6` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b658f` [global] mov byte ptr [0x104897ea], al
  - `100b6599` [relative] mov dword ptr [esp + 4], eax
  - `100b65b6` [relative] mov dword ptr [esp + 4], eax
  - `100b65d3` [relative] mov dword ptr [esp + 4], eax
  - `100b65d7` [global] mov dword ptr [0x1048992c], 0x3f800000
  - `100b6602` [relative] mov dword ptr [esp + 0xc], eax
  - `100b660d` [relative] mov dword ptr [esp + 0x10], ecx
  - `100b6611` [relative] mov dword ptr [esp + 8], edx
  - `100b6625` [relative] mov dword ptr [esp + 0x14], edx
- calls: 100b6490->f_100ae9b0, 100b6498->f_1003d5c0, 100b649d->f_10084210, 100b64db->f_10015250, 100b64e2->f_1001e780, 100b6501->f_10015250, 100b6508->f_10021680, 100b651c->f_100ae9b0, 100b6524->f_100842d0, 100b652f->f_10221c30, 100b6552->f_100af310, 100b656e->f_100ae9b0, 100b6576->f_1003d5c0, 100b657b->f_10084210, 100b6594->f_100af4b0, 100b65b1->f_100af4b0, 100b65ce->f_100af4b0, 100b662a->f_10015250, 100b6631->f_1001e780, 100b6650->f_10015250, 100b6657->f_10021680

### 47 - f_100b62b0 (73 instructions)
- advance: 100b638b:0xa
- exec set (jump form): 100b62eb:=cx
- yield byte writes: 100b62f2
- stores:
  - `100b62f2` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b6306` [relative] mov dword ptr [esp + 8], eax
  - `100b6327` [relative] mov dword ptr [esp + 0xc], eax
  - `100b6342` [relative] mov dword ptr [esp + 0x10], eax
  - `100b635d` [relative] mov dword ptr [esp + 0x14], eax
  - `100b6381` [global] mov byte ptr [0x104855a8], al
  - `100b6386` [global] mov byte ptr [0x1047faaf], al
- calls: 100b6301->f_100af4b0, 100b6322->f_100af4b0, 100b633d->f_100af4b0, 100b6358->f_100af4b0, 100b636f->f_100a05e0

### 48 - f_100b2200 (33 instructions)
- advance: 100b225f:3
- stores:
  - `100b2206` [global] mov byte ptr [0x10488208], 1
  - `100b221a` [relative] mov byte ptr [esp + 0x10], 0x97
  - `100b221f` [relative] mov byte ptr [esp + 0x11], 3
  - `100b2224` [relative] mov byte ptr [esp + 0x12], al
  - `100b2229` [relative] mov word ptr [ecx + 0x164], ax
  - `100b2235` [global] mov word ptr [0x10482f40], ax
  - `100b223b` [global] mov word ptr [0x104855aa], ax
- calls: 100b2241->f_100af4b0, 100b2249->f_100964a0, 100b2257->f_100ad660

### 49 - f_100b1de0 (45 instructions)
- advance: 100b1e68:7
- stores:
  - `100b1df6` [relative] mov byte ptr [esp + 0x10], 0x96
  - `100b1dfb` [relative] mov byte ptr [esp + 0x11], 2
  - `100b1e00` [relative] mov byte ptr [esp + 0x12], 0
  - `100b1e09` [relative] mov word ptr [eax + 0x164], 0
  - `100b1e23` [global] mov byte ptr [0x10488208], al
  - `100b1e2d` [global] mov word ptr [0x10482f40], ax
  - `100b1e33` [global] mov word ptr [0x104855aa], ax
- calls: 100b1e12->f_100afac0, 100b1e1a->f_100afb00, 100b1e4a->f_100af4b0, 100b1e52->f_100964a0, 100b1e60->f_100ad660

### 4A - f_100b6700 (102 instructions)
- advance: 100b6852:9
- stores:
  - `100b67fc` [relative] mov dword ptr [ecx + 0x150], ebx
  - `100b6814` [relative] mov dword ptr [ecx + 0x158], ebx
  - `100b6824` [relative] mov dword ptr [edx + 0x15c], eax
  - `100b682f` [relative] mov dword ptr [eax + 0x14], ebx
  - `100b6832` [relative] mov dword ptr [eax + 0x1c], ebx
- calls: 100b6716->f_100afac0, 100b671e->f_100afb00, 100b6739->f_100afac0, 100b6741->f_100afb00, 100b683b->f_100afac0, 100b6845->f_100afac0, 100b684d->f_100b8810

### 4B - f_100b6870 (61 instructions)
- advance: 100b6921:7, 100b6938:7
- stores:
  - `100b68c2` [relative] mov dword ptr [esp + 0xc], eax
  - `100b68f3` [relative] mov dword ptr [edx + 0x150], ebx
  - `100b690f` [relative] mov dword ptr [edx + 0x158], ebx
  - `100b691b` [relative] mov dword ptr [eax + 0x15c], ecx
  - `100b6932` [relative] mov dword ptr [eax + 0x14], ebx
  - `100b6935` [relative] mov dword ptr [eax + 0x1c], ebx
- calls: 100b6886->f_100afac0, 100b688e->f_100afb00, 100b68bd->f_100af4b0

### 4C - f_100b6950 (10 instructions)
- advance: 100b6974:1
- stores:
  - `100b696a` [relative] mov dword ptr [eax + 0x174], 8
- calls: none

### 4D - f_100b6a00 (10 instructions)
- advance: 100b6a24:1
- stores:
  - `100b6a1a` [relative] mov dword ptr [eax + 0x174], 9
- calls: none

### 4E - f_100b3580 (30 instructions)
- advance: 100b35d0:6
- stores: none
- calls: 100b3594->f_100afac0, 100b359c->f_100afb00, 100b35cb->f_1008e4b0

### 4F - f_100b69b0 (20 instructions)
- advance: 100b69ec:3
- stores:
  - `100b69e6` [relative] mov dword ptr [ecx + 0x174], eax
- calls: 100b69c8->f_100af4b0

### 50 - f_100b4d10 (74 instructions)
- advance: 100b4de7:0xd
- stores:
  - `100b4d92` [relative] mov dword ptr [esp + 0x10], edx
  - `100b4d99` [relative] mov dword ptr [esp + 0x10], esi
- calls: 100b4d26->f_100afac0, 100b4d2e->f_100afb00, 100b4d49->f_100afac0, 100b4d51->f_100afb00, 100b4dd9->f_100afac0

### 51 - f_100b5000 (76 instructions)
- advance: 100b50db:0xd
- stores:
  - `100b5082` [relative] mov dword ptr [esp + 0x10], edx
  - `100b5089` [relative] mov dword ptr [esp + 0x10], edi
- calls: 100b5016->f_100afac0, 100b501e->f_100afb00, 100b5039->f_100afac0, 100b5041->f_100afb00, 100b50d0->f_100afac0

### 52 - f_100b48d0 (3 instructions)
- stores: none
- calls: 100b48d5->f_100b49d0

### 53 - f_100b4e00 (4 instructions)
- stores: none
- calls: 100b4e04->f_100b4e10

### 54 - f_100b50f0 (83 instructions)
- advance: 100b51de:0xd
- yield byte writes: 100b51d1
- stores:
  - `100b5176` [relative] mov dword ptr [esp + 0x10], eax
  - `100b517d` [relative] mov dword ptr [esp + 0x10], ecx
  - `100b51d1` [exec-field] mov byte ptr [ebx + 0x25a], 1
- calls: 100b5106->f_100afac0, 100b510e->f_100afb00, 100b5129->f_100afac0, 100b5131->f_100afb00, 100b51ad->f_100afac0

### 55 - f_100b4950 (3 instructions)
- stores: none
- calls: 100b4955->f_100b4af0

### 56 - f_100b66c0 (18 instructions)
- advance: 100b66e1:5
- yield byte writes: 100b66e9
- stores:
  - `100b66e9` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b66d4->f_100afac0, 100b66dc->f_100afb00

### 57 - f_100b0350 (17 instructions)
- advance: 100b0373:3
- stores: none
- calls: 100b0356->f_100af4b0, 100b035d->f_10014cf0, 100b0362->f_10311c6c, 100b036e->f_100af310

### 58 - f_100b0380 (3 instructions)
- advance: 100b0380:1
- yield byte writes: 100b0387
- stores:
  - `100b0387` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### 59 - f_100b6a60 (268 instructions)
- advance: 100b6abd:4, 100b6b21:bx, 100b6b5b:4, 100b6bbf:bx, 100b6c3a:bx, 100b6ca8:7, 100b6d1a:6, 100b6d56:4, 100b6db1:bx
- yield byte writes: 100b6d0d
- jump table at: 100b6a89
- stores:
  - `100b6ab7` [relative] mov dword ptr [edx + 0xdc], eax
  - `100b6b1b` [relative] mov dword ptr [ecx + 0xdc], eax
  - `100b6b55` [relative] mov dword ptr [edx + 0xe0], eax
  - `100b6bb9` [relative] mov dword ptr [ecx + 0xe0], eax
  - `100b6c25` [relative] mov dword ptr [esp + 0xc], eax
  - `100b6ca2` [relative] mov dword ptr [eax + 0x120], ecx
  - `100b6d0d` [exec-field] mov byte ptr [esi + 0x25a], al
- calls: 100b6aa5->f_100af4b0, 100b6ada->f_100afac0, 100b6ae2->f_100afb00, 100b6b13->f_100af4b0, 100b6b43->f_100af4b0, 100b6b78->f_100afac0, 100b6b80->f_100afb00, 100b6bb1->f_100af4b0, 100b6bda->f_100afac0, 100b6be2->f_100afb00, 100b6c1a->f_100af4b0, 100b6c55->f_100afac0, 100b6c5d->f_100afb00, 100b6cc5->f_100afac0, 100b6ccd->f_100afb00, 100b6d3e->f_100af4b0, 100b6d51->f_1008a790, 100b6d73->f_100afac0, 100b6d7b->f_100afb00, 100b6da4->f_100af4b0, 100b6dac->f_1008a790

### 5A - f_100b31f0 (168 instructions)
- advance: 100b33f5:2, 100b34af:8
- yield byte writes: 100b341c
- stores:
  - `100b32a4` [relative] mov dword ptr [esp + 0x14], eax
  - `100b32e4` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b32e8` [relative] mov dword ptr [esp + 0x14], edx
  - `100b332b` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b332f` [relative] mov dword ptr [esp + 0x18], edx
  - `100b336e` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b3372` [relative] mov dword ptr [esp + 0x18], edx
  - `100b3416` [relative] mov dword ptr [eax + 0x124], ecx
  - `100b341c` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b3443` [relative] mov dword ptr [esp + 0x18], eax
  - `100b346a` [relative] mov dword ptr [esp + 0x18], eax
  - `100b3491` [relative] mov dword ptr [esp + 0x18], eax
- calls: 100b32a8->f_10014cf0, 100b32f0->f_10014cf0, 100b3333->f_10014cf0, 100b3376->f_10014cf0, 100b3435->f_1008af20, 100b343e->f_100af4b0, 100b3465->f_100af4b0, 100b348c->f_100af4b0

### 5B - f_100b51f0 (5 instructions)
- stores: none
- calls: 100b51f6->f_100b5210

### 5C - f_100b6e30 (97 instructions)
- advance: 100b6e9a:4, 100b6ee2:6, 100b6f09:6, 100b6f31:4, 100b6f66:2
- jump table at: 100b6e6a
- stores:
  - `100b6e7f` [relative] mov word ptr [edi*2 + 0x10485fe4], ax
  - `100b6e87` [relative] mov word ptr [edi*2 + 0x1048558c], 0x7f
  - `100b6e91` [global] mov word ptr [0x10482f1c], 0xffff
  - `100b6eb7` [relative] mov word ptr [edi*2 + 0x10485fe4], ax
  - `100b6ecf` [relative] mov word ptr [ecx], ax
  - `100b6ed4` [relative] mov word ptr [ecx], 0xff
  - `100b6ed9` [global] mov word ptr [0x10482f1c], 0xffff
- calls: 100b6e7a->f_100af4b0, 100b6eae->f_100af4b0, 100b6ebf->f_100af4b0, 100b6ef1->f_100af4b0, 100b6efb->f_100af4b0, 100b6f01->f_10037a30, 100b6f22->f_10246710, 100b6f2c->f_100af310, 100b6f50->f_10246710, 100b6f5e->f_100378b0

### 5D - f_100b7040 (14 instructions)
- advance: 100b705a:5
- stores: none
- calls: 100b7045->f_100af4b0, 100b704f->f_100af4b0, 100b7055->f_10037a30

### 5E - f_100b71e0 (53 instructions)
- advance: 100b72b1:5
- stores:
  - `100b722a` [relative] mov dword ptr [ecx + 0x1a4], edi
  - `100b7278` [relative] mov dword ptr [ecx + 0x7c8], edi
  - `100b7291` [relative] mov dword ptr [ecx + 0x7d8], 0x20202020
- calls: 100b720b->f_10087d40, 100b7214->f_100afac0, 100b723b->f_1008da00, 100b7258->f_1002c8f0, 100b72ac->f_100d5430

### 5F - f_100b72c0 (79 instructions)
- advance: 100b731d:2
- exec set (jump form): 100b732f:=dx, 100b7348:=ax, 100b735b:=cx, 100b737a:=dx, 100b7393:=ax, 100b73a6:=cx, 100b73ba:=di
- jump table at: 100b72e9
- stores:
  - `100b7317` [relative] mov dword ptr [eax + 0x124], edx
- calls: 100b7336->f_100b5510, 100b734f->f_100b5210, 100b7368->f_100b5210, 100b7381->f_100b5210, 100b739a->f_100b5210, 100b73b1->f_100b4e10

### 60 - f_100b73f0 (51 instructions)
- advance: 100b7432:ax, 100b7470:ax, 100b747a:cx
- stores:
  - `100b7465` [relative] mov dword ptr [ecx + 0x124], eax
- calls: 100b7421->f_100afac0

### 61 - f_100b74e0 (18 instructions)
- advance: 100b7517:2
- stores:
  - `100b7511` [relative] mov dword ptr [eax + 0x128], edx
- calls: none

### 62 - f_100b44d0 (3 instructions)
- stores: none
- calls: 100b44d5->f_100b4390

### 63 - f_100b7550 (27 instructions)
- advance: 100b759f:3
- yield byte writes: 100b75a9
- stores:
  - `100b7584` [relative] mov word ptr [edx + 0x1c0], ax
  - `100b7598` [relative] mov byte ptr [ecx + 0x1bc], 9
  - `100b75a9` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7572->f_100af4b0

### 64 - f_100b75c0 (40 instructions)
- advance: 100b761b:0xb
- stores:
  - `100b75fa` [relative] mov dword ptr [esp + 0xc], eax
  - `100b7602` [relative] mov dword ptr [esp + 0xc], ecx
- calls: 100b75c8->f_100af4b0, 100b75d3->f_100af4b0, 100b75de->f_100af4b0, 100b75e9->f_100af4b0, 100b760c->f_10311c6c, 100b7616->f_100af310

### 65 - f_100b7860 (97 instructions)
- advance: 100b79a6:0xb
- stores:
  - `100b791b` [relative] mov dword ptr [esp + 0x10], edx
  - `100b7944` [relative] mov dword ptr [esp + 0x28], edx
  - `100b7953` [relative] mov dword ptr [esp + 0x28], eax
- calls: 100b7874->f_100afac0, 100b787c->f_100afb00, 100b7897->f_100afac0, 100b789f->f_100afb00, 100b7991->f_10311c6c, 100b79a1->f_100af310

### 66 - f_100b5200 (5 instructions)
- stores: none
- calls: 100b5206->f_100b5210

### 67 - f_100b79c0 (33 instructions)
- advance: 100b7a14:5
- stores:
  - `100b7a0d` [global] mov byte ptr [0x10487fc9], 1
- calls: 100b79ce->f_100af4b0, 100b79d8->f_100af4b0, 100b79e2->f_10136870, 100b79f4->f_100af4b0, 100b79fe->f_100af4b0, 100b7a08->f_10136870

### 68 - f_100b7a20 (18 instructions)
- advance: 100b7a51:1
- stores:
  - `100b7a4a` [global] mov byte ptr [0x10487fc9], 0
- calls: 100b7a31->f_10136870, 100b7a45->f_10136870

### 69 - f_100b7a60 (48 instructions)
- advance: 100b7ae2:4
- stores:
  - `100b7a74` [relative] mov dword ptr [esp + 0xc], 0x3f800000
  - `100b7a8b` [relative] mov dword ptr [esp + 0xc], 0
  - `100b7add` [global] mov dword ptr [0x10351710], eax
- calls: 100b7a68->f_100af4b0, 100b7a9d->f_10036020, 100b7aab->f_10036070, 100b7ab9->f_10035f80, 100b7ac7->f_10035f00, 100b7ad8->f_10311c6c

### 6A - f_100b7af0 (76 instructions)
- advance: 100b7bc6:7
- stores:
  - `100b7b16` [relative] mov dword ptr [esp + 0xc], eax
  - `100b7b48` [relative] mov dword ptr [esp + 0xc], eax
  - `100b7b7a` [relative] mov dword ptr [esp + 0xc], eax
  - `100b7bac` [relative] mov dword ptr [esp + 0xc], eax
- calls: 100b7af7->f_100af4b0, 100b7b07->f_100af4b0, 100b7b11->f_100af4b0, 100b7b28->f_10035d40, 100b7b39->f_100af4b0, 100b7b43->f_100af4b0, 100b7b5a->f_10035d90, 100b7b6b->f_100af4b0, 100b7b75->f_100af4b0, 100b7b8c->f_10035de0, 100b7b9d->f_100af4b0, 100b7ba7->f_100af4b0, 100b7bbe->f_10035bc0

### 6B - f_100b7070 (58 instructions)
- advance: 100b7128:9
- stores:
  - `100b70d6` [relative] mov dword ptr [ecx + 0x1a4], edi
  - `100b7103` [relative] mov dword ptr [ecx + 0x7c8], edi
  - `100b7111` [relative] mov dword ptr [eax + 0x7d8], 0x20202020
- calls: 100b7086->f_100afac0, 100b708e->f_100afb00, 100b70c4->f_10087d40, 100b70cd->f_100afac0, 100b70de->f_1008da00, 100b70f0->f_1002c8f0, 100b7123->f_100d5430

### 6C - f_100b7630 (137 instructions)
- advance: 100b7702:9
- yield byte writes: 100b777e
- stores:
  - `100b76b6` [relative] mov word ptr [edx + 0x176], di
  - `100b76fb` [relative] mov byte ptr [ecx + 0x162], 0
  - `100b771d` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b7721` [relative] mov dword ptr [esp + 0x14], edx
  - `100b772f` [relative] mov dword ptr [esp + 0x10], eax
  - `100b777e` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b77a0` [relative] mov dword ptr [ecx + 0x170], eax
  - `100b77b7` [relative] mov byte ptr [edx + 0x174], al
  - `100b77c8` [relative] mov word ptr [ecx + 0x176], ax
  - `100b77df` [relative] mov word ptr [eax + 0x176], bx
  - `100b77f7` [relative] mov dword ptr [esp + 0x14], edx
  - `100b7818` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b7820` [relative] mov dword ptr [esp + 0x14], edx
  - `100b783c` [relative] mov byte ptr [eax + 0x162], bl
- calls: 100b764b->f_100afac0, 100b7653->f_100afb00, 100b76a4->f_10014cf0, 100b76a9->f_10311c6c, 100b7725->f_10014cf0, 100b772a->f_10311c6c, 100b7762->f_10311c6c, 100b77a8->f_100af4b0, 100b77bd->f_100af4b0

### 6D - f_100b7850 (2 instructions)
- advance: 100b7850:7
- stores: none
- calls: none

### 6E - f_100b7c40 (52 instructions)
- advance: 100b7cb7:7
- yield byte writes: 100b7cc5
- stores:
  - `100b7cc5` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7c55->f_100afac0, 100b7c5d->f_100afb00, 100b7c99->f_100af4b0, 100b7caf->f_100a0b60

### 6F - f_100b07f0 (36 instructions)
- advance: 100b0867:1
- yield byte writes: 100b0859
- stores:
  - `100b0814` [relative] mov dword ptr [ecx], 0x41800000
  - `100b0859` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b0829->f_10014cf0

### 70 - f_100b7dd0 (18 instructions)
- advance: 100b7dff:1
- yield byte writes: 100b7df1
- stores:
  - `100b7df1` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7dfa->f_1008af20

### 71 - f_100b7e10 (356 instructions)
- advance: 100b7e6d:2, 100b7eec:2, 100b806c:6, 100b80e3:8, 100b812e:2, 100b8181:2, 100b82d5:0xa, 100b8308:4
- exec set (jump form): 100b7f19:=ax
- yield byte writes: 100b7e75, 100b7ef4, 100b7f12, 100b7fc0, 100b7fd4, 100b8074, 100b80eb, 100b8136, 100b8189, 100b82dd, 100b8310
- jump table at: 100b7e42
- stores:
  - `100b7e5a` [global] mov byte ptr [0x1047fab0], 1
  - `100b7e61` [global] mov byte ptr [0x10489930], 1
  - `100b7e75` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b7ead` [relative] mov dword ptr [eax + 4], ecx
  - `100b7eb7` [relative] mov word ptr [eax + 8], dx
  - `100b7ec7` [relative] mov dword ptr [ecx], edx
  - `100b7ecf` [relative] mov dword ptr [ecx + 4], edx
  - `100b7ed8` [relative] mov dword ptr [ecx + 8], edx
  - `100b7ee1` [relative] mov dword ptr [ecx + 0xc], edx
  - `100b7ef4` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b7f12` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b7f50` [global] mov byte ptr [0x1047fab0], 1
  - `100b7f57` [global] mov byte ptr [0x10489930], 1
  - `100b7f6d` [global] mov byte ptr [0x1047fab0], 1
  - `100b7f74` [global] mov byte ptr [0x10489930], 1
  - `100b7fc0` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b7fd4` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b7fe7` [global] mov byte ptr [0x1047fab0], 1
  - `100b7fee` [global] mov byte ptr [0x10489930], 1
  - `100b8039` [global] mov byte ptr [0x1047fab0], 1
  - `100b8040` [global] mov byte ptr [0x10489930], 1
  - `100b8074` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b80eb` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b8136` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b8189` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b81b4` [relative] mov dword ptr [esp + 0x20], edi
  - `100b81c3` [relative] mov dword ptr [esp + 0x24], ebx
  - `100b81d0` [relative] mov dword ptr [esp + 0x14], eax
  - `100b81df` [relative] mov dword ptr [esp + 0x24], eax
  - `100b81e7` [relative] mov word ptr [esp + 0x14], 0
  - `100b81fd` [relative] mov dword ptr [esp + 0x14], ecx
  - `100b8214` [relative] mov word ptr [esp + 0x14], ax
  - `100b8224` [relative] mov word ptr [esp + 0x16], 0
  - `100b823a` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b8251` [relative] mov word ptr [esp + 0x16], ax
  - `100b8265` [relative] mov word ptr [esp + 0x18], bp
  - `100b8271` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b8288` [relative] mov word ptr [esp + 0x18], ax
  - `100b8298` [relative] mov word ptr [esp + 0x1a], di
  - `100b82a2` [relative] mov dword ptr [esp + 0x20], edx
  - `100b82b9` [relative] mov word ptr [esp + 0x1a], ax
  - `100b82dd` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b8310` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7e68->f_10206540, 100b7e95->f_100fc3c0, 100b7ee4->f_100fc4d0, 100b7f2b->f_10094160, 100b7f40->f_100af310, 100b7f5e->f_100af4b0, 100b7f7b->f_100af4b0, 100b7f85->f_100af4b0, 100b7fa2->f_103128b2, 100b7faf->f_100af310, 100b7fbb->f_100b93e0, 100b7fcf->f_100b9470, 100b7ff5->f_100af4b0, 100b800b->f_10206540, 100b802b->f_100af7e0, 100b8047->f_100af4b0, 100b8051->f_100af4b0, 100b8067->f_10206540, 100b8081->f_100b8430, 100b808a->f_100af4b0, 100b809d->f_101e4470, 100b80a7->f_100b8450, 100b80be->f_100af310, 100b80ce->f_100af310, 100b80de->f_100af310, 100b8109->f_100af4b0, 100b8115->f_10163130, 100b8129->f_101632a0, 100b8154->f_100af4b0, 100b8160->f_101632f0, 100b8174->f_10163310, 100b81a9->f_100af4b0, 100b81b8->f_100af4b0, 100b81c7->f_100af4b0, 100b81d4->f_100af4b0, 100b820f->f_10311c6c, 100b824c->f_10311c6c, 100b8283->f_10311c6c, 100b82b4->f_10311c6c, 100b82ce->f_10163320, 100b82f7->f_100af4b0, 100b8303->f_10163340

### 72 - f_100b84b0 (96 instructions)
- advance: 100b8568:6, 100b85d2:0xa, 100b85e4:4
- yield byte writes: 100b8570, 100b85da, 100b85ec
- stores:
  - `100b8555` [relative] mov dword ptr [eax - 4], edx
  - `100b8570` [exec-field] mov byte ptr [edi + 0x25a], 1
  - `100b85a9` [global] mov byte ptr [0x104886f5], 0
  - `100b85cb` [global] mov byte ptr [0x104886f5], 0xff
  - `100b85da` [exec-field] mov byte ptr [edi + 0x25a], 1
  - `100b85ec` [exec-field] mov byte ptr [edi + 0x25a], 1
- calls: 100b84e9->f_100af4b0, 100b84f7->f_100af4b0, 100b8509->f_100af4b0, 100b852c->f_100af4b0, 100b8560->f_10311bf0, 100b8583->f_100af4b0, 100b85b0->f_101c4820, 100b85c3->f_10017d20

### 73 - f_100b4540 (4 instructions)
- stores: none
- calls: 100b4544->f_100b4590

### 74 - f_100b7490 (19 instructions)
- advance: 100b74cd:2
- stores:
  - `100b74c6` [relative] mov dword ptr [edx + 0x124], esi
- calls: none

### 75 - f_100b8660 (55 instructions)
- advance: 100b86c4:8, 100b86ce:6, 100b8700:4
- exec set (jump form): 100b86a9:=ax, 100b86e2:=ax
- yield byte writes: 100b8672, 100b86d6
- stores:
  - `100b8672` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b86d6` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b8669->f_1018ba60, 100b86b0->f_100af4b0, 100b86b8->f_100fc680, 100b86ef->f_100af4b0, 100b86fb->f_1018baa0

### 76 - f_100b7d50 (37 instructions)
- advance: 100b7db5:5
- yield byte writes: 100b7da4
- stores:
  - `100b7da4` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7d64->f_100afac0, 100b7d6c->f_100afb00, 100b7db0->f_1008af20

### 77 - f_100b8710 (49 instructions)
- advance: 100b879e:5
- stores:
  - `100b873f` [global] mov byte ptr [0x104886da], 1
  - `100b878f` [global] mov byte ptr [0x104886db], 1
- calls: 100b871c->f_1018bbb0, 100b8725->f_100af4b0, 100b8730->f_100af4b0, 100b8746->f_1018c4f0, 100b8750->f_1018c180, 100b875f->f_1018bc40, 100b8769->f_1018bf50, 100b8774->f_1018bf00, 100b877e->f_1018c1e0, 100b8796->f_100940b0

### 78 - f_100b87b0 (11 instructions)
- advance: 100b87cd:1
- stores:
  - `100b87c1` [global] mov byte ptr [0x1065cb20], 0
- calls: 100b87bc->f_1018c4d0, 100b87c8->f_100940f0

### 79 - f_100b88e0 (90 instructions)
- advance: 100b8991:0xa, 100b89c4:0xc, 100b89ee:0xa
- stores:
  - `100b896b` [relative] mov dword ptr [eax + 0x12c], edx
  - `100b897a` [relative] mov word ptr [ecx + 0x14a], ax
  - `100b898a` [relative] mov word ptr [edx + 0x14c], ax
- calls: 100b8920->f_100afac0, 100b8928->f_100afb00, 100b8971->f_100af4b0, 100b8983->f_100af4b0, 100b89a3->f_100af4b0, 100b89ad->f_100afac0, 100b89b7->f_100afac0, 100b89bf->f_100b8810, 100b89d7->f_100afac0, 100b89e1->f_100afac0, 100b89e9->f_100b8810

### 7A - f_100b3a50 (187 instructions)
- advance: 100b3aa4:6, 100b3af0:7, 100b3b83:6, 100b3bd7:2, 100b3c54:8
- yield byte writes: 100b3b8b, 100b3bec, 100b3c48, 100b3c9a
- jump table at: 100b3a6f
- stores:
  - `100b3b53` [relative] mov word ptr [esi + 2], dx
  - `100b3b57` [relative] mov dword ptr [esi + 8], ecx
  - `100b3b68` [relative] mov dword ptr [esi + 0x260], edx
  - `100b3b7d` [relative] mov dword ptr [eax + 0x264], ecx
  - `100b3b8b` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b3bb9` [relative] mov dword ptr [eax + 0x264], ecx
  - `100b3bc1` [relative] mov dword ptr [eax + 0x264], 0
  - `100b3bdf` [relative] mov dword ptr [esi + 8], edx
  - `100b3be2` [relative] mov word ptr [esi + 2], ax
  - `100b3be6` [relative] mov dword ptr [esi + 0x260], ecx
  - `100b3bec` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b3c48` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b3c9a` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b3a84->f_100afac0, 100b3a8c->f_100afb00, 100b3a9c->f_100b3dc0, 100b3abf->f_100afac0, 100b3ac7->f_100afb00, 100b3ae8->f_100b3dc0, 100b3b0b->f_100afac0, 100b3b13->f_100afb00, 100b3c06->f_100afac0, 100b3c0e->f_100afb00, 100b3c3b->f_100b3cc0, 100b3c6f->f_100afac0, 100b3c77->f_100afb00, 100b3c89->f_100b3d70

### 7B - f_100b8a00 (34 instructions)
- advance: 100b8a69:5
- stores:
  - `100b8a58` [relative] mov dword ptr [eax + 0x12c], edx
  - `100b8a60` [relative] mov word ptr [eax + 0x148], 0xffff
- calls: 100b8a14->f_100afac0, 100b8a1c->f_100afb00

### 7C - f_100b8a80 (45 instructions)
- advance: 100b8af0:6, 100b8b09:6
- stores:
  - `100b8aea` [relative] mov dword ptr [eax + 0x128], ecx
  - `100b8b03` [relative] mov dword ptr [eax + 0x128], ecx
- calls: 100b8a94->f_100afac0, 100b8a9c->f_100afb00

### 7D - f_100b8b20 (25 instructions)
- advance: 100b8b69:3
- stores: none
- calls: 100b8b56->f_100af4b0, 100b8b61->f_10062e20

### 7E - f_100b8b80 (207 instructions)
- advance: 100b8c5d:bp, 100b8c8a:bp, 100b8cb3:bp, 100b8ce9:bp, 100b8e34:bp
- yield byte writes: 100b8c2d
- jump table at: 100b8ba2, 100b8c41
- stores:
  - `100b8c2d` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b8c57` [relative] mov dword ptr [edi + 0x174], ebx
  - `100b8c77` [relative] mov dword ptr [eax + 0x174], 5
  - `100b8ca4` [relative] mov dword ptr [eax + 0x174], ebx
  - `100b8cad` [relative] mov byte ptr [eax + 0x1db], bl
  - `100b8ccc` [relative] mov dword ptr [eax + 0x174], 0x55
  - `100b8ce3` [relative] mov byte ptr [ecx + 0x1db], al
  - `100b8d3c` [relative] mov dword ptr [eax + 0x174], 5
  - `100b8d5f` [relative] mov word ptr [esp + 0x1c], ax
  - `100b8d6d` [relative] mov word ptr [esp + 0x20], ax
  - `100b8d7b` [relative] mov word ptr [esp + 0x1e], ax
  - `100b8d89` [relative] mov word ptr [esp + 0x26], ax
  - `100b8d97` [relative] mov word ptr [esp + 0x1e], ax
  - `100b8d9c` [relative] mov word ptr [esp + 0x20], bx
  - `100b8daf` [relative] mov dword ptr [eax + 0x174], 5
  - `100b8dd2` [relative] mov word ptr [esp + 0x1c], ax
  - `100b8de0` [relative] mov word ptr [esp + 0x20], ax
  - `100b8dee` [relative] mov word ptr [esp + 0x1e], ax
  - `100b8dfc` [relative] mov word ptr [esp + 0x26], ax
  - `100b8e0a` [relative] mov word ptr [esp + 0x22], ax
  - `100b8e18` [relative] mov word ptr [esp + 0x20], ax
  - `100b8e20` [relative] mov word ptr [esp + 0x2c], bx
  - `100b8e25` [relative] mov word ptr [esp + 0x2a], bx
  - `100b8e2a` [relative] mov word ptr [esp + 0x28], bx
- calls: 100b8bd7->f_100afac0, 100b8bdf->f_100afb00, 100b8c85->f_10097a20, 100b8cda->f_100af4b0, 100b8d4a->f_10097a20, 100b8d53->f_100af4b0, 100b8d64->f_100af4b0, 100b8d72->f_100af4b0, 100b8d80->f_100af4b0, 100b8d8e->f_100af4b0, 100b8dbd->f_10097a20, 100b8dc6->f_100af4b0, 100b8dd7->f_100af4b0, 100b8de5->f_100af4b0, 100b8df3->f_100af4b0, 100b8e01->f_100af4b0, 100b8e0f->f_100af4b0, 100b8e2f->f_1009ac10

### 7F - f_100b2aa0 (63 instructions)
- advance: 100b2aea:1, 100b2b75:1
- yield byte writes: 100b2b49, 100b2b7c
- stores:
  - `100b2af7` [global] mov dword ptr [0x104887f8], 0xff
  - `100b2b1d` [global] mov dword ptr [0x104887f8], eax
  - `100b2b49` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b2b7c` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b2ab7->f_1014e0a0, 100b2adc->f_1014e1c0, 100b2b01->f_100ae3a0, 100b2b06->f_100b2a60, 100b2b22->f_100ae3a0, 100b2b27->f_100b2a60, 100b2b3d->f_1015e790, 100b2b44->f_1013bf40, 100b2b53->f_100b2a60, 100b2b69->f_1015e790, 100b2b70->f_1013bf40

### 80 - f_100b8e90 (53 instructions)
- advance: 100b8f23:5
- yield byte writes: 100b8ee2, 100b8f17
- stores:
  - `100b8ee2` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b8f17` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b8ea4->f_100afac0, 100b8eac->f_100afb00, 100b8f0e->f_100d12d0

### 81 - f_100b8fb0 (45 instructions)
- advance: 100b901f:6, 100b9033:6
- stores:
  - `100b9018` [relative] mov byte ptr [eax + 0x8b4], 1
  - `100b902c` [relative] mov byte ptr [eax + 0x8b4], 0
- calls: 100b8fc4->f_100afac0, 100b8fcc->f_100afb00

### 82 - f_100b9040 (52 instructions)
- advance: 100b90bc:7
- exec set (jump form): 100b90d3:=ax
- stores:
  - `100b9089` [relative] mov dword ptr [esp + 8], edx
  - `100b9090` [relative] mov dword ptr [esp + 0xc], ecx
  - `100b9097` [relative] mov dword ptr [esp + 0x10], edx
  - `100b909e` [relative] mov dword ptr [esp + 0x14], ecx
- calls: 100b9049->f_100afac0, 100b9068->f_1017ad70, 100b90b3->f_1017ad80, 100b90ce->f_100afa90

### 83 - f_100b90e0 (10 instructions)
- advance: 100b90f2:3
- stores: none
- calls: 100b90e3->f_101039e0, 100b90ed->f_100af310

### 84 - f_100b9100 (8 instructions)
- advance: 100b9118:1
- stores:
  - `100b9111` [relative] or dword ptr [eax + 0x12c], 1
- calls: none

### 85 - f_100b9120 (8 instructions)
- advance: 100b912d:1
- stores: none
- calls: 100b9125->f_101d6110

### 86 - f_100b8f30 (41 instructions)
- advance: 100b8fa2:6
- stores:
  - `100b8f9c` [relative] mov dword ptr [eax + 0x12c], ecx
- calls: 100b8f44->f_100afac0, 100b8f4c->f_100afb00

### 87 - f_100b9140 (69 instructions)
- advance: 100b919c:2, 100b91f6:2
- exec set (jump form): 100b91c0:=cx
- yield byte writes: 100b91a4, 100b91b2, 100b91fe, 100b9207
- stores:
  - `100b9187` [relative] mov word ptr [eax + 4], 2
  - `100b9197` [global] mov byte ptr [0x1048557a], al
  - `100b91a4` [exec-field] mov byte ptr [esi + 0x25a], al
  - `100b91b2` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b91e1` [relative] mov word ptr [eax + 4], 0
  - `100b91f1` [global] mov byte ptr [0x1048557a], al
  - `100b91fe` [exec-field] mov byte ptr [esi + 0x25a], al
  - `100b9207` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b9172->f_100fc3c0, 100b918d->f_100fc4d0, 100b91d0->f_100fc3c0, 100b91e7->f_100fc4d0

### 88 - f_100b9220 (69 instructions)
- advance: 100b9280:2, 100b92dc:2
- exec set (jump form): 100b92a4:=cx
- yield byte writes: 100b9288, 100b9296, 100b92e4, 100b92ed
- stores:
  - `100b9267` [relative] mov word ptr [eax + 4], 3
  - `100b927a` [global] mov byte ptr [0x1048557a], bl
  - `100b9288` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b9296` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b92ca` [relative] mov word ptr [eax + 4], bx
  - `100b92d6` [global] mov byte ptr [0x1048557a], bl
  - `100b92e4` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b92ed` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b9252->f_100fc3c0, 100b926d->f_100fc4d0, 100b92b4->f_100fc3c0, 100b92ce->f_100fc4d0

### 89 - f_100b9300 (15 instructions)
- advance: 100b931b:3
- stores: none
- calls: 100b9305->f_100af4b0, 100b9316->f_100b2570

### 8A - f_100b93c0 (6 instructions)
- advance: 100b93c8:1
- stores: none
- calls: 100b93c3->f_100b2a60

### 8B - f_100b9490 (112 instructions)
- advance: 100b9586:0x19, 100b95f0:0x19
- stores:
  - `100b94b5` [relative] mov dword ptr [esp + 0x10], eax
  - `100b94cd` [relative] mov dword ptr [esp + 0xc], eax
  - `100b94e6` [relative] mov dword ptr [esp + 0x14], eax
  - `100b94f1` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b94f8` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b9501` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b950c` [relative] mov byte ptr [esp + eax + 0x14], 0x20
- calls: 100b949a->f_100af4b0, 100b94a5->f_100af4b0, 100b94b0->f_100af4b0, 100b94c5->f_100af4b0, 100b953a->f_100b2570, 100b9569->f_101f8d50, 100b957c->f_101f8e30, 100b9581->f_100b2a60, 100b959b->f_100b2570, 100b95ca->f_101f8d50, 100b95d8->f_101f8de0, 100b95eb->f_101f8e30

### 8C - f_100b98e0 (192 instructions)
- advance: 100b996a:8, 100b9a16:0xc, 100b9a93:0xa, 100b9b3a:0xe
- exec set (jump form): 100b9993:=ax
- yield byte writes: 100b9972, 100b998d, 100b9a1e, 100b9a9b, 100b9b42
- jump table at: 100b990d
- stores:
  - `100b9930` [relative] mov word ptr [edi + 0xa], bx
  - `100b993d` [relative] mov word ptr [edi + 4], ax
  - `100b994a` [relative] mov word ptr [edi + 6], ax
  - `100b9958` [relative] mov word ptr [edi + 8], ax
  - `100b9964` [global] mov byte ptr [0x10480a90], bl
  - `100b9972` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b998d` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b99ba` [relative] mov word ptr [edi + 0xa], 2
  - `100b99c9` [relative] mov word ptr [edi + 4], ax
  - `100b99d6` [relative] mov word ptr [edi + 6], ax
  - `100b99da` [relative] mov word ptr [edi + 8], 0
  - `100b99e9` [relative] mov word ptr [edi + 0x12], ax
  - `100b99f6` [relative] mov word ptr [edi + 0xc], ax
  - `100b9a04` [relative] mov word ptr [edi + 0xe], ax
  - `100b9a10` [global] mov byte ptr [0x10480a90], bl
  - `100b9a1e` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b9a40` [relative] mov word ptr [edi + 0xa], 3
  - `100b9a53` [relative] mov word ptr [edi + 4], ax
  - `100b9a5c` [relative] mov word ptr [edi + 6], ax
  - `100b9a64` [relative] mov word ptr [edi + 8], 0
  - `100b9a73` [relative] mov word ptr [edi + 0x12], ax
  - `100b9a81` [relative] mov word ptr [edi + 0x10], ax
  - `100b9a8d` [global] mov byte ptr [0x10480a90], bl
  - `100b9a9b` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100b9abd` [relative] mov word ptr [edi + 0xa], 4
  - `100b9add` [relative] mov word ptr [edi + 0xa], 5
  - `100b9aec` [relative] mov word ptr [edi + 0xc], ax
  - `100b9af9` [relative] mov word ptr [edi + 4], ax
  - `100b9b06` [relative] mov word ptr [edi + 6], ax
  - `100b9b13` [relative] mov word ptr [edi + 0x12], ax
  - `100b9b20` [relative] mov word ptr [edi + 0x10], ax
  - `100b9b2e` [relative] mov word ptr [edi + 8], ax
  - `100b9b42` [exec-field] mov byte ptr [esi + 0x25a], bl
- calls: 100b991a->f_100fc3c0, 100b9934->f_100af4b0, 100b9941->f_100af4b0, 100b994e->f_100af4b0, 100b995c->f_100fc4d0, 100b99a4->f_100fc3c0, 100b99c0->f_100af4b0, 100b99cd->f_100af4b0, 100b99e0->f_100af4b0, 100b99ed->f_100af4b0, 100b99fa->f_100af4b0, 100b9a08->f_100fc4d0, 100b9a2e->f_100fc3c0, 100b9a4a->f_100af4b0, 100b9a57->f_100af4b0, 100b9a6a->f_100af4b0, 100b9a77->f_100af4b0, 100b9a85->f_100fc4d0, 100b9aab->f_100fc3c0, 100b9acb->f_100fc3c0, 100b9ae3->f_100af4b0, 100b9af0->f_100af4b0, 100b9afd->f_100af4b0, 100b9b0a->f_100af4b0, 100b9b17->f_100af4b0, 100b9b24->f_100af4b0, 100b9b32->f_100fc4d0

### 8D - f_100b9330 (22 instructions)
- advance: 100b9359:5
- stores: none
- calls: 100b9336->f_100af4b0, 100b9341->f_100af4b0, 100b9354->f_100b2570

### 8E - f_100b6980 (10 instructions)
- advance: 100b69a4:1
- stores:
  - `100b699a` [relative] mov dword ptr [eax + 0x174], 0x2d
- calls: none

### 8F - f_100b6a30 (10 instructions)
- advance: 100b6a54:1
- stores:
  - `100b6a4a` [relative] mov dword ptr [eax + 0x174], 0x2e
- calls: none

### 90 - f_100b3540 (18 instructions)
- advance: 100b3577:1
- stores:
  - `100b3571` [relative] mov dword ptr [eax + 0x124], ecx
- calls: 100b3556->f_1008e4b0

### 91 - f_100b9b70 (14 instructions)
- advance: 100b9b95:3
- stores:
  - `100b9b7b` [relative] mov dword ptr [esp + 4], eax
- calls: 100b9b76->f_100af4b0

### 92 - f_100b9ba0 (37 instructions)
- advance: 100b9c07:6
- stores:
  - `100b9c01` [relative] mov dword ptr [eax + 0x12c], ecx
- calls: 100b9bb4->f_100afac0, 100b9bbc->f_100afb00

### 93 - f_100b9d30 (27 instructions)
- advance: 100b9d5d:3, 100b9d78:3
- stores: none
- calls: 100b9d35->f_100af4b0, 100b9d51->f_1015e790, 100b9d58->f_1013bf40, 100b9d73->f_1013e520

### 94 - f_100b9cb0 (37 instructions)
- advance: 100b9d17:6
- stores:
  - `100b9d11` [relative] mov dword ptr [eax + 0x12c], ecx
- calls: 100b9cc4->f_100afac0, 100b9ccc->f_100afb00

### 95 - f_100b9c20 (22 instructions)
- advance: 100b9c62:3
- stores: none
- calls: 100b9c38->f_100af4b0, 100b9c4b->f_10087d80, 100b9c5d->f_10092900

### 96 - f_100b9c70 (15 instructions)
- advance: 100b9c9b:1
- stores: none
- calls: 100b9c84->f_10087db0, 100b9c96->f_10092900

### 97 - f_100ba0c0 (16 instructions)
- advance: 100ba0f2:5
- stores:
  - `100ba0d9` [relative] mov dword ptr [ecx + 0x138], eax
  - `100ba0ec` [relative] mov dword ptr [edx + 0x13c], eax
- calls: 100ba0c3->f_100ba040, 100ba0cc->f_100af4b0, 100ba0e1->f_100af4b0

### 98 - f_100b8630 (12 instructions)
- advance: 100b864b:1
- yield byte writes: 100b8642
- stores:
  - `100b8642` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b8639->f_1018ba60

### 99 - f_100b7ce0 (35 instructions)
- advance: 100b7d3e:5
- yield byte writes: 100b7d32
- stores:
  - `100b7d32` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b7cf4->f_100afac0, 100b7cfc->f_100afb00

### 9A - f_100b6df0 (16 instructions)
- advance: 100b6e1b:1
- yield byte writes: 100b6e12, 100b6e24
- stores:
  - `100b6e12` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b6e24` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b6e0b->f_10037320

### 9B - f_100b7520 (12 instructions)
- advance: 100b7543:1
- yield byte writes: 100b753b
- stores:
  - `100b753b` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### 9C - f_100ba100 (50 instructions)
- advance: 100ba139:3, 100ba152:3, 100ba16b:3, 100ba184:3
- stores: none
- calls: 100ba134->f_100af310, 100ba14d->f_100af310, 100ba166->f_100af310, 100ba17f->f_100af310

### 9D - f_100ba190 (568 instructions)
- advance: 100ba226:bp, 100ba314:bp, 100ba33d:6, 100ba412:cx, 100ba470:8, 100ba4d5:bp
- exec set (jump form): 100ba200:=ax, 100ba21a:=di, 100ba27f:=ax, 100ba2a6:=bx, 100ba307:=bx, 100ba3ca:=0, 100ba3fd:=bp, 100ba540:=ax, 100ba553:=ax, 100ba5ce:=ax, 100ba5e8:=bx, 100ba61c:=ax, 100ba628:=bx, 100ba730:=bp, 100ba7f5:=ax, 100ba844:=bp
- jump table at: 100ba1bb
- stores:
  - `100ba291` [relative] mov dword ptr [esp + 0x14], ecx
  - `100ba298` [relative] mov dword ptr [esp + 0x18], edx
  - `100ba29f` [relative] mov dword ptr [esp + 0x1c], ecx
  - `100ba2fe` [relative] mov dword ptr [edi + 0x104897f8], eax
  - `100ba30e` [relative] mov dword ptr [edi + 0x104896e8], edx
  - `100ba354` [relative] mov dword ptr [esp + 0x14], 0xa
  - `100ba365` [relative] mov dword ptr [esp + 0x10], 8
  - `100ba391` [relative] mov dword ptr [esp + 0x24], eax
  - `100ba3da` [relative] mov dword ptr [esi + 0x20], edx
  - `100ba3e7` [relative] mov dword ptr [esi + 0x1c], ecx
  - `100ba3fa` [relative] mov dword ptr [esi + 0x1c], ebx
  - `100ba404` [relative] mov dword ptr [esi + 0x20], edi
  - `100ba4a9` [relative] mov dword ptr [esp + 0x14], ecx
  - `100ba4b0` [relative] mov dword ptr [esp + 0x18], edx
  - `100ba4b7` [relative] mov dword ptr [esp + 0x1c], ecx
  - `100ba4c2` [relative] mov dword ptr [esp + 0x20], edx
  - `100ba4cb` [relative] mov byte ptr [esp + 0x2b], 0
  - `100ba4eb` [relative] mov dword ptr [esp + 0x14], 0xa
  - `100ba4fc` [relative] mov dword ptr [esp + 0x10], 8
  - `100ba588` [relative] mov dword ptr [esp + 0x18], ecx
  - `100ba58f` [relative] mov dword ptr [esp + 0x1c], edx
  - `100ba596` [relative] mov dword ptr [esp + 0x20], ecx
  - `100ba59d` [relative] mov dword ptr [esp + 0x24], edx
  - `100ba5a3` [relative] mov byte ptr [esp + 0x27], 0
  - `100ba65b` [relative] mov dword ptr [esp + 0x68], ecx
  - `100ba662` [relative] mov dword ptr [esp + 0x6c], edx
  - `100ba66b` [relative] mov dword ptr [esp + 0x70], ecx
  - `100ba677` [relative] mov dword ptr [esp + 0x74], edx
  - `100ba67d` [relative] mov byte ptr [esp + 0x78], 0
  - `100ba682` [relative] mov byte ptr [esp + 0x38], 0
  - `100ba695` [relative] mov dword ptr [esp + 0x28], ecx
  - `100ba69c` [relative] mov dword ptr [esp + 0x2c], ecx
  - `100ba6a3` [relative] mov dword ptr [esp + 0x30], ecx
  - `100ba6aa` [relative] mov dword ptr [esp + 0x34], edx
  - `100ba74f` [relative] mov dword ptr [esp + 0x28], ecx
  - `100ba756` [relative] mov dword ptr [esp + 0x2c], edx
  - `100ba75d` [relative] mov dword ptr [esp + 0x30], ecx
  - `100ba765` [relative] mov dword ptr [esp + 0x38], edx
  - `100ba77a` [relative] mov dword ptr [esp + 0x68], ecx
  - `100ba781` [relative] mov dword ptr [esp + 0x6c], edx
  - `100ba78a` [relative] mov dword ptr [esp + 0x70], ecx
  - `100ba796` [relative] mov dword ptr [esp + 0x74], edx
  - `100ba79a` [relative] mov byte ptr [esp + 0x38], 0
  - `100ba79f` [relative] mov byte ptr [esp + 0x78], 0
- calls: 100ba1cb->f_100af4b0, 100ba1dd->f_100af4b0, 100ba1fb->f_100afa90, 100ba210->f_100af4b0, 100ba221->f_100af310, 100ba241->f_100af4b0, 100ba253->f_100af4b0, 100ba274->f_100afa90, 100ba2bb->f_100af4b0, 100ba2cd->f_100af4b0, 100ba2ef->f_100afa90, 100ba335->f_10017d20, 100ba35c->f_100af4b0, 100ba371->f_100af4b0, 100ba3a8->f_100af4b0, 100ba3f0->f_100af4b0, 100ba407->f_100af310, 100ba42c->f_100af4b0, 100ba43e->f_100af4b0, 100ba468->f_10017d20, 100ba487->f_100af4b0, 100ba4d0->f_100af7e0, 100ba4f3->f_100af4b0, 100ba508->f_100af4b0, 100ba51f->f_100af4b0, 100ba531->f_100afa90, 100ba547->f_100af310, 100ba56d->f_100af4b0, 100ba57f->f_100af960, 100ba5a8->f_100af4b0, 100ba5c6->f_100afa90, 100ba5e0->f_100af7e0, 100ba5fd->f_100af4b0, 100ba611->f_100afa90, 100ba623->f_100afa90, 100ba636->f_100b0510, 100ba64a->f_100af960, 100ba745->f_100af960, 100ba769->f_100af960, 100ba7f0->f_100afa90

### 9E - f_100ba8a0 (11 instructions)
- advance: 100ba8bc:2
- stores:
  - `100ba8b6` [global] mov dword ptr [0x104898fc], ebx
- calls: none

### 9F - f_100b44e0 (3 instructions)
- stores: none
- calls: 100b44e5->f_100b4390

### A0 - f_100b4960 (3 instructions)
- stores: none
- calls: 100b4965->f_100b4af0

### A1 - f_100b48e0 (3 instructions)
- stores: none
- calls: 100b48e5->f_100b49d0

### A2 - f_100b4970 (3 instructions)
- stores: none
- calls: 100b4975->f_100b4af0

### A3 - f_100b48f0 (3 instructions)
- stores: none
- calls: 100b48f5->f_100b49d0

### A4 - f_100ba8d0 (22 instructions)
- advance: 100ba910:2
- stores:
  - `100ba90a` [relative] mov dword ptr [eax + 0x12c], edx
- calls: none

### A5 - f_100ba920 (22 instructions)
- advance: 100ba960:2
- stores:
  - `100ba95a` [relative] mov dword ptr [eax + 0x12c], edx
- calls: none

### A6 - f_100ba9c0 (54 instructions)
- advance: 100ba9f7:4, 100baa45:2
- exec set (jump form): 100baa1c:=cx
- yield byte writes: 100ba9ff, 100baa15, 100baa57
- stores:
  - `100ba9ff` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100baa15` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100baa52` [global] mov byte ptr [0x104804c0], al
  - `100baa57` [exec-field] mov byte ptr [esi + 0x25a], al
- calls: 100ba9f2->f_100af310, 100baa2f->f_100fc3c0, 100baa40->f_100fc4d0

### A7 - f_100baa60 (55 instructions)
- advance: 100baaab:4, 100baaf3:2
- yield byte writes: 100baab3, 100baadb, 100bab05, 100bab0e
- stores:
  - `100baab3` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100baad5` [relative] mov dword ptr [ecx + 0x180], eax
  - `100baadb` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100baafb` [relative] mov dword ptr [eax + 0x184], 0
  - `100bab05` [exec-field] mov byte ptr [esi + 0x25a], bl
  - `100bab0e` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100baaa6->f_100af310, 100baae4->f_100a0510

### A8 - f_100b9780 (57 instructions)
- advance: 100b980d:6
- stores: none
- calls: 100b97ad->f_100af4b0, 100b97b8->f_100af4b0, 100b97d8->f_100b2570, 100b97ec->f_101f8da0, 100b97ff->f_101f8e30, 100b9808->f_100b2a60

### A9 - f_100bab50 (41 instructions)
- advance: 100babca:3
- stores:
  - `100bab64` [global] mov byte ptr [0x104886da], 1
  - `100bab78` [global] mov byte ptr [0x1065cb20], 1
- calls: 100bab5b->f_1018bbb0, 100bab6b->f_100af4b0, 100bab71->f_1018be70, 100bab7f->f_1018c4f0, 100bab89->f_1018c180, 100bab98->f_1018bc40, 100baba3->f_1018bc40, 100babad->f_1018bfa0, 100babb8->f_1018bf00, 100babc2->f_1018c1e0

### AA - f_100babe0 (62 instructions)
- advance: 100bac79:0x11
- stores: none
- calls: 100babe6->f_100af4b0, 100babee->f_10103a80, 100bac00->f_100af310, 100bac06->f_10103a90, 100bac14->f_100af310, 100bac1a->f_10103ab0, 100bac28->f_100af310, 100bac2e->f_10103ae0, 100bac3b->f_100af310, 100bac41->f_10103b10, 100bac4e->f_100af310, 100bac54->f_10103b30, 100bac61->f_100af310, 100bac67->f_10103b60, 100bac74->f_100af310

### AB - f_100bac90 (325 instructions)
- advance: 100baced:cx, 100bad1d:2, 100bad4e:2, 100bad9f:2, 100badd0:2, 100badfd:2, 100bae2a:2, 100bae53:cx, 100bae6d:2, 100bae92:2, 100baeb7:2, 100baee0:cx, 100baf04:2, 100baf18:2, 100baf2c:2, 100baf47:4, 100baf6f:2, 100baf97:2, 100bafbd:4, 100bafe3:4, 100bb009:4, 100bb02f:4, 100bb055:4, 100bb095:2, 100bb0f8:6
- exec set (jump form): 100bacc5:=dx
- yield byte writes: 100bad84
- jump table at: 100bacbb
- stores:
  - `100bace7` [relative] or dword ptr [eax + 0x120], ecx
  - `100bad17` [relative] mov dword ptr [eax + 0x120], ecx
  - `100bad48` [relative] mov dword ptr [eax + 0x120], ecx
  - `100bad84` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bad99` [relative] mov dword ptr [eax + 0x120], ecx
  - `100badca` [relative] mov dword ptr [eax + 0x120], ecx
  - `100badf7` [relative] mov dword ptr [eax + 0x120], ecx
  - `100bae24` [relative] mov dword ptr [eax + 0x128], ecx
  - `100bae4d` [relative] or dword ptr [eax + 0x128], ecx
  - `100bae8b` [relative] or dword ptr [eax + 0x120], 0x40
  - `100baeb0` [relative] and dword ptr [eax + 0x120], 0xffffffbf
  - `100baeda` [relative] or dword ptr [eax + 0x130], ecx
  - `100baefd` [relative] and dword ptr [eax + 0x130], 0xfffffffd
  - `100baf11` [global] mov byte ptr [0x10489900], 1
  - `100baf25` [global] mov byte ptr [0x10489900], 0
  - `100baf42` [global] mov dword ptr [0x1035af3c], eax
  - `100baf65` [relative] or dword ptr [eax + 0x128], 0x1000000
  - `100baf8d` [relative] and dword ptr [eax + 0x128], 0xfeffffff
  - `100bb08f` [relative] mov dword ptr [eax + 0x13c], ecx
  - `100bb0f2` [relative] mov dword ptr [ecx + 0x13c], ebx
- calls: 100bae65->f_10087120, 100baf3d->f_100af4b0, 100bafa8->f_100af4b0, 100bafb5->f_1008aa80, 100bafce->f_100af4b0, 100bafdb->f_1008aad0, 100baff4->f_100af4b0, 100bb001->f_1008ab20, 100bb01a->f_100af4b0, 100bb027->f_1008ab70, 100bb040->f_100af4b0, 100bb04d->f_1008aba0, 100bb0b9->f_100afac0, 100bb0c1->f_100afb00

### AC - f_100bb180 (134 instructions)
- advance: 100bb1e1:cx, 100bb221:cx, 100bb2b3:cx, 100bb314:cx
- yield byte writes: 100bb29f
- jump table at: 100bb1a7
- stores:
  - `100bb1d5` [relative] mov dword ptr [edx + 0x16c], eax
  - `100bb215` [relative] mov dword ptr [edx + 0x174], eax
  - `100bb28a` [relative] mov dword ptr [ecx + 0x138], ebx
  - `100bb29f` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bb309` [relative] or dword ptr [ecx + 0x13c], eax
- calls: 100bb1c3->f_100af4b0, 100bb203->f_100af4b0, 100bb246->f_100afac0, 100bb24e->f_100afb00, 100bb293->f_100aec70, 100bb2ce->f_100afac0, 100bb2d6->f_100afb00, 100bb2ff->f_100af4b0

### AD - f_100bb340 (225 instructions)
- advance: 100bb46d:0xc, 100bb49a:0xc, 100bb4c3:0xc, 100bb4ec:0xc, 100bb515:0xc, 100bb53e:0xc, 100bb567:0xc, 100bb590:0xc, 100bb5b9:0xc, 100bb5e2:0xc
- jump table at: 100bb44c
- stores:
  - `100bb3e0` [relative] mov dword ptr [esp + 0x10], edx
  - `100bb3e7` [relative] mov dword ptr [esp + 0x10], esi
- calls: 100bb36d->f_100afac0, 100bb375->f_100afb00, 100bb390->f_100afac0, 100bb398->f_100afb00, 100bb45f->f_100af4b0, 100bb48c->f_100af4b0, 100bb4b5->f_100af4b0, 100bb4de->f_100af4b0, 100bb507->f_100af4b0, 100bb530->f_100af4b0, 100bb559->f_100af4b0, 100bb582->f_100af4b0, 100bb5ab->f_100af4b0, 100bb5d4->f_100af4b0

### AE - f_100bb620 (270 instructions)
- advance: 100bb669:6, 100bb6ea:8, 100bb700:8, 100bb78f:8, 100bb846:0xa, 100bb8be:6, 100bb921:0xa, 100bb97f:6
- jump table at: 100bb644
- stores:
  - `100bb6b9` [relative] mov dword ptr [esp + 0x18], ecx
  - `100bb6e3` [relative] mov word ptr [edx + 0x1d8], ax
  - `100bb6f7` [relative] mov word ptr [eax + 0x1d8], 0x17
  - `100bb74f` [relative] mov dword ptr [esp + 0x18], ecx
  - `100bb789` [relative] mov dword ptr [edx + 0x168], eax
  - `100bb813` [relative] mov dword ptr [esp + 0x10], edx
  - `100bb81a` [relative] mov dword ptr [esp + 0x10], edi
  - `100bb840` [relative] mov dword ptr [eax + 0x798], ecx
  - `100bb897` [relative] mov dword ptr [esp + 0x18], ecx
  - `100bb8b4` [relative] mov dword ptr [eax + 0x798], 0
  - `100bb90c` [relative] mov dword ptr [esp + 0x18], edx
  - `100bb91b` [relative] mov dword ptr [edi + 0x20c], eax
  - `100bb96a` [relative] mov dword ptr [esp + 0x18], ecx
  - `100bb975` [relative] mov dword ptr [eax + 0x20c], 0
- calls: 100bb64f->f_100af4b0, 100bb65b->f_100af4b0, 100bb661->f_10094040, 100bb685->f_100afac0, 100bb68d->f_100afb00, 100bb6db->f_100af4b0, 100bb71c->f_100afac0, 100bb724->f_100afb00, 100bb75f->f_100af4b0, 100bb7ab->f_100afac0, 100bb7b3->f_100afb00, 100bb7ce->f_100afac0, 100bb7d6->f_100afb00, 100bb862->f_100afac0, 100bb86a->f_100afb00, 100bb8da->f_100afac0, 100bb8e2->f_100afb00, 100bb916->f_100afac0, 100bb93d->f_100afac0, 100bb945->f_100afb00

### AF - f_100bb9c0 (53 instructions)
- advance: 100bba60:8
- stores: none
- calls: 100bb9e3->f_10015250, 100bb9eb->f_10311c6c, 100bb9f5->f_100af310, 100bb9fa->f_10015250, 100bba02->f_10311c6c, 100bba0c->f_100af310, 100bba11->f_10015250, 100bba1b->f_10015250, 100bba23->f_10311c6c, 100bba2d->f_100af310, 100bba32->f_10015250, 100bba3a->f_10311c6c, 100bba44->f_100af310, 100bba49->f_10015250, 100bba51->f_10311c6c, 100bba5b->f_100af310

### B0 - f_100b1fa0 (104 instructions)
- advance: 100b20ed:0xc
- stores:
  - `100b1fa6` [relative] mov byte ptr [esp + 4], 0x96
  - `100b1fab` [relative] mov byte ptr [esp + 5], 2
  - `100b1fb6` [relative] mov byte ptr [esp + 6], 0
  - `100b1fbd` [relative] mov word ptr [eax + 0x164], 0
  - `100b2030` [global] mov byte ptr [0x10488208], al
  - `100b2083` [global] mov word ptr [0x10482f40], bp
  - `100b208a` [global] mov word ptr [0x104855aa], bp
  - `100b20e4` [relative] mov word ptr [eax + 0x164], di
- calls: 100b1fea->f_100afac0, 100b1ff2->f_100afb00, 100b200d->f_100afac0, 100b2015->f_100afb00, 100b2077->f_10312031, 100b20c0->f_100af4b0, 100b20c8->f_100964a0, 100b20d6->f_100ad5e0

### B1 - f_100bba70 (42 instructions)
- advance: 100bba9d:4, 100bbabc:4, 100bbade:4
- stores: none
- calls: 100bba98->f_100af310, 100bbab7->f_100af310, 100bbad9->f_100af310

### B2 - f_100b0910 (69 instructions)
- advance: 100b0951:2, 100b09ee:4
- yield byte writes: 100b0959, 100b09f6
- stores:
  - `100b0959` [exec-field] mov byte ptr [esi + 0x25a], al
  - `100b098a` [relative] mov dword ptr [esp + 4], eax
  - `100b09b4` [relative] mov dword ptr [esp + 8], eax
  - `100b09f6` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b0944->f_1012f100, 100b0983->f_100af4b0, 100b09af->f_10014d60

### B3 - f_100bbaf0 (191 instructions)
- advance: 100bbba8:0xe, 100bbbd4:2, 100bbc9f:0x12, 100bbcd3:4, 100bbcf4:2, 100bbd25:4
- exec set (jump form): 100bbd3a:=ax
- yield byte writes: 100bbbb0, 100bbbdc, 100bbca7, 100bbcdb, 100bbcfc, 100bbd2d, 100bbd41
- jump table at: 100bbb17
- stores:
  - `100bbbb0` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbbdc` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbca7` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbcdb` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbcfc` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbd2d` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100bbd41` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100bbb22->f_100af4b0, 100bbb2f->f_100a34d0, 100bbb3c->f_100a34b0, 100bbb49->f_100af310, 100bbb4e->f_100a33f0, 100bbb5b->f_100af310, 100bbb60->f_100a3400, 100bbb6d->f_100af310, 100bbb72->f_100a3410, 100bbb7f->f_100af310, 100bbb84->f_100a3450, 100bbb91->f_100af310, 100bbb96->f_100a3460, 100bbba3->f_100af310, 100bbbc3->f_100a3490, 100bbbcf->f_1020a2a0, 100bbbea->f_100af4b0, 100bbbfb->f_100af4b0, 100bbc08->f_100a34e0, 100bbc15->f_100a34b0, 100bbc22->f_100af310, 100bbc27->f_100a33f0, 100bbc34->f_100af310, 100bbc39->f_100a3400, 100bbc46->f_100af310, 100bbc4b->f_100a3410, 100bbc58->f_100af310, 100bbc5d->f_100a3450, 100bbc6a->f_100af310, 100bbc6f->f_100a3460, 100bbc7c->f_100af310, 100bbc81->f_100a3470, 100bbc8b->f_100af310, 100bbc90->f_100a3480, 100bbc9a->f_100af310, 100bbcb5->f_100af4b0, 100bbcc3->f_100af4b0, 100bbccb->f_100a3310, 100bbcef->f_1020a330, 100bbd20->f_100af310

### B4 - f_100b0a00 (513 instructions)
- advance: 100b0a9a:0x14, 100b0b00:6, 100b0b7f:6, 100b0c2a:6, 100b0c7b:3, 100b0cb4:3, 100b0cf4:3, 100b0d22:3, 100b0d50:3, 100b0d94:4, 100b0de6:4, 100b0e2a:4, 100b0e4f:2, 100b0e97:4, 100b0eb7:2, 100b0ed7:2, 100b0f15:6, 100b0f3a:6, 100b0f78:6, 100b0f9d:6, 100b104c:0xc
- exec set (jump form): 100b107a:=dx
- yield byte writes: 100b0aa2, 100b0b08, 100b0b87, 100b0c32, 100b0c83, 100b0cbc, 100b0cfc, 100b0d2a, 100b0d58, 100b0d9c, 100b0dee, 100b0e32, 100b0e57, 100b0e9f, 100b0ebf, 100b0edf, 100b0f1d, 100b0f42, 100b0f80, 100b0fa5, 100b1054, 100b1081
- jump table at: 100b0a29
- stores:
  - `100b0a35` [relative] mov dword ptr [esp + 0x1c], edx
  - `100b0a3c` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b0a43` [relative] mov dword ptr [esp + 0x24], edx
  - `100b0a4a` [relative] mov dword ptr [esp + 0x28], eax
  - `100b0a4e` [relative] mov byte ptr [esp + 0x2b], 0
  - `100b0a5a` [relative] mov dword ptr [esp + 0x1c], edx
  - `100b0a61` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b0a68` [relative] mov dword ptr [esp + 0x24], edx
  - `100b0a6f` [relative] mov dword ptr [esp + 0x28], eax
  - `100b0a73` [relative] mov byte ptr [esp + 0x2b], 0
  - `100b0a81` [relative] mov byte ptr [esp + eax + 0x1c], 0x20
  - `100b0aa2` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0ad4` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b0adb` [relative] mov dword ptr [esp + 0x20], ecx
  - `100b0ae2` [relative] mov dword ptr [esp + 0x24], eax
  - `100b0aed` [relative] mov dword ptr [esp + 0x28], ecx
  - `100b0af6` [relative] mov byte ptr [esp + 0x33], 0
  - `100b0b08` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0b47` [relative] mov dword ptr [edi], ebx
  - `100b0b4c` [relative] mov dword ptr [edi + 4], ebx
  - `100b0b52` [relative] mov dword ptr [edi + 8], ebx
  - `100b0b58` [relative] mov dword ptr [edi + 0xc], eax
  - `100b0b5b` [relative] mov byte ptr [ecx + 0x10488d17], 0
  - `100b0b6b` [relative] mov dword ptr [ecx], eax
  - `100b0b70` [relative] mov dword ptr [ecx + 4], eax
  - `100b0b79` [relative] mov dword ptr [ecx + 8], eax
  - `100b0b7c` [relative] mov dword ptr [ecx + 0xc], edx
  - `100b0b87` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0bb9` [relative] mov dword ptr [esp + 0x1c], ecx
  - `100b0bc0` [relative] mov dword ptr [esp + 0x20], edx
  - `100b0bc7` [relative] mov dword ptr [esp + 0x24], ecx
  - `100b0bd3` [relative] mov dword ptr [esp + 0x28], edx
  - `100b0bd9` [relative] mov byte ptr [esp + 0x2b], 0
  - `100b0be0` [relative] mov byte ptr [eax + 0x65], 1
  - `100b0be4` [relative] mov dword ptr [eax + 0x68], 0x80808080
  - `100b0c06` [relative] mov byte ptr [ecx + 0x66], bl
  - `100b0c32` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0c66` [relative] mov word ptr [eax + 0x60], cx
  - `100b0c77` [relative] mov word ptr [edx + 0x60], cx
  - `100b0c83` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0c9c` [relative] mov word ptr [eax + 0x60], 1
  - `100b0cae` [relative] mov word ptr [ecx + 0x60], 1
  - `100b0cbc` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0ce2` [global] mov byte ptr [0x104886f4], 2
  - `100b0cfc` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0d10` [global] mov byte ptr [0x104886f4], 0
  - `100b0d2a` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0d3e` [global] mov byte ptr [0x104886f4], 1
  - `100b0d58` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0d9c` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0dee` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0e32` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0e57` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0e9f` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0ead` [global] mov dword ptr [0x1047bc8c], 1
  - `100b0ebf` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0ecd` [global] mov dword ptr [0x1047bc8c], 0
  - `100b0edf` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0f1d` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0f42` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0f80` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0fa5` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b0fbc` [relative] mov dword ptr [esp + 0xc], eax
  - `100b0fd7` [relative] mov dword ptr [esp + 0xc], eax
  - `100b0ff2` [relative] mov dword ptr [esp + 0xc], eax
  - `100b100d` [relative] mov dword ptr [esp + 0xc], eax
  - `100b1054` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b1081` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b0a95->f_100af7e0, 100b0ab4->f_100af4b0, 100b0afb->f_100af7e0, 100b0b1a->f_100af4b0, 100b0b32->f_100af960, 100b0b98->f_100af4b0, 100b0bb2->f_100af960, 100b0bf6->f_10150a30, 100b0c0e->f_1015ec50, 100b0c25->f_1015e350, 100b0cef->f_1014e210, 100b0d1d->f_1014e210, 100b0d4b->f_1014e210, 100b0d70->f_1015ec50, 100b0d7d->f_100af4b0, 100b0d8f->f_1012c1d0, 100b0dc0->f_1015ec50, 100b0dd5->f_10311c6c, 100b0de1->f_100af310, 100b0e06->f_1015ec50, 100b0e13->f_100af4b0, 100b0e25->f_1012c500, 100b0e4a->f_1015e910, 100b0e71->f_1015ec50, 100b0e86->f_10311c6c, 100b0e92->f_100af310, 100b0efa->f_100af4b0, 100b0f04->f_100af4b0, 100b0f10->f_10209430, 100b0f35->f_102094d0, 100b0f5d->f_100af4b0, 100b0f67->f_100af4b0, 100b0f73->f_10209890, 100b0f98->f_102098b0, 100b0fb7->f_100af4b0, 100b0fd2->f_100af4b0, 100b0fed->f_100af4b0, 100b1008->f_100af4b0, 100b1023->f_100af4b0, 100b1047->f_101fb070

### B5 - f_100b10f0 (25 instructions)
- advance: 100b1134:4
- yield byte writes: 100b113c
- stores:
  - `100b113c` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b111c->f_100af960, 100b112f->f_10093430

### B6 - f_100b1150 (735 instructions)
- advance: 100b147c:0x14, 100b173a:0xe, 100b1983:bx, 100b1a82:2, 100b1afd:6, 100b1d1e:0x16, 100b1d72:4
- exec set (jump form): 100b1a0b:=di
- yield byte writes: 100b1484, 100b1742, 100b198a, 100b1a04, 100b1a8a, 100b1b05, 100b1d26, 100b1d7a
- jump table at: 100b1186
- stores:
  - `100b11c5` [relative] or byte ptr [ecx + 0xf4], 1
  - `100b11cc` [relative] mov word ptr [ecx + 0xfc], ax
  - `100b1212` [relative] mov byte ptr [ecx + 0xef], al
  - `100b1220` [relative] mov dword ptr [ecx + 0x120], eax
  - `100b1283` [relative] mov word ptr [edx + ecx*2 + 0xfc], ax
  - `100b1292` [relative] or word ptr [edx + 0xf4], ax
  - `100b12e5` [relative] mov byte ptr [ecx + 0xef], al
  - `100b12f3` [relative] mov dword ptr [ecx + 0x120], eax
  - `100b1318` [relative] or byte ptr [ecx + 0xf4], 1
  - `100b131f` [relative] mov word ptr [ecx + 0xfc], ax
  - `100b1345` [relative] or byte ptr [ecx + 0xf4], 2
  - `100b134c` [relative] mov word ptr [ecx + 0xfe], ax
  - `100b1372` [relative] or byte ptr [ecx + 0xf4], 4
  - `100b1379` [relative] mov word ptr [ecx + 0x100], ax
  - `100b139f` [relative] or byte ptr [ecx + 0xf4], 8
  - `100b13a6` [relative] mov word ptr [ecx + 0x102], ax
  - `100b13d1` [relative] or word ptr [ecx + 0xf4], bx
  - `100b13d8` [relative] mov word ptr [ecx + 0x104], ax
  - `100b13fe` [relative] or byte ptr [ecx + 0xf4], 0x20
  - `100b1405` [relative] mov word ptr [ecx + 0x106], ax
  - `100b142a` [relative] or byte ptr [ecx + 0xf4], 0x40
  - `100b1431` [relative] mov word ptr [ecx + 0x108], ax
  - `100b1457` [relative] or byte ptr [ecx + 0xf4], 0x80
  - `100b145e` [relative] mov word ptr [ecx + 0x10a], ax
  - `100b1472` [relative] or dword ptr [eax + 0x130], 0x20000
  - `100b1484` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b14ac` [relative] mov byte ptr [eax + 0xee], 7
  - `100b14c7` [relative] or byte ptr [eax + 0xf4], 1
  - `100b14ce` [relative] mov word ptr [eax + 0xfc], bp
  - `100b14f4` [relative] or byte ptr [ecx + 0xf5], 2
  - `100b14fb` [relative] mov word ptr [ecx + 0x10e], ax
  - `100b150f` [relative] or dword ptr [eax + 0x130], 0x20000
  - `100b157c` [relative] mov dword ptr [ecx + 0x120], edx
  - `100b15a7` [relative] mov byte ptr [ecx + 0xef], al
  - `100b15b5` [relative] mov dword ptr [ecx + 0x120], eax
  - `100b15da` [relative] or byte ptr [ecx + 0xf4], 4
  - `100b15e1` [relative] mov word ptr [ecx + 0x100], ax
  - `100b1607` [relative] or byte ptr [ecx + 0xf4], 2
  - `100b160e` [relative] mov word ptr [ecx + 0xfe], ax
  - `100b1634` [relative] or byte ptr [ecx + 0xf4], 0x20
  - `100b163b` [relative] mov word ptr [ecx + 0x106], ax
  - `100b1661` [relative] or byte ptr [ecx + 0xf4], 8
  - `100b1668` [relative] mov word ptr [ecx + 0x102], ax
  - `100b1685` [relative] or byte ptr [eax + 0xf4], 0x10
  - `100b168c` [relative] mov word ptr [eax + 0x104], bp
  - `100b16a9` [relative] or byte ptr [eax + 0xf4], 1
  - `100b16b0` [relative] mov word ptr [eax + 0xfc], bp
  - `100b16cd` [relative] or byte ptr [eax + 0xf4], 0x40
  - `100b16d4` [relative] mov word ptr [eax + 0x108], bp
  - `100b16f1` [relative] or byte ptr [eax + 0xf4], 0x80
  - `100b16f8` [relative] mov word ptr [eax + 0x10a], bp
  - `100b1715` [relative] or byte ptr [eax + 0xf5], 1
  - `100b171c` [relative] mov word ptr [eax + 0x10c], bp
  - `100b1730` [relative] or dword ptr [eax + 0x130], 0x20000
  - `100b1742` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b17b4` [relative] mov dword ptr [ecx + 0x120], edx
  - `100b17c9` [relative] mov dword ptr [esp + 0x10], eax
  - `100b17e7` [relative] mov byte ptr [ecx + 0xef], al
  - `100b17f5` [relative] mov dword ptr [ecx + 0x120], eax
  - `100b181a` [relative] or byte ptr [ecx + 0xf4], 4
  - `100b1821` [relative] mov word ptr [ecx + 0x100], ax
  - `100b1847` [relative] or byte ptr [ecx + 0xf4], 2
  - `100b184e` [relative] mov word ptr [ecx + 0xfe], ax
  - `100b1874` [relative] or byte ptr [ecx + 0xf4], 0x20
  - `100b187b` [relative] mov word ptr [ecx + 0x106], ax
  - `100b18a1` [relative] or byte ptr [ecx + 0xf4], 8
  - `100b18a8` [relative] mov word ptr [ecx + 0x102], ax
  - `100b18ce` [relative] or word ptr [ecx + 0xf4], bx
  - `100b18d5` [relative] mov word ptr [ecx + 0x104], ax
  - `100b18f2` [relative] or byte ptr [eax + 0xf4], 1
  - `100b18f9` [relative] mov word ptr [eax + 0xfc], bp
  - `100b1916` [relative] or byte ptr [eax + 0xf4], 0x40
  - `100b191d` [relative] mov word ptr [eax + 0x108], bp
  - `100b193a` [relative] or byte ptr [eax + 0xf4], 0x80
  - `100b1941` [relative] mov word ptr [eax + 0x10a], bp
  - `100b195e` [relative] or byte ptr [eax + 0xf5], 1
  - `100b1965` [relative] mov word ptr [eax + 0x10c], bp
  - `100b1979` [relative] or dword ptr [eax + 0x130], 0x20000
  - `100b198a` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b19b7` [relative] mov dword ptr [esp + 0x10], eax
  - `100b1a04` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b1a7c` [relative] mov dword ptr [eax + 0x128], ecx
  - `100b1a8a` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b1adc` [relative] mov dword ptr [esp + 0x10], eax
  - `100b1af7` [relative] mov dword ptr [ecx + 0x128], ebx
  - `100b1b05` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b1b50` [relative] mov byte ptr [ecx + 0xef], al
  - `100b1b5e` [relative] mov dword ptr [ecx + 0x120], eax
  - `100b1b84` [relative] or byte ptr [ecx + 0xf4], 1
  - `100b1b8b` [relative] mov word ptr [ecx + 0xfc], ax
  - `100b1bb2` [relative] or byte ptr [ecx + 0xf4], 2
  - `100b1bb9` [relative] mov word ptr [ecx + 0xfe], ax
  - `100b1be0` [relative] or byte ptr [ecx + 0xf4], 4
  - `100b1be7` [relative] mov word ptr [ecx + 0x100], ax
  - `100b1c0e` [relative] or byte ptr [ecx + 0xf4], 8
  - `100b1c15` [relative] mov word ptr [ecx + 0x102], ax
  - `100b1c41` [relative] or word ptr [ecx + 0xf4], bx
  - `100b1c48` [relative] mov word ptr [ecx + 0x104], ax
  - `100b1c6f` [relative] or byte ptr [ecx + 0xf4], 0x20
  - `100b1c76` [relative] mov word ptr [ecx + 0x106], ax
  - `100b1c9c` [relative] or byte ptr [ecx + 0xf4], 0x40
  - `100b1ca3` [relative] mov word ptr [ecx + 0x108], ax
  - `100b1cca` [relative] or byte ptr [ecx + 0xf4], 0x80
  - `100b1cd1` [relative] mov word ptr [ecx + 0x10a], ax
  - `100b1cf8` [relative] or byte ptr [ecx + 0xf5], 1
  - `100b1cff` [relative] mov word ptr [ecx + 0x10c], ax
  - `100b1d14` [relative] or dword ptr [eax + 0x130], 0x20000
  - `100b1d26` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b1d57` [relative] mov dword ptr [esp + 0x18], eax
  - `100b1d7a` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b11a6->f_100af4b0, 100b11f1->f_100af4b0, 100b1251->f_100af4b0, 100b12c4->f_100af4b0, 100b12fd->f_100af4b0, 100b132a->f_100af4b0, 100b1357->f_100af4b0, 100b1384->f_100af4b0, 100b13b1->f_100af4b0, 100b13e3->f_100af4b0, 100b140f->f_100af4b0, 100b143c->f_100af4b0, 100b14d9->f_100af4b0, 100b1539->f_100af4b0, 100b1584->f_100af4b0, 100b15bf->f_100af4b0, 100b15ec->f_100af4b0, 100b1619->f_100af4b0, 100b1646->f_100af4b0, 100b1771->f_100af4b0, 100b17bc->f_100af4b0, 100b17ff->f_100af4b0, 100b182c->f_100af4b0, 100b1859->f_100af4b0, 100b1886->f_100af4b0, 100b18b3->f_100af4b0, 100b19b2->f_100af4b0, 100b1a33->f_100af4b0, 100b1a46->f_1008a750, 100b1aaf->f_100afac0, 100b1ab7->f_100afb00, 100b1b2e->f_100af4b0, 100b1b68->f_100af4b0, 100b1b96->f_100af4b0, 100b1bc4->f_100af4b0, 100b1bf2->f_100af4b0, 100b1c20->f_100af4b0, 100b1c53->f_100af4b0, 100b1c80->f_100af4b0, 100b1cae->f_100af4b0, 100b1cdc->f_100af4b0, 100b1d4b->f_100af4b0

### B7 - f_100afee0 (185 instructions)
- advance: 100aff5c:0xa, 100affbb:8, 100affd6:8, 100afff2:8, 100b002b:8, 100b0047:8, 100b008c:8, 100b00dc:8, 100b00f8:8
- jump table at: 100aff04
- stores:
  - `100b00d4` [relative] mov word ptr [eax*2 + 0x10480a94], di
- calls: 100aff19->f_100afac0, 100aff21->f_100afb00, 100aff4d->f_100af4b0, 100aff57->f_100af310, 100aff78->f_100afac0, 100aff80->f_100afb00, 100affb6->f_100af7e0, 100affd1->f_100af7e0, 100affed->f_100af7e0, 100b000e->f_100afac0, 100b0016->f_100afb00, 100b0026->f_100af310, 100b0042->f_100af310, 100b0063->f_100afac0, 100b006b->f_100afb00, 100b0087->f_100af310, 100b00a8->f_100afac0, 100b00b0->f_100afb00, 100b00c6->f_100af4b0, 100b00f0->f_10017d30

### B8 - f_100b9600 (118 instructions)
- advance: 100b9704:0x1b, 100b976b:0x1b
- stores:
  - `100b9631` [relative] mov dword ptr [esp + 0x14], eax
  - `100b9649` [relative] mov dword ptr [esp + 0x10], eax
  - `100b9662` [relative] mov dword ptr [esp + 0x18], eax
  - `100b966d` [relative] mov dword ptr [esp + 0x1c], ecx
  - `100b9674` [relative] mov dword ptr [esp + 0x20], eax
  - `100b967d` [relative] mov dword ptr [esp + 0x24], ecx
  - `100b9688` [relative] mov byte ptr [esp + eax + 0x18], 0x20
- calls: 100b960b->f_100af4b0, 100b9616->f_100af4b0, 100b9621->f_100af4b0, 100b962c->f_100af4b0, 100b9641->f_100af4b0, 100b96b8->f_100b2570, 100b96e7->f_101f8d50, 100b96fa->f_101f8e30, 100b96ff->f_100b2a60, 100b9716->f_100b2570, 100b9745->f_101f8d50, 100b9753->f_101f8de0, 100b9766->f_101f8e30

### B9 - f_100b9820 (66 instructions)
- advance: 100b98c9:8
- stores:
  - `100b9833` [relative] mov byte ptr [esp + 0x13], 1
  - `100b9846` [relative] mov byte ptr [esp + 0x13], 0
- calls: 100b9854->f_100af4b0, 100b985f->f_100af4b0, 100b986a->f_100af4b0, 100b9890->f_100b2570, 100b98a4->f_101f8da0, 100b98b7->f_101f8e30, 100b98c4->f_100b2a60

### BA - f_100b5f70 (121 instructions)
- advance: 100b612e:0xd
- stores:
  - `100b5fd1` [relative] mov dword ptr [esp + 0x10], eax
  - `100b5fee` [relative] mov dword ptr [esp + 0x10], eax
  - `100b600b` [relative] mov dword ptr [esp + 0x10], eax
  - `100b6028` [relative] mov dword ptr [esp + 0x10], eax
  - `100b608a` [relative] mov dword ptr [esp + 0x10], edx
  - `100b6092` [relative] mov dword ptr [esp + 0x14], eax
  - `100b609a` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b60c2` [relative] mov dword ptr [esi + 0x144], eax
  - `100b60dc` [relative] mov dword ptr [esp + 0x18], ecx
  - `100b60e0` [relative] mov dword ptr [esp + 0x14], edx
  - `100b60ec` [relative] mov dword ptr [esp + 0x10], eax
  - `100b610f` [relative] mov dword ptr [esi + 0x144], edx
- calls: 100b5f85->f_100afac0, 100b5f8d->f_100afb00, 100b5fcc->f_100af4b0, 100b5fe9->f_100af4b0, 100b6006->f_100af4b0, 100b6023->f_100af4b0, 100b60b5->f_10181290, 100b6102->f_10181300, 100b611c->f_10092f90, 100b6128->f_1008f2a0

### BB - f_100b44f0 (3 instructions)
- stores: none
- calls: 100b44f5->f_100b4390

### BC - f_100b4980 (3 instructions)
- stores: none
- calls: 100b4985->f_100b4af0

### BD - f_100b4900 (3 instructions)
- stores: none
- calls: 100b4905->f_100b49d0

### BE - f_100b3e20 (14 instructions)
- advance: 100b3e3f:3
- stores: none
- calls: 100b3e3a->f_100af310

### BF - f_100bbd80 (142 instructions)
- advance: 100bbeb4:0xa, 100bbefa:8
- jump table at: 100bbdb1
- stores: none
- calls: 100bbd87->f_100af4b0, 100bbdbb->f_100f97c0, 100bbdd1->f_100af4b0, 100bbde9->f_10017d20, 100bbe01->f_100f97f0, 100bbe17->f_100af4b0, 100bbe2b->f_100af4b0, 100bbe5d->f_100f9820, 100bbe6f->f_100af4b0, 100bbe83->f_100af4b0, 100bbea2->f_10017d20, 100bbeaf->f_100af310, 100bbec3->f_100f9850, 100bbed5->f_100af4b0, 100bbee4->f_10017d20, 100bbef5->f_100af310

### C0 - f_100ba970 (22 instructions)
- advance: 100ba9b1:3
- stores:
  - `100ba9ab` [relative] mov dword ptr [edx + 0x12c], eax
- calls: 100ba988->f_100af4b0

### C1 - f_100b5500 (3 instructions)
- stores: none
- calls: 100b5502->f_100b5510

### C2 - f_100bbf80 (45 instructions)
- advance: 100bbfce:6, 100bbfed:4
- exec set (jump form): 100bbfa6:=cx
- stores: none
- calls: 100bbfb4->f_100af4b0, 100bbfbc->f_100e98b0, 100bbfc9->f_100af310, 100bbfdb->f_100e9850, 100bbfe8->f_100af310

### C3 - f_100bc000 (31 instructions)
- advance: 100bc04b:7
- stores:
  - `100bc02d` [relative] mov dword ptr [ecx], edx
  - `100bc032` [relative] mov dword ptr [ecx + 4], edx
  - `100bc038` [relative] mov dword ptr [ecx + 8], edx
  - `100bc03e` [relative] mov dword ptr [ecx + 0xc], eax
  - `100bc048` [relative] mov dword ptr [edi + 0x10], eax
- calls: 100bc005->f_100af4b0, 100bc022->f_100af960, 100bc043->f_100af4b0

### C4 - f_100b4550 (26 instructions)
- stores: none
- calls: 100b4575->f_100b4590, 100b457f->f_100b4590, 100b4589->f_100b4590

### C5 - f_100b4500 (3 instructions)
- stores: none
- calls: 100b4505->f_100b4390

### C6 - f_100b4990 (3 instructions)
- stores: none
- calls: 100b4995->f_100b4af0

### C7 - f_100b4910 (3 instructions)
- stores: none
- calls: 100b4915->f_100b49d0

### C8 - f_100b9370 (30 instructions)
- advance: 100b93aa:7
- stores: none
- calls: 100b9377->f_100af4b0, 100b9382->f_100af4b0, 100b938d->f_100af4b0, 100b93a5->f_100b2570

### C9 - f_100b87e0 (10 instructions)
- advance: 100b87f8:1
- stores:
  - `100b87f1` [global] mov byte ptr [0x1065cb20], 0
- calls: 100b87ec->f_1018c4d0

### CA - f_100afc80 (25 instructions)
- yield byte writes: 100afcde
- stores:
  - `100afc90` [relative] mov word ptr [edx + ecx + 0x26], ax
  - `100afca1` [relative] mov word ptr [edx + ecx + 0x24], 0xff
  - `100afcb4` [relative] mov dword ptr [edx + ecx + 0x3c], 0xbf800000
  - `100afccb` [relative] mov dword ptr [edx + ecx], eax
  - `100afcda` [relative] mov byte ptr [edx + ecx + 0x3a], al
  - `100afcde` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### CB - f_100afc80 (25 instructions)
- yield byte writes: 100afcde
- stores:
  - `100afc90` [relative] mov word ptr [edx + ecx + 0x26], ax
  - `100afca1` [relative] mov word ptr [edx + ecx + 0x24], 0xff
  - `100afcb4` [relative] mov dword ptr [edx + ecx + 0x3c], 0xbf800000
  - `100afccb` [relative] mov dword ptr [edx + ecx], eax
  - `100afcda` [relative] mov byte ptr [edx + ecx + 0x3a], al
  - `100afcde` [exec-field] mov byte ptr [ecx + 0x25a], 1
- calls: none

### CC - f_100b9d90 (191 instructions)
- advance: 100b9e2f:ax, 100b9e5c:ax, 100b9f3d:ax, 100b9f77:ax, 100b9fac:ax, 100b9fd5:ax, 100b9fec:ax
- yield byte writes: 100b9f8d
- jump table at: 100b9dbc
- stores:
  - `100b9ddf` [global] mov dword ptr [0x104886bc], eax
  - `100b9ded` [global] mov dword ptr [0x104886c0], eax
  - `100b9df7` [global] mov dword ptr [0x104886c4], eax
  - `100b9e97` [global] xor word ptr [0x104886c2], ax
  - `100b9eb7` [global] xor word ptr [0x104886c2], ax
  - `100b9edd` [global] mov word ptr [0x104886c2], ax
  - `100b9eec` [global] mov byte ptr [0x104886c4], al
  - `100b9efa` [global] mov dword ptr [0x104886c5], edx
  - `100b9f03` [global] mov dword ptr [0x104886c9], ecx
  - `100b9f0c` [global] mov dword ptr [0x104886cd], edx
  - `100b9f16` [global] mov word ptr [0x104886d1], cx
  - `100b9f2c` [global] mov byte ptr [0x104886d3], dl
  - `100b9f8d` [exec-field] mov byte ptr [esi + 0x25a], 1
- calls: 100b9dc7->f_100af4b0, 100b9dd6->f_100af4b0, 100b9de4->f_100af4b0, 100b9df2->f_100af4b0, 100b9e24->f_1013e520, 100b9e4a->f_1015e790, 100b9e51->f_1013bf40, 100b9e6a->f_100af4b0, 100b9e83->f_100af4b0, 100b9e9e->f_100af4b0, 100b9ebe->f_100af4b0, 100b9ee3->f_100af4b0, 100b9ef1->f_100af960, 100b9f32->f_1013e520, 100b9f4b->f_100af4b0, 100b9f56->f_100af4b0, 100b9f6c->f_1020f1d0, 100b9fa1->f_100af310, 100b9fba->f_100af4b0, 100b9fca->f_1022a6d0, 100b9fe2->f_1022a640

### CD - f_100b4510 (3 instructions)
- stores: none
- calls: 100b4515->f_100b4390

### CE - f_100b49a0 (3 instructions)
- stores: none
- calls: 100b49a5->f_100b4af0

### CF - f_100b4920 (3 instructions)
- stores: none
- calls: 100b4925->f_100b49d0

### D0 - f_100b4520 (3 instructions)
- stores: none
- calls: 100b4525->f_100b4390

### D1 - f_100b49b0 (3 instructions)
- stores: none
- calls: 100b49b5->f_100b4af0

### D2 - f_100b4930 (3 instructions)
- stores: none
- calls: 100b4935->f_100b49d0

### D3 - f_100b7140 (46 instructions)
- advance: 100b71c7:6
- stores: none
- calls: 100b7168->f_100afac0, 100b7170->f_100afb00, 100b71b1->f_1002c8f0, 100b71c2->f_100ce070

### D4 - f_100b2280 (219 instructions)
- exec set (jump form): 100b22b7:=ax, 100b22d1:=bp, 100b22f2:=bp, 100b2343:=bp, 100b23aa:=bp, 100b23bd:=dx, 100b23d4:=bp, 100b2408:=bp, 100b2477:=bp, 100b2525:=bp
- yield byte writes: 100b22ca, 100b22eb, 100b23cd
- jump table at: 100b22ab
- stores:
  - `100b22ca` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b22eb` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b232a` [global] mov dword ptr [0x1047d3e8], eax
  - `100b2331` [relative] mov dword ptr [eax + 0x53c], 0x101f9510
  - `100b2377` [relative] mov word ptr [esp + 0x14], ax
  - `100b2386` [relative] mov word ptr [esp + 0x12], ax
  - `100b2392` [global] mov dword ptr [0x1047d3e8], eax
  - `100b23cd` [exec-field] mov byte ptr [esi + 0x25a], 1
  - `100b23ef` [global] mov dword ptr [0x1047d3e8], eax
  - `100b23f6` [relative] mov dword ptr [eax + 0x53c], 0x1013e6a0
  - `100b2440` [relative] mov byte ptr [esp + 0x16], al
  - `100b244e` [relative] mov word ptr [esp + 0x10], ax
  - `100b2453` [relative] mov byte ptr [esp + 0x13], 0
  - `100b245f` [global] mov dword ptr [0x1047d3e8], eax
  - `100b24af` [relative] mov byte ptr [esp + 0x16], 1
  - `100b24bb` [relative] mov word ptr [esp + 0x10], ax
  - `100b24d9` [relative] mov byte ptr [esp + 0x17], cl
  - `100b24e8` [relative] mov dword ptr [esp + 0x18], eax
  - `100b24f5` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b2503` [relative] mov dword ptr [esp + 0x1c], eax
  - `100b250e` [global] mov dword ptr [0x1047d3e8], eax
- calls: 100b22be->f_100b2550, 100b22e2->f_100b25f0, 100b2316->f_100b2570, 100b2323->f_1015ea30, 100b2355->f_100af4b0, 100b236e->f_100af4b0, 100b237c->f_100af4b0, 100b238b->f_1015ea30, 100b23a1->f_1014e070, 100b23c4->f_100b25f0, 100b23e8->f_1015ea30, 100b241a->f_100af4b0, 100b2444->f_100af4b0, 100b2458->f_1015ea30, 100b246e->f_1014e070, 100b2489->f_100af4b0, 100b24b4->f_100af4b0, 100b24df->f_100af4b0, 100b24ec->f_100af4b0, 100b24f9->f_100af4b0, 100b2507->f_1015ea30, 100b251d->f_1014e070

### D5 - f_100b4530 (3 instructions)
- stores: none
- calls: 100b4535->f_100b4390

### D6 - f_100b49c0 (3 instructions)
- stores: none
- calls: 100b49c5->f_100b4af0

### D7 - f_100b4940 (3 instructions)
- stores: none
- calls: 100b4945->f_100b49d0

### D8 - f_100bc060 (128 instructions)
- advance: 100bc120:bx, 100bc15c:bx, 100bc198:bx, 100bc23d:bx
- jump table at: 100bc0dc
- stores:
  - `100bc0c3` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc0ec` [relative] mov dword ptr [ecx + 0x150], edx
  - `100bc0fb` [relative] mov dword ptr [ecx + 0x154], edx
  - `100bc10a` [relative] mov dword ptr [ecx + 0x158], edx
  - `100bc11a` [relative] mov dword ptr [ecx + 0x15c], edx
  - `100bc136` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc172` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc1ae` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc1c6` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc1ef` [relative] mov dword ptr [esp + 0xc], eax
  - `100bc218` [relative] mov dword ptr [esp + 0xc], eax
- calls: 100bc07b->f_100afac0, 100bc083->f_100afb00, 100bc131->f_100af4b0, 100bc16d->f_100af4b0, 100bc1a9->f_100af4b0, 100bc1c1->f_100af4b0, 100bc1ea->f_100af4b0, 100bc213->f_100af4b0

### D9 - f_100b7be0 (11 instructions)
- advance: 100b7bfb:2
- stores:
  - `100b7bf6` [global] mov byte ptr [0x10463924], al
- calls: none

### DA - f_100b7c10 (12 instructions)
- advance: 100b7c27:3
- stores: none
- calls: 100b7c15->f_10193850, 100b7c22->f_100af310
