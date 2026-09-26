//! Emote id → emote-animation file + routine, the presentation half of an
//! emote (the server's MOTIONMES and the event opcodes carry only the id and
//! param; this table names the DAT routine that plays it).

/// The look-race byte the emote routine lengths were measured from (HumeM):
/// the reachable routines are race-uniform in length, so one install's HumeM
/// emote DATs stand in for every race's hold timer.
pub const EMOTE_LENS_RACE: u8 = 1;

pub const EMOTE_ROUTINES_PER_FILE: u16 = 8;

const SALUTE_NATION_MAX: u16 = 2;

fn em_routine(sub: u16) -> [u8; 4] {
    [
        b'e',
        b'm',
        b'0',
        b'0' + (sub % EMOTE_ROUTINES_PER_FILE) as u8,
    ]
}

/// Emote id → (emote-file offset from the FFXiMain.dll race base, `em0N`
/// routine). Derived empirically from the retail HumeM emote DATs (dump:
/// kuluu-render/examples/zz-emote-probe.rs; each routine's Motion clip mnemonic names the
/// emote — bow/poi/sl1-3/kne/lau/wee, den/nod/wav/wel/gla/che/clp, …) and
/// pinned to XIM's only known points (Actor.kt onGatheringAttempt HELM: Logging=(5,0),
/// Mining=(6,0), Harvesting=(7,0) — confirmed by the files' Japanese tool
/// particles: ono0=axe, turu=pickaxe, kama=sickle). Notable non-uniformities
/// the old id/8 hypothesis missed: Point/Bow are swapped in file 0, Salute
/// occupies em02..em04 (one per nation, 0x05A Param = nation), ids ≥ 6
/// sit at (id+2)/8 only through id 37, and Hurray, bell-ring and aim have
/// weapon- or note-keyed variants with unmapped selection, so those ids play
/// the em00 default. Returns None when no body routine exists in the era
/// DATs (face-only emotes, id gaps, unmapped job emotes).
pub fn emote_routine(emote_id: u16, param: u16) -> Option<(u32, [u8; 4])> {
    match emote_id {
        0 => Some((0, *b"em01")),
        1 => Some((0, *b"em00")),
        2 => Some((0, em_routine(2 + param.min(SALUTE_NATION_MAX)))),
        3 => Some((0, *b"em05")),
        4 => Some((0, *b"em06")),
        5 => Some((0, *b"em07")),
        6..=37 => {
            let shifted = emote_id + 2;
            Some((
                (shifted / EMOTE_ROUTINES_PER_FILE) as u32,
                em_routine(shifted % EMOTE_ROUTINES_PER_FILE),
            ))
        }
        40 => Some((5, *b"em00")),
        41 => Some((6, *b"em00")),
        42 => Some((7, *b"em00")),
        43 => Some((8, *b"em00")),
        44 => Some((11, *b"em00")),
        65..=68 => Some((12, em_routine(emote_id - 65))),
        73 => Some((10, *b"em00")),
        96 => Some((9, *b"em00")),
        _ => None,
    }
}
