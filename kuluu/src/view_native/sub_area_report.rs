//! Wire report for the client-side sub-area latch: every transition
//! [`kuluu_render::sub_area_activation`] makes goes out as c2s `0x0F2`
//! GP_CLI_COMMAND_SUBMAPCHANGE so the server's `PChar->loc.boundary` matches
//! what the client has loaded (vendor/server/src/map/packets/c2s/0x0f2_submapchange.cpp).

use bevy::prelude::*;
use kuluu_render::sub_area_activation::SubAreaChanged;

use super::input::CommandTx;
use kuluu_session::state::AgentCommand;

/// The 280 sub-area ids the retail zone DATs declare run 293..640, so the `u32`
/// the latch carries always fits the wire's `u16` `SubMapNumber`.
fn wire_sub_area(sub_area: Option<u32>) -> u16 {
    match sub_area {
        Some(id) => u16::try_from(id).unwrap_or(ffxi_proto::map::submap::NO_SUB_AREA),
        None => ffxi_proto::map::submap::NO_SUB_AREA,
    }
}

pub fn report_sub_area_system(cmd_tx: Res<CommandTx>, mut changed: MessageReader<SubAreaChanged>) {
    for SubAreaChanged { sub_area } in changed.read() {
        let _ = cmd_tx.0.try_send(AgentCommand::ReportSubArea {
            sub_area: wire_sub_area(*sub_area),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sub_area_sentinel_matches_the_wire() {
        assert_eq!(
            kuluu_render::sub_area_activation::NO_SUB_AREA,
            ffxi_proto::map::submap::NO_SUB_AREA
        );
    }

    #[test]
    fn leaving_an_interior_reports_the_no_sub_area_sentinel() {
        assert_eq!(wire_sub_area(None), ffxi_proto::map::submap::NO_SUB_AREA);
    }

    #[test]
    fn a_shipped_sub_area_id_survives_the_narrowing() {
        assert_eq!(wire_sub_area(Some(0x1C6)), 0x1C6);
        assert_eq!(wire_sub_area(Some(u32::from(u16::MAX))), u16::MAX);
    }
}
