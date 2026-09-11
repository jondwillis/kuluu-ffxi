//! Server entity animation bytes (the 0x0D/0x37 `server_status` field).

pub const NONE: u8 = 0;

pub const ATTACK: u8 = 1;

pub const HEALING: u8 = 33;

pub const SIT: u8 = 47;

/// A door's swing state. Retail keeps the same value as the door actor's
/// `GameStatus` (research/XIClient/src/XIClient/include/World/Actor/GameStatus.h, `D_OPEN` /
/// `D_CLOSE`), so this one byte is the whole of what the server says about a
/// door — the swing itself is the client's, driven from the zone DAT's per-door
/// `open`/`clos` routines (`enum ANIMATIONTYPE`,
/// vendor/server/src/map/entities/baseentity.h).
pub const OPEN_DOOR: u8 = 8;
pub const CLOSE_DOOR: u8 = 9;

/// Riding a chocobo — the classic mount, which retail renders from a dedicated
/// PC race config rather than the generic mount model block. Noble Chocobo also
/// arrives as `CHOCOBO`; the two differ only in `CustomProperties[1]`
/// (vendor/server/src/map/entities/charentity.cpp,
/// CCharEntity::tryStartNextEvent).
pub const CHOCOBO: u8 = 5;

/// Riding any non-chocobo mount; the specific one comes from the packet's mount
/// index, not from this byte.
pub const MOUNT: u8 = 85;

/// `CBattleEntity::isMounted` (vendor/server/src/map/entities/battleentity.cpp)
/// — the single predicate the server itself uses to gate mount speed and to refuse
/// actions, so the client must agree with it exactly.
pub fn is_mounted(animation: u8) -> bool {
    animation == CHOCOBO || animation == MOUNT
}

// ANIMATIONTYPE, vendor/server/src/map/entities/baseentity.h. The server writes
// these into the entity's server_status (the 0x0D/0x37 animation byte) and broadcasts
// them; the client maps each to the matching fsh* model clip (research/xim Actor.kt updateFishingState).
// The pre-overhaul (38-43,50) and current (56-62) fishing systems share fsh0..fsh6.
pub const FISHING_FISH_OLD: u8 = 38;
pub const FISHING_CAUGHT_OLD: u8 = 39;
pub const FISHING_ROD_BREAK_OLD: u8 = 40;
pub const FISHING_LINE_BREAK_OLD: u8 = 41;
pub const FISHING_MONSTER_OLD: u8 = 42;
pub const FISHING_STOP_OLD: u8 = 43;
pub const FISHING_START_OLD: u8 = 50;

pub const FISHING_START: u8 = 56;
pub const FISHING_FISH: u8 = 57;
pub const FISHING_CAUGHT: u8 = 58;
pub const FISHING_ROD_BREAK: u8 = 59;
pub const FISHING_LINE_BREAK: u8 = 60;
pub const FISHING_MONSTER: u8 = 61;
pub const FISHING_STOP: u8 = 62;

/// Phase index 0..=6 of a fishing macro-state animation byte (current or pre-overhaul),
/// or `None` if the byte is not a fishing animation. The index selects the `fsh<n>` clip:
/// 0=cast/wait, 1=fighting, 2=caught fish, 3=rod break, 4=line break, 5=caught monster,
/// 6=stop/cancel.
pub fn fishing_phase(animation: u8) -> Option<u8> {
    Some(match animation {
        FISHING_START | FISHING_START_OLD => 0,
        FISHING_FISH | FISHING_FISH_OLD => 1,
        FISHING_CAUGHT | FISHING_CAUGHT_OLD => 2,
        FISHING_ROD_BREAK | FISHING_ROD_BREAK_OLD => 3,
        FISHING_LINE_BREAK | FISHING_LINE_BREAK_OLD => 4,
        FISHING_MONSTER | FISHING_MONSTER_OLD => 5,
        FISHING_STOP | FISHING_STOP_OLD => 6,
        _ => return None,
    })
}
