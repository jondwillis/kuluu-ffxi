//! Client-local menus (Mog House exit door "Where to?" and the Mog Menu) driven
//! through the same [`DialogState`] frames the event VM produces, so the HUD
//! dialog panel and headless agents consume them unchanged. Terminal picks map to
//! [`MogHouseExit`]/[`AgentCommand::ChangeJob`]-shaped results; the session owns
//! the packet sends.
//!
//! [`AgentCommand::ChangeJob`]: crate::state::AgentCommand::ChangeJob

use ffxi_proto::decode::ServerLoginMyroom;

use crate::state::{DialogState, JobInfoState, MogHouseExit, MyRoomInfo};

/// Synthetic client-local actor ids, kept in a reserved top-of-range block: LSB
/// unique_no encodings ((4<<28)|(zone<<12)|targid at the largest) never reach
/// 0xFFFF_FF00 (vendor/server/src/map/zone_entities.cpp id assignment).
pub const MH_DOOR_ENTITY_ID: u32 = 0xFFFF_FF01;
pub const MOG_MENU_ID: u32 = 0xFFFF_FF02;

/// Nameplate/dialog speaker for the synthesized exit door, matching XIM's
/// re-creation (research/xim .../assetviewer/AssetViewer.kt:669 — LSB spawns no
/// door NPC, so there is no server-side name to echo).
pub const MH_DOOR_NAME: &str = "Door: To Town";
pub const MOG_MENU_NPC_NAME: &str = "Moogle";

pub const WHERE_TO_PROMPT: &str = "Where to?";
pub const STAY_ROW: &str = "Stay in your room.";
pub const HOME_ROW: &str = "Area you entered from.";
pub const CHANGE_FLOORS_ROW: &str = "Change floors.";
pub const SELECT_AREA_ROW: &str = "Select an area to exit to.";
pub const MOG_GARDEN_ROW: &str = "Mog Garden.";

pub const MOG_MENU_PROMPT: &str = "Mog Menu";
pub const CHANGE_JOBS_ROW: &str = "Change Jobs";
pub const MOG_SAFE_ROW: &str = "Mog Safe";
pub const MOG_LOCKER_ROW: &str = "Mog Locker";
pub const STORAGE_ROW: &str = "Storage";
pub const DELIVERY_BOX_ROW: &str = "Delivery Box";
pub const CANCEL_ROW: &str = "Cancel";

pub const MAIN_JOB_ROW: &str = "Main Job";
pub const SUPPORT_JOB_ROW: &str = "Support Job";

/// JOBTYPE 1=WAR..22=RUN are player-selectable; 23 (MON) is not.
/// vendor/server/src/map/entities/battleentity.h JOBTYPE.
const SELECTABLE_JOB_MAX: u8 = 22;

/// District rows per MyRoomExitBit; the slot is the MYROOMEXITMODE Option1-4 value
/// LSB maps to a destination zone (vendor/server/src/map/packets/c2s/
/// 0x05e_maprect.cpp:88-135).
fn district_rows(exit_bit: u8) -> &'static [(&'static str, u8)] {
    match exit_bit {
        1 => &[
            ("Southern San d'Oria", 1),
            ("Northern San d'Oria", 2),
            ("Port San d'Oria", 3),
        ],
        2 => &[
            ("Bastok Mines", 1),
            ("Bastok Markets", 2),
            ("Port Bastok", 3),
        ],
        3 => &[
            ("Windurst Waters", 1),
            ("Windurst Walls", 2),
            ("Port Windurst", 3),
            ("Windurst Woods", 4),
        ],
        4 => &[
            ("Ru'Lude Gardens", 1),
            ("Upper Jeuno", 2),
            ("Lower Jeuno", 3),
            ("Port Jeuno", 4),
        ],
        5 => &[("Al Zahbi", 1), ("Aht Urhgan Whitegate", 2)],
        9 => &[("Western Adoulin", 1), ("Eastern Adoulin", 2)],
        _ => &[],
    }
}

