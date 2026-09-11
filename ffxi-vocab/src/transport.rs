#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    pub voyage_zone: u16,
    pub npc_id: u32,
    pub boundary: u16,
    pub offset: u16,
    pub interval: u16,
    pub arrival: u16,
    pub waiting: u16,
    pub departure: u16,
}

include!(concat!(env!("OUT_DIR"), "/transport_table.rs"));

pub fn voyage(zone: u16) -> Option<&'static Schedule> {
    let mut schedules = SCHEDULES
        .iter()
        .filter(|s| s.voyage_zone == zone && s.voyage_zone != 0);
    let schedule = schedules.next()?;
    schedules.next().is_none().then_some(schedule)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_requires_an_unambiguous_voyage_schedule() {
        assert!(voyage(228).is_some());
        assert!(voyage(0).is_none());
        for zone in 223..=226 {
            assert!(voyage(zone).is_none());
        }
    }
}
