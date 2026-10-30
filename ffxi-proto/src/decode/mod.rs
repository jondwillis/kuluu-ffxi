/// Pins a decoder's hand-written body offset to LSB's own `offsetof`, which
/// ffxi-proto/build.rs walks out of vendor/server/src/map/packets/s2c/*.h into
/// [`crate::s2c_layout`]. `$upstream_field` names the member upstream calls it,
/// so a vendor bump that moves a field fails the build saying which one.
macro_rules! pin_s2c_offset {
    ($ours:expr, $upstream:expr, $upstream_field:literal) => {
        const _: () = assert!(
            $ours == $upstream,
            concat!(
                "LSB moved ",
                $upstream_field,
                ": the decoder offset is stale"
            )
        );
    };
}

pub mod animation;

mod death_menu;
pub use death_menu::*;

mod widescan;
pub use widescan::*;

mod entity;
pub use entity::*;
mod login;
pub use login::*;
mod status;
pub use status::*;
mod emote;
pub use emote::*;
mod fishing;
pub use fishing::*;
mod messages;
pub use messages::*;
mod weather;
pub use weather::*;
mod key_items;
pub use key_items::*;
mod movement;
pub use movement::*;
mod party;
pub use party::*;
mod trade;
pub use trade::*;
mod inventory;
pub use inventory::*;
mod delivery;
pub use delivery::*;
mod abilities;
pub use abilities::*;
mod equip_inspect;
pub use equip_inspect::*;
mod inspect_message;
pub use inspect_message::*;
mod bazaar;
pub use bazaar::*;
mod treasure;
pub use treasure::*;
mod auction;
pub use auction::*;
mod assist;
pub use assist::*;
mod pending;
pub use pending::*;
mod unity;
pub use unity::*;
mod merits;
pub use merits::*;
mod homepoints;
pub use homepoints::*;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("expected at least {0} bytes, have {1}")]
    Truncated(usize, usize),
    #[error("unrecognized discriminant 0x{0:02x}")]
    UnknownDiscriminant(u8),
}
