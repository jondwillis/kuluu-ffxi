//! Zone → event-bytecode DAT file location.
//!
//! Re-expressed (as our own committed table) from atom0s/XiEvents
//! `Event DAT Files.md`, a **studied reference** under `research/` — which is
//! deliberately NOT a build input (unlike `vendor/`). POLUtils carries no
//! event-DAT category, and the mapping is irregular (no formula: it spans ROM,
//! ROM2, ROM3, ROM4, ROM9 with scattered dirs), so it is materialized here
//! rather than scraped at build time. Regenerate by studying the reference if
//! the upstream table changes.

use crate::archive::DatLocation;
use crate::ftable::SubPath;

/// `(zone_id, rom_dir, dir, file)` for each zone's event DAT, sorted by
/// `zone_id` for binary search. The path is pre-resolved (the reference gives
/// ROM paths directly), so it bypasses VTABLE/FTABLE.
pub const EVENT_DAT_LOCATIONS: &[(u16, &str, u16, u8)] = &[
    (0, "ROM3", 0, 66),
    (1, "ROM3", 0, 67),
    (2, "ROM3", 0, 68),
    (3, "ROM3", 0, 69),
    (4, "ROM3", 0, 70),
    (5, "ROM3", 0, 71),
    (6, "ROM3", 0, 72),
    (7, "ROM3", 0, 73),
    (8, "ROM3", 0, 74),
    (9, "ROM3", 0, 75),
    (10, "ROM3", 0, 76),
    (11, "ROM3", 0, 77),
    (12, "ROM3", 0, 78),
    (13, "ROM3", 0, 79),
    (14, "ROM3", 0, 80),
    (15, "ROM", 19, 80),
    (16, "ROM3", 0, 82),
    (17, "ROM3", 0, 83),
    (18, "ROM3", 0, 84),
    (19, "ROM3", 0, 85),
    (20, "ROM3", 0, 86),
    (21, "ROM3", 0, 87),
    (22, "ROM3", 0, 88),
    (23, "ROM3", 0, 89),
    (24, "ROM3", 0, 90),
    (25, "ROM3", 0, 91),
    (26, "ROM3", 0, 92),
    (27, "ROM3", 0, 93),
    (28, "ROM3", 0, 94),
    (29, "ROM3", 0, 95),
    (30, "ROM3", 0, 96),
    (31, "ROM3", 0, 97),
    (32, "ROM3", 0, 98),
    (33, "ROM3", 0, 99),
    (34, "ROM3", 0, 100),
    (35, "ROM3", 0, 101),
    (36, "ROM3", 0, 102),
    (37, "ROM3", 0, 103),
    (38, "ROM3", 0, 104),
    (39, "ROM3", 0, 105),
    (40, "ROM3", 0, 106),
    (41, "ROM3", 0, 107),
    (42, "ROM3", 0, 108),
    (43, "ROM3", 0, 109),
    (44, "ROM3", 0, 110),
    (45, "ROM", 19, 110),
    (46, "ROM4", 0, 51),
    (47, "ROM4", 0, 52),
    (48, "ROM4", 0, 53),
    (49, "ROM4", 0, 54),
    (50, "ROM4", 0, 55),
    (51, "ROM4", 0, 56),
    (52, "ROM4", 0, 57),
    (53, "ROM4", 0, 58),
    (54, "ROM4", 0, 59),
    (55, "ROM4", 0, 60),
    (56, "ROM4", 0, 61),
    (57, "ROM4", 0, 62),
    (58, "ROM4", 0, 63),
    (59, "ROM4", 0, 64),
    (60, "ROM4", 0, 65),
    (61, "ROM4", 0, 66),
    (62, "ROM4", 0, 67),
    (63, "ROM4", 0, 68),
    (64, "ROM4", 0, 69),
    (65, "ROM4", 0, 70),
    (66, "ROM4", 0, 71),
    (67, "ROM4", 0, 72),
    (68, "ROM4", 0, 73),
    (69, "ROM4", 0, 74),
    (70, "ROM4", 0, 75),
    (71, "ROM4", 0, 76),
    (72, "ROM4", 0, 77),
    (73, "ROM4", 0, 78),
    (74, "ROM4", 0, 79),
    (75, "ROM4", 0, 80),
    (76, "ROM4", 0, 81),
    (77, "ROM4", 0, 82),
    (78, "ROM4", 0, 83),
    (79, "ROM4", 0, 84),
    (80, "ROM", 20, 17),
    (81, "ROM", 20, 18),
    (82, "ROM", 20, 19),
    (83, "ROM", 20, 20),
    (84, "ROM", 20, 21),
    (85, "ROM", 20, 22),
    (86, "ROM", 20, 23),
    (87, "ROM", 20, 24),
    (88, "ROM", 20, 25),
    (89, "ROM", 20, 26),
    (90, "ROM", 20, 27),
    (91, "ROM", 20, 28),
    (92, "ROM", 20, 29),
    (93, "ROM", 20, 30),
    (94, "ROM", 20, 31),
    (95, "ROM", 20, 32),
    (96, "ROM", 20, 33),
    (97, "ROM", 20, 34),
    (98, "ROM", 20, 35),
    (99, "ROM", 20, 36),
    (100, "ROM", 20, 37),
    (101, "ROM", 20, 38),
    (102, "ROM", 20, 39),
    (103, "ROM", 20, 40),
    (104, "ROM", 20, 41),
    (105, "ROM", 20, 42),
    (106, "ROM", 20, 43),
    (107, "ROM", 20, 44),
    (108, "ROM", 20, 45),
    (109, "ROM", 20, 46),
    (110, "ROM", 20, 47),
    (111, "ROM", 20, 48),
    (112, "ROM", 20, 49),
    (113, "ROM2", 13, 5),
    (114, "ROM2", 13, 6),
    (115, "ROM", 20, 52),
    (116, "ROM", 20, 53),
    (117, "ROM", 20, 54),
    (118, "ROM", 20, 55),
    (119, "ROM", 20, 56),
    (120, "ROM", 20, 57),
    (121, "ROM2", 13, 7),
    (122, "ROM2", 13, 8),
    (123, "ROM2", 13, 9),
    (124, "ROM2", 13, 10),
    (125, "ROM2", 13, 11),
    (126, "ROM", 20, 63),
    (127, "ROM", 20, 64),
    (128, "ROM2", 13, 12),
    (129, "ROM", 20, 66),
    (130, "ROM2", 13, 13),
    (131, "ROM", 20, 68),
    (132, "ROM", 20, 69),
    (133, "ROM9", 8, 68),
    (134, "ROM2", 13, 14),
    (135, "ROM2", 13, 15),
    (136, "ROM", 20, 73),
    (137, "ROM", 20, 74),
    (138, "ROM", 20, 75),
    (139, "ROM", 20, 76),
    (140, "ROM", 20, 77),
    (141, "ROM", 20, 78),
    (142, "ROM", 20, 79),
    (143, "ROM", 20, 80),
    (144, "ROM", 20, 81),
    (145, "ROM", 20, 82),
    (146, "ROM", 20, 83),
    (147, "ROM", 20, 84),
    (148, "ROM", 20, 85),
    (149, "ROM", 20, 86),
    (150, "ROM", 20, 87),
    (151, "ROM", 20, 88),
    (152, "ROM", 20, 89),
    (153, "ROM2", 13, 16),
    (154, "ROM2", 13, 17),
    (155, "ROM", 20, 92),
    (156, "ROM", 20, 93),
    (157, "ROM", 20, 94),
    (158, "ROM", 20, 95),
    (159, "ROM2", 13, 18),
    (160, "ROM2", 13, 19),
    (161, "ROM", 20, 98),
    (162, "ROM", 20, 99),
    (163, "ROM2", 13, 20),
    (164, "ROM", 20, 101),
    (165, "ROM", 20, 102),
    (166, "ROM", 20, 103),
    (167, "ROM", 20, 104),
    (168, "ROM2", 13, 21),
    (169, "ROM", 20, 106),
    (170, "ROM2", 13, 22),
    (171, "ROM", 20, 108),
    (172, "ROM", 20, 109),
    (173, "ROM2", 13, 23),
    (174, "ROM2", 13, 24),
    (175, "ROM", 20, 112),
    (176, "ROM2", 13, 25),
    (177, "ROM2", 13, 26),
    (178, "ROM2", 13, 27),
    (179, "ROM2", 13, 28),
    (180, "ROM2", 13, 29),
    (181, "ROM2", 13, 30),
    (182, "ROM", 20, 119),
    (183, "ROM", 20, 120),
    (184, "ROM", 20, 121),
    (185, "ROM2", 13, 31),
    (186, "ROM2", 13, 32),
    (187, "ROM2", 13, 33),
    (188, "ROM2", 13, 34),
    (189, "ROM", 20, 126),
    (190, "ROM", 20, 127),
    (191, "ROM", 21, 0),
    (192, "ROM", 21, 1),
    (193, "ROM", 21, 2),
    (194, "ROM", 21, 3),
    (195, "ROM", 21, 4),
    (196, "ROM", 21, 5),
    (197, "ROM", 21, 6),
    (198, "ROM", 21, 7),
    (199, "ROM", 21, 8),
    (200, "ROM", 21, 9),
    (201, "ROM2", 13, 35),
    (202, "ROM2", 13, 36),
    (203, "ROM2", 13, 37),
    (204, "ROM", 21, 13),
    (205, "ROM2", 13, 38),
    (206, "ROM", 21, 15),
    (207, "ROM2", 13, 39),
    (208, "ROM2", 13, 40),
    (209, "ROM2", 13, 41),
    (210, "ROM", 21, 19),
    (211, "ROM2", 13, 42),
    (212, "ROM2", 13, 43),
    (213, "ROM2", 13, 44),
    (214, "ROM", 21, 23),
    (215, "ROM", 21, 24),
    (216, "ROM", 21, 25),
    (217, "ROM", 21, 26),
    (218, "ROM", 21, 27),
    (219, "ROM", 21, 28),
    (220, "ROM", 21, 29),
    (221, "ROM", 21, 30),
    (222, "ROM", 21, 31),
    (223, "ROM", 21, 32),
    (224, "ROM", 21, 33),
    (225, "ROM", 21, 34),
    (226, "ROM2", 13, 45),
    (227, "ROM", 21, 36),
    (228, "ROM", 21, 37),
    (229, "ROM", 21, 38),
    (230, "ROM", 21, 39),
    (231, "ROM", 21, 40),
    (232, "ROM", 21, 41),
    (233, "ROM", 21, 42),
    (234, "ROM", 21, 43),
    (235, "ROM", 21, 44),
    (236, "ROM", 21, 45),
    (237, "ROM", 21, 46),
    (238, "ROM", 21, 47),
    (239, "ROM", 21, 48),
    (240, "ROM", 21, 49),
    (241, "ROM", 21, 50),
    (242, "ROM", 21, 51),
    (243, "ROM", 21, 52),
    (244, "ROM", 21, 53),
    (245, "ROM", 21, 54),
    (246, "ROM", 21, 55),
    (247, "ROM2", 13, 46),
    (248, "ROM", 21, 57),
    (249, "ROM", 21, 58),
    (250, "ROM2", 13, 47),
    (251, "ROM2", 13, 48),
    (252, "ROM2", 13, 49),
    (253, "ROM", 21, 62),
    (254, "ROM", 21, 63),
    (255, "ROM", 21, 64),
    (256, "ROM9", 5, 53),
    (257, "ROM9", 5, 54),
    (258, "ROM9", 5, 55),
    (259, "ROM9", 5, 56),
    (260, "ROM9", 5, 57),
    (261, "ROM9", 5, 58),
    (262, "ROM9", 5, 59),
    (263, "ROM9", 5, 60),
    (264, "ROM9", 5, 61),
    (265, "ROM9", 5, 62),
    (266, "ROM9", 5, 63),
    (267, "ROM9", 5, 64),
    (268, "ROM9", 5, 65),
    (269, "ROM9", 5, 66),
    (270, "ROM9", 5, 67),
    (271, "ROM9", 5, 68),
    (272, "ROM9", 5, 69),
    (273, "ROM9", 5, 70),
    (274, "ROM9", 5, 71),
    (275, "ROM9", 5, 72),
    (276, "ROM9", 5, 73),
    (277, "ROM9", 5, 74),
    (278, "ROM", 375, 125),
    (279, "ROM9", 5, 76),
    (280, "ROM", 303, 28),
    (281, "ROM", 315, 104),
    (282, "ROM", 315, 105),
    (283, "ROM", 374, 96),
    (284, "ROM", 303, 29),
    (285, "ROM", 306, 56),
    (287, "ROM", 362, 20),
    (288, "ROM", 332, 104),
    (289, "ROM", 337, 61),
    (290, "ROM", 342, 78),
    (291, "ROM", 342, 79),
    (292, "ROM", 353, 56),
    (293, "ROM", 342, 80),
    (294, "ROM", 354, 111),
    (295, "ROM", 355, 6),
    (296, "ROM", 355, 34),
    (297, "ROM", 355, 49),
    (298, "ROM", 361, 87),
    (299, "ROM", 378, 101),
];