#[derive(Debug, Clone)]
enum Action {
    Close,
    Exit(MogHouseExit),
    OpenAreas { exit_bit: u8 },
    OpenJobType,
    OpenJobList { support: bool },
    PickJob { support: bool, job: u8 },
    Stub(&'static str),
}

struct Menu {
    npc_id: u32,
    npc_name: &'static str,
    prompt: String,
    rows: Vec<(String, Action)>,
}

/// Outcome of feeding a player response to the active local menu.
pub enum Advance {
    Frame(DialogState),
    Exit(MogHouseExit),
    ChangeJob {
        main_job: Option<u8>,
        sub_job: Option<u8>,
    },
    /// An unimplemented row: surface `notice` as a system chat line and keep the
    /// menu open with `frame`.
    Stub {
        notice: &'static str,
        frame: DialogState,
    },
    Close,
}

#[derive(Default)]
pub struct LocalMenuSession {
    menu: Option<Menu>,
    job_info: Option<JobInfoState>,
}

impl LocalMenuSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active(&self) -> bool {
        self.menu.is_some()
    }

    pub fn clear(&mut self) {
        self.menu = None;
    }

    /// The exit-door "Where to?" menu. "Change floors." shows when the player is
    /// on the 2F — LSB forces the full menu there (0x00a_login.cpp MH branch) —
    /// or when the 2F-unlock bit (char_sync.cpp:61) is known true; the server's
    /// rejection of a locked 2F request is log-only, so an ungated row would
    /// fail silently.
    pub fn open_mh_exit(
        &mut self,
        myroom: &MyRoomInfo,
        mh_2f_unlocked: Option<bool>,
    ) -> DialogState {
        let on_2f = myroom.sub_map == ServerLoginMyroom::SUB_MAP_2F;
        let mut rows = vec![
            (STAY_ROW.to_string(), Action::Close),
            (
                HOME_ROW.to_string(),
                Action::Exit(MogHouseExit::Home {
                    exit_bit: myroom.exit_bit,
                }),
            ),
        ];
        if on_2f || mh_2f_unlocked == Some(true) {
            let target = if on_2f {
                MogHouseExit::Mog1F
            } else {
                MogHouseExit::Mog2F
            };
            rows.push((CHANGE_FLOORS_ROW.to_string(), Action::Exit(target)));
        }
        if !district_rows(myroom.exit_bit).is_empty() {
            rows.push((
                SELECT_AREA_ROW.to_string(),
                Action::OpenAreas {
                    exit_bit: myroom.exit_bit,
                },
            ));
        }
        rows.push((
            MOG_GARDEN_ROW.to_string(),
            Action::Exit(MogHouseExit::MogGarden),
        ));
        self.set(Menu {
            npc_id: MH_DOOR_ENTITY_ID,
            npc_name: MH_DOOR_NAME,
            prompt: WHERE_TO_PROMPT.to_string(),
            rows,
        })
    }

    /// The Mog Menu (opened by s2c 0x02E or `AgentCommand::OpenMogMenu`).
    pub fn open_mog_menu(&mut self, job_info: Option<JobInfoState>) -> DialogState {
        self.job_info = job_info;
        self.set(mog_menu())
    }