/// Pre-resolved [`DatLocation`] of a zone's event-bytecode DAT, or `None`.
pub fn zone_id_to_event_location(zone_id: u16) -> Option<DatLocation> {
    EVENT_DAT_LOCATIONS
        .binary_search_by_key(&zone_id, |&(z, _, _, _)| z)
        .ok()
        .map(|i| {
            let (_, rom_dir, dir, file) = EVENT_DAT_LOCATIONS[i];
            DatLocation {
                rom_dir: rom_dir.to_string(),
                sub_path: SubPath { dir, file },
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_populated() {
        assert!(EVENT_DAT_LOCATIONS.len() > 250);
        assert!(EVENT_DAT_LOCATIONS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(EVENT_DAT_LOCATIONS.iter().all(|&(_, _, _, f)| f <= 127));
    }

    #[test]
    fn known_base_zone_locations() {
        // Southern San d'Oria (230) and the early sequential block.
        let loc = zone_id_to_event_location(0).unwrap();
        assert_eq!(loc.rom_dir, "ROM3");
        assert_eq!(loc.sub_path, SubPath { dir: 0, file: 66 });
        assert!(zone_id_to_event_location(9999).is_none());
    }

    /// Loads real event + string DATs from a retail install when present;
    /// self-skips otherwise.
    #[test]
    fn loads_real_event_and_string_dats_when_install_present() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        // First event DAT that exists on disk parses as a container.
        let parsed_event = EVENT_DAT_LOCATIONS.iter().any(|&(_, rom, dir, file)| {
            let loc = DatLocation {
                rom_dir: rom.to_string(),
                sub_path: SubPath { dir, file },
            };
            std::fs::read(loc.path_under(root.root()))
                .ok()
                .filter(|b| b.len() > 4)
                .is_some_and(|b| crate::event_dat::EventDat::parse(&b).is_ok())
        });
        assert!(parsed_event, "no event DAT parsed from install");

        // First zone whose string DAT resolves + exists parses as a DialogTable.
        let parsed_string = EVENT_DAT_LOCATIONS.iter().any(|&(zone, _, _, _)| {
            let Some(fid) = crate::zone_dat::zone_id_to_string_file_id(zone) else {
                return false;
            };
            let Ok(loc) = root.resolve(fid) else {
                return false;
            };
            std::fs::read(loc.path_under(root.root()))
                .ok()
                .filter(|b| b.len() > 8)
                .is_some_and(|b| crate::dmsg::StringDat::parse(&b).is_ok())
        });
        assert!(parsed_string, "no string DAT parsed from install");
    }
}