    pub fn advance(&mut self, choice: Option<u32>) -> Advance {
        let Some(menu) = self.menu.as_ref() else {
            return Advance::Close;
        };
        let action = match choice.and_then(|c| menu.rows.get(c as usize)) {
            Some((_, action)) => action.clone(),
            None => {
                self.clear();
                return Advance::Close;
            }
        };
        match action {
            Action::Close => {
                self.clear();
                Advance::Close
            }
            Action::Exit(kind) => {
                self.clear();
                Advance::Exit(kind)
            }
            Action::OpenAreas { exit_bit } => Advance::Frame(self.set(areas_menu(exit_bit))),
            Action::OpenJobType => match self.job_info {
                Some(info) => Advance::Frame(self.set(job_type_menu(&info))),
                None => Advance::Stub {
                    notice: "Job data has not arrived yet (no 0x01B JOB_INFO) — try again.",
                    frame: frame(self.menu.as_ref().expect("menu still active")),
                },
            },
            Action::OpenJobList { support } => match self.job_info {
                Some(info) => Advance::Frame(self.set(job_list_menu(&info, support))),
                None => {
                    self.clear();
                    Advance::Close
                }
            },
            Action::PickJob { support, job } => {
                self.clear();
                Advance::ChangeJob {
                    main_job: (!support).then_some(job),
                    sub_job: support.then_some(job),
                }
            }
            Action::Stub(notice) => Advance::Stub {
                notice,
                frame: frame(self.menu.as_ref().expect("menu still active")),
            },
        }
    }

    fn set(&mut self, menu: Menu) -> DialogState {
        let f = frame(&menu);
        self.menu = Some(menu);
        f
    }
}

fn frame(menu: &Menu) -> DialogState {
    DialogState {
        event_id: menu.npc_id,
        npc_id: menu.npc_id,
        npc_name: Some(menu.npc_name.to_string()),
        act_index: 0,
        event_num: 0,
        event_para: 0,
        mode: 0,
        event_num2: 0,
        event_para2: 0,
        strings: Vec::new(),
        nums: Vec::new(),
        prompt: Some(menu.prompt.clone()),
        choices: menu.rows.iter().map(|(label, _)| label.clone()).collect(),
    }
}

fn areas_menu(exit_bit: u8) -> Menu {
    let rows = district_rows(exit_bit)
        .iter()
        .map(|&(label, slot)| {
            (
                label.to_string(),
                Action::Exit(MogHouseExit::from_bit_slot(exit_bit, slot)),
            )
        })
        .chain(std::iter::once((CANCEL_ROW.to_string(), Action::Close)))
        .collect();
    Menu {
        npc_id: MH_DOOR_ENTITY_ID,
        npc_name: MH_DOOR_NAME,
        prompt: SELECT_AREA_ROW.to_string(),
        rows,
    }
}

fn mog_menu() -> Menu {
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: MOG_MENU_PROMPT.to_string(),
        rows: vec![
            (CHANGE_JOBS_ROW.to_string(), Action::OpenJobType),
            (
                MOG_SAFE_ROW.to_string(),
                Action::Stub(
                    "Mog Safe is not yet implemented — storage UI is tracked as kuluu-6a0.",
                ),
            ),
            (
                MOG_LOCKER_ROW.to_string(),
                Action::Stub(
                    "Mog Locker is not yet implemented — storage UI is tracked as kuluu-6a0.",
                ),
            ),
            (
                STORAGE_ROW.to_string(),
                Action::Stub(
                    "Storage is not yet implemented — storage UI is tracked as kuluu-6a0.",
                ),
            ),
            (
                DELIVERY_BOX_ROW.to_string(),
                Action::Stub(
                    "Delivery Box is not yet implemented — storage UI is tracked as kuluu-6a0.",
                ),
            ),
            (CANCEL_ROW.to_string(), Action::Close),
        ],
    }
}

fn job_type_menu(info: &JobInfoState) -> Menu {
    let mut rows = vec![(
        MAIN_JOB_ROW.to_string(),
        Action::OpenJobList { support: false },
    )];
    if info.sub_job_unlocked {
        rows.push((
            SUPPORT_JOB_ROW.to_string(),
            Action::OpenJobList { support: true },
        ));
    }
    rows.push((CANCEL_ROW.to_string(), Action::Close));
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: CHANGE_JOBS_ROW.to_string(),
        rows,
    }
}

fn job_list_menu(info: &JobInfoState, support: bool) -> Menu {
    let mut rows: Vec<(String, Action)> = Vec::new();
    for job in 1..=SELECTABLE_JOB_MAX {
        if info.unlocked & (1u32 << job) == 0 {
            continue;
        }
        let name = ffxi_proto::job_names::lookup(job as u16).unwrap_or("Unknown");
        let level = info
            .job_levels
            .get(job as usize)
            .copied()
            .unwrap_or_default();
        let marker = if job == info.mjob_no {
            " (main)"
        } else if job == info.sjob_no {
            " (support)"
        } else {
            ""
        };
        rows.push((
            format!("{name} Lv.{level}{marker}"),
            Action::PickJob { support, job },
        ));
    }
    rows.push((CANCEL_ROW.to_string(), Action::Close));
    Menu {
        npc_id: MOG_MENU_ID,
        npc_name: MOG_MENU_NPC_NAME,
        prompt: if support {
            "Select a support job.".to_string()
        } else {
            "Select a main job.".to_string()
        },
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(sub_map: u8, exit_bit: u8) -> MyRoomInfo {
        MyRoomInfo {
            model: 257,
            sub_map,
            exit_bit,
        }
    }

    fn pick(session: &mut LocalMenuSession, frame: &DialogState, label: &str) -> Advance {
        let idx = frame
            .choices
            .iter()
            .position(|c| c.starts_with(label))
            .unwrap_or_else(|| panic!("row `{label}` in {:?}", frame.choices));
        session.advance(Some(idx as u32))
    }

    fn job_info() -> JobInfoState {
        let mut job_levels = [0u8; ffxi_proto::decode::JobInfo::MAX_JOBTYPE];
        job_levels[1] = 75;
        job_levels[3] = 37;
        JobInfoState {
            mjob_no: 1,
            sjob_no: 3,
            unlocked: 0b1_1011,
            sub_job_unlocked: true,
            job_levels,
        }
    }

    #[test]
    fn synthetic_ids_stay_outside_lsb_unique_no_space() {
        // Largest LSB unique_no shape: (4<<28)|(zone<<12)|targid.
        let lsb_ceiling = (4u32 << 28) | (0xFFFu32 << 12) | 0xFFF;
        for id in [MH_DOOR_ENTITY_ID, MOG_MENU_ID] {
            assert!(id > lsb_ceiling, "0x{id:08X} collides with server id space");
        }
        assert_ne!(MH_DOOR_ENTITY_ID, MOG_MENU_ID);
    }

    /// Pins the LSB MYROOMEXITBIT/MYROOMEXITMODE contract
    /// (vendor/server/src/map/packets/c2s/0x05e_maprect.h:26-50).
    #[test]
    fn terminal_exit_rows_map_to_lsb_wire_pairs() {
        assert_eq!(MogHouseExit::Home { exit_bit: 1 }.wire_pair(), (1, 0));
        assert_eq!(MogHouseExit::Mog2F.wire_pair(), (0, 125));
        assert_eq!(MogHouseExit::Mog1F.wire_pair(), (0, 126));
        assert_eq!(MogHouseExit::MogGarden.wire_pair(), (0, 127));

        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, HOME_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (1, 0)),
            _ => panic!("Home row must be a terminal exit"),
        }

        // Non-city MHs (exit_bit 0) echo bit 0 on the Home exit — retail derives
        // the bit from the zone (research/XiPackets/world/client/0x005E) and 0 is
        // MYROOMEXITBIT::Default in the LSB validator.
        let f = s.open_mh_exit(&room(0, 0), None);
        match pick(&mut s, &f, HOME_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 0)),
            _ => panic!("Home row must be a terminal exit"),
        }

        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, MOG_GARDEN_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 127)),
            _ => panic!("Mog Garden row must be a terminal exit"),
        }
    }

    #[test]
    fn change_floors_row_direction_and_gating() {
        let mut s = LocalMenuSession::new();

        let f = s.open_mh_exit(&room(ServerLoginMyroom::SUB_MAP_2F, 1), None);
        match pick(&mut s, &f, CHANGE_FLOORS_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 126), "2F goes down"),
            _ => panic!("2F must always show Change floors."),
        }

        let f = s.open_mh_exit(&room(0, 1), Some(true));
        match pick(&mut s, &f, CHANGE_FLOORS_ROW) {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (0, 125), "1F goes up"),
            _ => panic!("unlocked 1F must show Change floors."),
        }

        let f = s.open_mh_exit(&room(0, 1), None);
        assert!(
            !f.choices.iter().any(|c| c == CHANGE_FLOORS_ROW),
            "unknown 2F unlock hides the row (server rejection is silent)"
        );
        s.clear();
    }

    #[test]
    fn district_submenu_maps_slots_to_city_exits() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 3), None);
        let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("area row opens the district submenu"),
        };
        assert_eq!(
            f.choices,
            vec![
                "Windurst Waters",
                "Windurst Walls",
                "Port Windurst",
                "Windurst Woods",
                CANCEL_ROW,
            ]
        );
        match pick(&mut s, &f, "Windurst Walls") {
            Advance::Exit(kind) => assert_eq!(kind.wire_pair(), (3, 2)),
            _ => panic!("district row must be a terminal exit"),
        }
    }

    /// Pins every district row's label → (MyRoomExitBit, MyRoomExitMode) pair to
    /// LSB's destination-zone formulas — row order IS the destination, including
    /// the three irregular ones (Jeuno's Ru'Lude-first base, Whitegate's split
    /// Al Zahbi/Whitegate base, Adoulin's mode-2 Eastern special case)
    /// (vendor/server/src/map/packets/c2s/0x05e_maprect.cpp:88-135 + zone.h) —
    /// plus the `from_bit_slot` inverse and membership in the PacketValidator
    /// oneOf enums (0x05e_maprect.h:26-50), whose rejection is a silent drop.
    #[test]
    fn district_rows_match_lsb_destination_zone_formulas() {
        let expected: &[(u8, &[(&str, u8)])] = &[
            (
                1,
                &[
                    ("Southern San d'Oria", 1),
                    ("Northern San d'Oria", 2),
                    ("Port San d'Oria", 3),
                ],
            ),
            (
                2,
                &[
                    ("Bastok Mines", 1),
                    ("Bastok Markets", 2),
                    ("Port Bastok", 3),
                ],
            ),
            (
                3,
                &[
                    ("Windurst Waters", 1),
                    ("Windurst Walls", 2),
                    ("Port Windurst", 3),
                    ("Windurst Woods", 4),
                ],
            ),
            (
                4,
                &[
                    ("Ru'Lude Gardens", 1),
                    ("Upper Jeuno", 2),
                    ("Lower Jeuno", 3),
                    ("Port Jeuno", 4),
                ],
            ),
            (5, &[("Al Zahbi", 1), ("Aht Urhgan Whitegate", 2)]),
            (9, &[("Western Adoulin", 1), ("Eastern Adoulin", 2)]),
        ];
        const VALID_BITS: std::ops::RangeInclusive<u8> = 0..=9;
        const VALID_MODES: [u8; 8] = [0, 1, 2, 3, 4, 125, 126, 127];

        for &(bit, rows) in expected {
            let mut s = LocalMenuSession::new();
            let f = s.open_mh_exit(&room(0, bit), None);
            let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
                Advance::Frame(f) => f,
                _ => panic!("area row opens the district submenu"),
            };
            let labels: Vec<&str> = rows.iter().map(|&(l, _)| l).collect();
            assert_eq!(
                f.choices[..rows.len()],
                labels[..],
                "row order is the destination for bit {bit}"
            );
            s.clear();

            for &(label, slot) in rows {
                let f = s.open_mh_exit(&room(0, bit), None);
                let f = match pick(&mut s, &f, SELECT_AREA_ROW) {
                    Advance::Frame(f) => f,
                    _ => panic!("area row opens the district submenu"),
                };
                let kind = match pick(&mut s, &f, label) {
                    Advance::Exit(kind) => kind,
                    _ => panic!("`{label}` must be a terminal exit"),
                };
                assert_eq!(kind.wire_pair(), (bit, slot), "{label}");
                assert_eq!(
                    MogHouseExit::from_bit_slot(bit, slot).wire_pair(),
                    (bit, slot),
                    "from_bit_slot must invert wire_pair for {label}"
                );
                let (b, m) = kind.wire_pair();
                assert!(VALID_BITS.contains(&b), "{label} bit {b} out of enum");
                assert!(VALID_MODES.contains(&m), "{label} mode {m} out of enum");
            }
        }
    }

    #[test]
    fn exit_bit_zero_hides_area_row() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mh_exit(&room(0, 0), None);
        assert!(!f.choices.iter().any(|c| c == SELECT_AREA_ROW));
        assert!(f.choices.iter().any(|c| c == STAY_ROW));
        s.clear();
    }

    #[test]
    fn job_pick_produces_change_job() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(Some(job_info()));
        assert_eq!(f.npc_id, MOG_MENU_ID);
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Change Jobs opens the type chooser"),
        };
        assert!(f.choices.iter().any(|c| c == SUPPORT_JOB_ROW));
        let f = match pick(&mut s, &f, MAIN_JOB_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Main Job opens the job list"),
        };
        assert!(
            f.choices[0].starts_with("Warrior Lv.75"),
            "raw level, no client-side halving: {:?}",
            f.choices
        );
        assert!(f.choices[0].ends_with("(main)"));
        match pick(&mut s, &f, "Warrior") {
            Advance::ChangeJob { main_job, sub_job } => {
                assert_eq!(main_job, Some(1));
                assert_eq!(sub_job, None);
            }
            _ => panic!("job pick must yield ChangeJob"),
        }
        assert!(!s.active());
    }

    #[test]
    fn support_row_gated_on_sub_job_unlocked() {
        let mut s = LocalMenuSession::new();
        let info = JobInfoState {
            sub_job_unlocked: false,
            ..job_info()
        };
        let f = s.open_mog_menu(Some(info));
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!("Change Jobs opens the type chooser"),
        };
        assert!(!f.choices.iter().any(|c| c == SUPPORT_JOB_ROW));
        s.clear();
    }

    #[test]
    fn support_pick_fills_sub_job_only() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(Some(job_info()));
        let f = match pick(&mut s, &f, CHANGE_JOBS_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        let f = match pick(&mut s, &f, SUPPORT_JOB_ROW) {
            Advance::Frame(f) => f,
            _ => panic!(),
        };
        match pick(&mut s, &f, "White Mage") {
            Advance::ChangeJob { main_job, sub_job } => {
                assert_eq!(main_job, None);
                assert_eq!(sub_job, Some(3));
            }
            _ => panic!("support pick must yield ChangeJob"),
        }
    }

    #[test]
    fn storage_rows_stub_and_keep_menu_open() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None);
        match pick(&mut s, &f, MOG_SAFE_ROW) {
            Advance::Stub { notice, frame } => {
                assert!(notice.contains("kuluu-6a0"));
                assert_eq!(frame.choices, f.choices);
            }
            _ => panic!("storage row must stub"),
        }
        assert!(s.active());
    }

    #[test]
    fn dismiss_and_out_of_range_close() {
        let mut s = LocalMenuSession::new();
        let f = s.open_mog_menu(None);
        assert!(matches!(s.advance(None), Advance::Close));
        assert!(!s.active());

        let f2 = s.open_mog_menu(None);
        assert_eq!(f.choices, f2.choices);
        assert!(matches!(s.advance(Some(99)), Advance::Close));
        assert!(!s.active());
    }
}
