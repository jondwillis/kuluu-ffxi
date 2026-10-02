use std::{fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use lsb_scrape::{
    check_scrape_count, parse_cpp_enum_class, parse_lua_indexed_pair_table, parse_sql_insert_rows,
    parse_u16_pair_rows, parse_u32_pair_rows, parse_xi_ident_table, parse_yaml,
    parse_yaml_enum_values, prettify_snake_case, rust_string_literal, split_sql_fields,
    split_sql_tuple, write_u16_table, write_u16_u16_table, write_u16_u32_table, write_u16_u8_table,
    zone_data_files, Yaml,
};

const LSB_MSG_BASIC_H: &str = "../vendor/server/src/map/enums/msg_basic.h";
const LSB_MSG_LUA: &str = "../vendor/server/scripts/enum/msg.lua";
const LSB_KEY_ITEM_LUA: &str = "../vendor/server/scripts/enum/key_item.lua";
const LSB_JOB_NAME_LUA: &str = "../vendor/server/scripts/enum/job_name.lua";
const LSB_SPELL_LIST_SQL: &str = "../vendor/server/sql/spell_list.sql";
const LSB_ABILITIES_SQL: &str = "../vendor/server/sql/abilities.sql";
const LSB_WEAPON_SKILLS_SQL: &str = "../vendor/server/sql/weapon_skills.sql";
const LSB_MOB_SKILLS_SQL: &str = "../vendor/server/sql/mob_skills.sql";
const LSB_ITEM_BASIC_SQL: &str = "../vendor/server/sql/item_basic.sql";
const LSB_ITEM_EQUIPMENT_SQL: &str = "../vendor/server/sql/item_equipment.sql";
const LSB_ITEM_USABLE_SQL: &str = "../vendor/server/sql/item_usable.sql";
const LSB_ITEM_WEAPON_SQL: &str = "../vendor/server/sql/item_weapon.sql";
const LSB_STATUS_EFFECTS_YAML: &str = "../vendor/server/data/status_effects.yaml";
const LSB_STATUS_EFFECT_FLAG_YAML: &str = "../vendor/server/data/enums/status_effect_flag.yaml";
const LSB_ZONE_ENUM_YAML: &str = "../vendor/server/data/enums/zone.yaml";
const LSB_ZONES_DATA_DIR: &str = "../vendor/server/data/zones";
const LSB_EMOTE_H: &str = "../vendor/server/src/map/enums/emote.h";

/// Smallest row count each scrape can return and still plausibly have parsed
/// its source; the argument is the count the pinned vendor tree yields today
/// (kuluu-m4yk).
mod floor {
    use lsb_scrape::scrape_floor;

    pub const MSG_BASIC: usize = scrape_floor(243);
    pub const MSG_CHANNEL: usize = scrape_floor(13);
    pub const MSG_AREA: usize = scrape_floor(6);
    /// The actionModifier table has two rows, so half of it still passes when
    /// the walker matched only one; its full count is the only floor that
    /// detects a partial drift.
    pub const MSG_ACTION_MODIFIER: usize = 2;
    pub const MSG_SYSTEM: usize = scrape_floor(9);
    pub const STATUS_EFFECT: usize = scrape_floor(668);
    pub const KEY_ITEM: usize = scrape_floor(3206);
    pub const JOB_NAME: usize = scrape_floor(23);
    pub const SPELL: usize = scrape_floor(890);
    pub const SPELL_SKILL: usize = scrape_floor(771);
    pub const SPELL_VALID_TARGET: usize = scrape_floor(891);
    pub const SPELL_ANIMATION: usize = scrape_floor(891);
    pub const SPELL_CAST_TIME: usize = scrape_floor(891);
    pub const SPELL_RECAST_TIME: usize = scrape_floor(891);
    pub const ABILITY: usize = scrape_floor(616);
    pub const ABILITY_VALID_TARGET: usize = scrape_floor(616);
    pub const ABILITY_RECAST_ID: usize = scrape_floor(616);
    pub const ABILITY_ANIMATION: usize = scrape_floor(616);
    pub const TP_MOVE: usize = scrape_floor(2652);
    pub const WEAPON_SKILL_NAME: usize = scrape_floor(226);
    pub const ITEM: usize = scrape_floor(23233);
    pub const ITEM_FLAGS: usize = scrape_floor(23187);
    pub const ITEM_STACK_SIZE: usize = scrape_floor(4231);
    pub const STATUS_EFFECT_FLAGS: usize = scrape_floor(664);
    pub const EQUIP_INFO: usize = scrape_floor(15378);
    pub const ITEM_USABLE: usize = scrape_floor(3075);
    pub const WEAPON_SKILL: usize = scrape_floor(4681);
    pub const WEAPON_SKILL_ANIMATION: usize = scrape_floor(226);
    pub const MOB_SKILL_ANIMATION: usize = scrape_floor(4344);
    pub const EMOTE: usize = scrape_floor(51);
    pub const TRANSPORT: usize = scrape_floor(24);
}

fn main() -> Result<()> {
    scrape_transport()?;
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={LSB_MSG_BASIC_H}");
    println!("cargo:rerun-if-changed={LSB_MSG_LUA}");
    println!("cargo:rerun-if-changed={LSB_KEY_ITEM_LUA}");
    println!("cargo:rerun-if-changed={LSB_JOB_NAME_LUA}");
    println!("cargo:rerun-if-changed={LSB_SPELL_LIST_SQL}");
    println!("cargo:rerun-if-changed={LSB_ABILITIES_SQL}");
    println!("cargo:rerun-if-changed={LSB_WEAPON_SKILLS_SQL}");
    println!("cargo:rerun-if-changed={LSB_MOB_SKILLS_SQL}");
    println!("cargo:rerun-if-changed={LSB_ITEM_BASIC_SQL}");
    println!("cargo:rerun-if-changed={LSB_ITEM_EQUIPMENT_SQL}");
    println!("cargo:rerun-if-changed={LSB_ITEM_USABLE_SQL}");
    println!("cargo:rerun-if-changed={LSB_ITEM_WEAPON_SQL}");
    println!("cargo:rerun-if-changed={LSB_STATUS_EFFECTS_YAML}");
    println!("cargo:rerun-if-changed={LSB_STATUS_EFFECT_FLAG_YAML}");
    println!("cargo:rerun-if-changed={LSB_EMOTE_H}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("OUT_DIR not set")?);

    let msg_src = fs::read_to_string(LSB_MSG_BASIC_H)
        .with_context(|| format!("reading {LSB_MSG_BASIC_H}"))?;
    let entries = parse_msg_basic(&msg_src)?;
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by ffxi-vocab/build.rs from msg_basic.h.\n");
    out.push_str("// Do not edit by hand.\n");
    out.push_str("pub const MSG_BASIC: &[(u16, &str)] = &[\n");
    for (id, text) in &entries {
        out.push_str(&format!("    ({id}, {}),\n", rust_string_literal(text)));
    }
    out.push_str("];\n");
    fs::write(out_dir.join("msg_basic_table.rs"), &out)?;
    check_scrape_count(
        "msg_basic entries",
        LSB_MSG_BASIC_H,
        entries.len(),
        floor::MSG_BASIC,
    )?;

    let lua_src =
        fs::read_to_string(LSB_MSG_LUA).with_context(|| format!("reading {LSB_MSG_LUA}"))?;
    for (lua_table, out_const, out_file, min_rows) in [
        (
            "channel",
            "MSG_CHANNEL",
            "msg_channel_table.rs",
            floor::MSG_CHANNEL,
        ),
        ("area", "MSG_AREA", "msg_area_table.rs", floor::MSG_AREA),
        (
            "actionModifier",
            "MSG_ACTION_MODIFIER",
            "msg_action_modifier_table.rs",
            floor::MSG_ACTION_MODIFIER,
        ),
        (
            "system",
            "MSG_SYSTEM",
            "msg_system_table.rs",
            floor::MSG_SYSTEM,
        ),
    ] {
        let entries = parse_lua_table(&lua_src, lua_table)?;
        let mut out = String::new();
        out.push_str("// AUTO-GENERATED by ffxi-vocab/build.rs from msg.lua.\n");
        out.push_str("// Do not edit by hand.\n");
        out.push_str(&format!("pub const {out_const}: &[(u16, &str)] = &[\n"));
        for (id, text) in &entries {
            out.push_str(&format!("    ({id}, {}),\n", rust_string_literal(text)));
        }
        out.push_str("];\n");
        fs::write(out_dir.join(out_file), &out)?;
        check_scrape_count(
            &format!("{} entries", lua_table_label(lua_table)),
            LSB_MSG_LUA,
            entries.len(),
            min_rows,
        )?;
    }

    let status_effects_src = fs::read_to_string(LSB_STATUS_EFFECTS_YAML)
        .with_context(|| format!("reading {LSB_STATUS_EFFECTS_YAML}"))?;
    let status_effects = parse_status_effects(&status_effects_src)
        .with_context(|| format!("parsing {LSB_STATUS_EFFECTS_YAML}"))?;
    let effect_entries: Vec<(u32, String)> = status_effects
        .iter()
        .map(|effect| (u32::from(effect.id), prettify_snake_case(&effect.key)))
        .collect();
    write_u16_table(
        &out_dir.join("status_names_table.rs"),
        "STATUS_NAMES",
        LSB_STATUS_EFFECTS_YAML,
        &effect_entries,
    )?;
    check_scrape_count(
        "status_effect entries",
        LSB_STATUS_EFFECTS_YAML,
        effect_entries.len(),
        floor::STATUS_EFFECT,
    )?;

    let key_item_src = fs::read_to_string(LSB_KEY_ITEM_LUA)
        .with_context(|| format!("reading {LSB_KEY_ITEM_LUA}"))?;
    let key_item_entries = parse_xi_ident_table(&key_item_src, "xi.keyItem")?;
    write_u16_table(
        &out_dir.join("key_item_names_table.rs"),
        "KEY_ITEM_NAMES",
        LSB_KEY_ITEM_LUA,
        &key_item_entries,
    )?;
    check_scrape_count(
        "key_item entries",
        LSB_KEY_ITEM_LUA,
        key_item_entries.len(),
        floor::KEY_ITEM,
    )?;

    let job_src = fs::read_to_string(LSB_JOB_NAME_LUA)
        .with_context(|| format!("reading {LSB_JOB_NAME_LUA}"))?;
    let job_entries = parse_lua_indexed_pair_table(&job_src, "xi.jobName", 3)?;
    write_u16_table(
        &out_dir.join("job_names_table.rs"),
        "JOB_NAMES",
        LSB_JOB_NAME_LUA,
        &job_entries,
    )?;
    let job_abbrevs = parse_lua_indexed_pair_table(&job_src, "xi.jobName", 1)?;
    write_u16_table(
        &out_dir.join("job_abbrevs_table.rs"),
        "JOB_ABBREVS",
        LSB_JOB_NAME_LUA,
        &job_abbrevs,
    )?;
    check_scrape_count(
        "job_name entries",
        LSB_JOB_NAME_LUA,
        job_entries.len(),
        floor::JOB_NAME,
    )?;
    check_scrape_count(
        "job_name abbreviations",
        LSB_JOB_NAME_LUA,
        job_abbrevs.len(),
        floor::JOB_NAME,
    )?;

    let spell_src = fs::read_to_string(LSB_SPELL_LIST_SQL)
        .with_context(|| format!("reading {LSB_SPELL_LIST_SQL}"))?;
    let spell_entries = parse_sql_insert_rows(&spell_src, "spell_list", 0, 1)?;
    write_u16_table(
        &out_dir.join("spell_names_table.rs"),
        "SPELL_NAMES",
        LSB_SPELL_LIST_SQL,
        &spell_entries,
    )?;
    check_scrape_count(
        "spell entries",
        LSB_SPELL_LIST_SQL,
        spell_entries.len(),
        floor::SPELL,
    )?;

    let spell_skill_entries = parse_spell_skill_rows(&spell_src)?;
    write_u16_u8_table(
        &out_dir.join("spell_skill_table.rs"),
        "SPELL_MAGIC_SKILL",
        LSB_SPELL_LIST_SQL,
        &spell_skill_entries,
    )?;
    check_scrape_count(
        "spell-skill entries",
        LSB_SPELL_LIST_SQL,
        spell_skill_entries.len(),
        floor::SPELL_SKILL,
    )?;

    let spell_target_entries = parse_u16_pair_rows(&spell_src, "spell_list", 7)?;
    write_u16_u16_table(
        &out_dir.join("spell_valid_target_table.rs"),
        "SPELL_VALID_TARGET",
        LSB_SPELL_LIST_SQL,
        &spell_target_entries,
    )?;
    check_scrape_count(
        "spell validTarget entries",
        LSB_SPELL_LIST_SQL,
        spell_target_entries.len(),
        floor::SPELL_VALID_TARGET,
    )?;

    let spell_anim_entries = parse_u16_pair_rows(&spell_src, "spell_list", 14)?;
    write_u16_u16_table(
        &out_dir.join("spell_animation_table.rs"),
        "SPELL_ANIMATION",
        LSB_SPELL_LIST_SQL,
        &spell_anim_entries,
    )?;
    check_scrape_count(
        "spell animation entries",
        LSB_SPELL_LIST_SQL,
        spell_anim_entries.len(),
        floor::SPELL_ANIMATION,
    )?;

    // vendor/server/sql/spell_list.sql fields 10 castTime / 11 recastTime (ms).
    let spell_cast_entries = parse_u16_pair_rows(&spell_src, "spell_list", 10)?;
    write_u16_u16_table(
        &out_dir.join("spell_cast_time_table.rs"),
        "SPELL_CAST_TIME_MS",
        LSB_SPELL_LIST_SQL,
        &spell_cast_entries,
    )?;
    check_scrape_count(
        "spell castTime entries",
        LSB_SPELL_LIST_SQL,
        spell_cast_entries.len(),
        floor::SPELL_CAST_TIME,
    )?;

    let spell_recast_entries = parse_u32_pair_rows(&spell_src, "spell_list", 11)?;
    write_u16_u32_table(
        &out_dir.join("spell_recast_time_table.rs"),
        "SPELL_RECAST_TIME_MS",
        LSB_SPELL_LIST_SQL,
        &spell_recast_entries,
    )?;
    check_scrape_count(
        "spell recastTime entries",
        LSB_SPELL_LIST_SQL,
        spell_recast_entries.len(),
        floor::SPELL_RECAST_TIME,
    )?;

    let abil_src = fs::read_to_string(LSB_ABILITIES_SQL)
        .with_context(|| format!("reading {LSB_ABILITIES_SQL}"))?;
    let abil_entries = parse_sql_insert_rows(&abil_src, "abilities", 0, 1)?;
    write_u16_table(
        &out_dir.join("ability_names_table.rs"),
        "ABILITY_NAMES",
        LSB_ABILITIES_SQL,
        &abil_entries,
    )?;
    check_scrape_count(
        "ability entries",
        LSB_ABILITIES_SQL,
        abil_entries.len(),
        floor::ABILITY,
    )?;

    let abil_target_entries = parse_u16_pair_rows(&abil_src, "abilities", 4)?;
    write_u16_u16_table(
        &out_dir.join("ability_valid_target_table.rs"),
        "ABILITY_VALID_TARGET",
        LSB_ABILITIES_SQL,
        &abil_target_entries,
    )?;
    check_scrape_count(
        "ability validTarget entries",
        LSB_ABILITIES_SQL,
        abil_target_entries.len(),
        floor::ABILITY_VALID_TARGET,
    )?;

    let abil_recast_entries = parse_u16_pair_rows(&abil_src, "abilities", 6)?;
    write_u16_u16_table(
        &out_dir.join("ability_recast_id_table.rs"),
        "ABILITY_RECAST_ID",
        LSB_ABILITIES_SQL,
        &abil_recast_entries,
    )?;
    check_scrape_count(
        "ability recastId entries",
        LSB_ABILITIES_SQL,
        abil_recast_entries.len(),
        floor::ABILITY_RECAST_ID,
    )?;

    let abil_anim_entries = parse_u16_pair_rows(&abil_src, "abilities", 9)?;
    write_u16_u16_table(
        &out_dir.join("ability_animation_table.rs"),
        "ABILITY_ANIMATION",
        LSB_ABILITIES_SQL,
        &abil_anim_entries,
    )?;
    check_scrape_count(
        "ability animation entries",
        LSB_ABILITIES_SQL,
        abil_anim_entries.len(),
        floor::ABILITY_ANIMATION,
    )?;

    let ws_src = fs::read_to_string(LSB_WEAPON_SKILLS_SQL)
        .with_context(|| format!("reading {LSB_WEAPON_SKILLS_SQL}"))?;

    // vendor/server/sql/weapon_skills.sql field 6 `animation`, the per-skill index the WS
    // completion effect's file id is the race base plus.
    let ws_anim_entries = parse_u16_pair_rows(&ws_src, "weapon_skills", 6)?;
    write_u16_u16_table(
        &out_dir.join("weapon_skill_animation_table.rs"),
        "WEAPON_SKILL_ANIMATION",
        LSB_WEAPON_SKILLS_SQL,
        &ws_anim_entries,
    )?;
    check_scrape_count(
        "weapon-skill animation entries",
        LSB_WEAPON_SKILLS_SQL,
        ws_anim_entries.len(),
        floor::WEAPON_SKILL_ANIMATION,
    )?;

    // One id space, two LSB tables: ids < 256 are weapon skills PCs and mobs share
    // (weapon_skills.name), ids >= 256 are monster-only TP moves (mob_skills.mob_skill_name).
    // mob_skills mirrors the low ids verbatim but omits some, so both are merged.
    let mob_skill_src = fs::read_to_string(LSB_MOB_SKILLS_SQL)
        .with_context(|| format!("reading {LSB_MOB_SKILLS_SQL}"))?;
    // vendor/server/sql/mob_skills.sql field 1 `mob_anim_id`, the FTABLE index
    // `ffxi_vocab::action_anim::mob_skill_file_id` bases.
    let mob_anim_entries = parse_u16_pair_rows(&mob_skill_src, "mob_skills", 1)?;
    write_u16_u16_table(
        &out_dir.join("mob_skill_animation_table.rs"),
        "MOB_SKILL_ANIMATION",
        LSB_MOB_SKILLS_SQL,
        &mob_anim_entries,
    )?;
    check_scrape_count(
        "mob-skill animation entries",
        LSB_MOB_SKILLS_SQL,
        mob_anim_entries.len(),
        floor::MOB_SKILL_ANIMATION,
    )?;

    let ws_name_entries = parse_sql_insert_rows(&ws_src, "weapon_skills", 0, 1)?;
    write_u16_table(
        &out_dir.join("weapon_skill_names_table.rs"),
        "WEAPON_SKILL_NAMES",
        LSB_WEAPON_SKILLS_SQL,
        &ws_name_entries,
    )?;
    check_scrape_count(
        "weapon-skill name entries",
        LSB_WEAPON_SKILLS_SQL,
        ws_name_entries.len(),
        floor::WEAPON_SKILL_NAME,
    )?;

    let mut tp_move_entries = parse_sql_insert_rows(&mob_skill_src, "mob_skills", 0, 2)?;
    tp_move_entries.extend(ws_name_entries);
    write_u16_table(
        &out_dir.join("tp_move_names_table.rs"),
        "TP_MOVE_NAMES",
        &format!("{LSB_MOB_SKILLS_SQL} + {LSB_WEAPON_SKILLS_SQL}"),
        &tp_move_entries,
    )?;
    check_scrape_count(
        "TP-move name entries",
        &format!("{LSB_MOB_SKILLS_SQL} + {LSB_WEAPON_SKILLS_SQL}"),
        tp_move_entries.len(),
        floor::TP_MOVE,
    )?;

    let item_src = fs::read_to_string(LSB_ITEM_BASIC_SQL)
        .with_context(|| format!("reading {LSB_ITEM_BASIC_SQL}"))?;
    let item_entries = parse_sql_insert_rows(&item_src, "item_basic", 0, 2)?;
    write_u16_table(
        &out_dir.join("item_names_table.rs"),
        "ITEM_NAMES",
        LSB_ITEM_BASIC_SQL,
        &item_entries,
    )?;
    check_scrape_count(
        "item entries",
        LSB_ITEM_BASIC_SQL,
        item_entries.len(),
        floor::ITEM,
    )?;

    let item_flag_entries = parse_sql_item_flags(&item_src)?;
    write_u16_u32_table(
        &out_dir.join("item_flags_table.rs"),
        "ITEM_FLAGS",
        LSB_ITEM_BASIC_SQL,
        &item_flag_entries,
    )?;
    check_scrape_count(
        "nonzero item-flag entries",
        LSB_ITEM_BASIC_SQL,
        item_flag_entries.len(),
        floor::ITEM_FLAGS,
    )?;

    let item_stack_entries = parse_sql_item_stack_sizes(&item_src)?;
    write_u16_u8_table(
        &out_dir.join("item_stack_size_table.rs"),
        "ITEM_STACK_SIZES",
        LSB_ITEM_BASIC_SQL,
        &item_stack_entries,
    )?;
    check_scrape_count(
        "stackable item entries",
        LSB_ITEM_BASIC_SQL,
        item_stack_entries.len(),
        floor::ITEM_STACK_SIZE,
    )?;

    let flag_src = fs::read_to_string(LSB_STATUS_EFFECT_FLAG_YAML)
        .with_context(|| format!("reading {LSB_STATUS_EFFECT_FLAG_YAML}"))?;
    let flag_bits = parse_yaml_enum_values(&flag_src)
        .with_context(|| format!("parsing {LSB_STATUS_EFFECT_FLAG_YAML}"))?;
    let mut flag_consts = String::new();
    flag_consts.push_str(&format!(
        "// AUTO-GENERATED by ffxi-vocab/build.rs from {LSB_STATUS_EFFECT_FLAG_YAML}.\n"
    ));
    flag_consts.push_str("// Do not edit by hand (ffxi-vocab/build.rs).\n");
    for (name, bits) in &flag_bits {
        flag_consts.push_str(&format!(
            "pub const FLAG_{}: u32 = {bits:#x};\n",
            name.to_ascii_uppercase()
        ));
    }
    fs::write(out_dir.join("status_effect_flag_consts.rs"), &flag_consts)?;
    let status_flag_entries = status_effect_flag_words(&status_effects, &flag_bits)?;
    write_u16_u32_table(
        &out_dir.join("status_effect_flags_table.rs"),
        "STATUS_EFFECT_FLAGS",
        LSB_STATUS_EFFECTS_YAML,
        &status_flag_entries,
    )?;
    check_scrape_count(
        "nonzero status-effect flag entries",
        LSB_STATUS_EFFECTS_YAML,
        status_flag_entries.len(),
        floor::STATUS_EFFECT_FLAGS,
    )?;

    let equip_src = fs::read_to_string(LSB_ITEM_EQUIPMENT_SQL)
        .with_context(|| format!("reading {LSB_ITEM_EQUIPMENT_SQL}"))?;
    let equip_entries = parse_sql_equip_rows(&equip_src)?;
    write_equip_info_table(&out_dir.join("equip_info_table.rs"), &equip_entries)?;
    check_scrape_count(
        "equip_info entries",
        LSB_ITEM_EQUIPMENT_SQL,
        equip_entries.len(),
        floor::EQUIP_INFO,
    )?;

    let usable_src = fs::read_to_string(LSB_ITEM_USABLE_SQL)
        .with_context(|| format!("reading {LSB_ITEM_USABLE_SQL}"))?;
    let usable_entries = parse_sql_usable_rows(&usable_src)?;
    write_item_usable_table(&out_dir.join("item_usable_table.rs"), &usable_entries)?;
    check_scrape_count(
        "item_usable entries",
        LSB_ITEM_USABLE_SQL,
        usable_entries.len(),
        floor::ITEM_USABLE,
    )?;

    let weapon_src = fs::read_to_string(LSB_ITEM_WEAPON_SQL)
        .with_context(|| format!("reading {LSB_ITEM_WEAPON_SQL}"))?;
    let weapon_skill_entries = parse_sql_weapon_skill_rows(&weapon_src)?;
    write_u16_u8_table(
        &out_dir.join("weapon_skill_table.rs"),
        "WEAPON_SKILL",
        LSB_ITEM_WEAPON_SQL,
        &weapon_skill_entries,
    )?;
    check_scrape_count(
        "item_weapon skill entries",
        LSB_ITEM_WEAPON_SQL,
        weapon_skill_entries.len(),
        floor::WEAPON_SKILL,
    )?;

    let emote_src =
        fs::read_to_string(LSB_EMOTE_H).with_context(|| format!("reading {LSB_EMOTE_H}"))?;
    let emote_entries = parse_cpp_enum_class(&emote_src, "Emote")?;
    for (id, name) in &emote_entries {
        if *id > u8::MAX as u32 {
            bail!("Emote id {id} ({name:?}) overflows u8 — emote.h widened its enum?");
        }
    }
    let mut out = String::new();
    out.push_str(&format!(
        "// AUTO-GENERATED by ffxi-vocab/build.rs from {LSB_EMOTE_H}.\n"
    ));
    out.push_str("// Do not edit by hand.\n");
    out.push_str("pub const EMOTES: &[(u8, &str)] = &[\n");
    for (id, name) in &emote_entries {
        out.push_str(&format!("    ({id}, {}),\n", rust_string_literal(name)));
    }
    out.push_str("];\n");
    fs::write(out_dir.join("emote_table.rs"), &out)?;
    check_scrape_count(
        "emote entries",
        LSB_EMOTE_H,
        emote_entries.len(),
        floor::EMOTE,
    )?;

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct EquipRow {
    item_id: u16,
    level: u8,
    jobs_mask: u32,
    slot_mask: u16,
}

fn parse_sql_equip_rows(src: &str) -> Result<Vec<EquipRow>> {
    let needle = "INSERT INTO `item_equipment` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(row) = parse_equip_row(&fields) else {
                continue;
            };
            out.push(row);
        }
    }
    if out.is_empty() {
        bail!(
            "parsed zero rows from `INSERT INTO item_equipment` — \
             SQL format may have changed"
        );
    }

    out.sort_by_key(|e| e.item_id);
    out.dedup_by_key(|e| e.item_id);
    Ok(out)
}

fn parse_equip_row(fields: &[&str]) -> Option<EquipRow> {
    let item_id: u16 = fields.first()?.trim().parse().ok()?;

    let level: u8 = fields.get(2)?.trim().parse::<u16>().ok()?.min(255) as u8;
    let jobs_mask: u32 = fields.get(4)?.trim().parse().ok()?;
    let slot_mask: u16 = fields.get(8)?.trim().parse().ok()?;
    Some(EquipRow {
        item_id,
        level,
        jobs_mask,
        slot_mask,
    })
}

#[derive(Debug, Clone, Copy)]
struct UsableRow {
    item_id: u16,
    valid_targets: u16,
    max_charges: u8,
}

/// Scrapes LSB `item_usable` (itemId, name, validTargets, activation,
/// animation, animationTime, maxCharges, useDelay, reuseDelay, aoe) — the
/// table 0x037 item-use consults for whether an item can fire at all.
fn parse_sql_usable_rows(src: &str) -> Result<Vec<UsableRow>> {
    let needle = "INSERT INTO `item_usable` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(row) = parse_usable_row(&fields) else {
                continue;
            };
            out.push(row);
        }
    }
    if out.is_empty() {
        bail!(
            "parsed zero rows from `INSERT INTO item_usable` — \
             SQL format may have changed"
        );
    }

    out.sort_by_key(|e| e.item_id);
    out.dedup_by_key(|e| e.item_id);
    Ok(out)
}

fn parse_usable_row(fields: &[&str]) -> Option<UsableRow> {
    let item_id: u16 = fields.first()?.trim().parse().ok()?;
    let valid_targets: u16 = fields.get(2)?.trim().parse().ok()?;
    let max_charges: u8 = fields.get(6)?.trim().parse::<u16>().ok()?.min(255) as u8;
    Some(UsableRow {
        item_id,
        valid_targets,
        max_charges,
    })
}

fn write_item_usable_table(out_path: &PathBuf, entries: &[UsableRow]) -> Result<()> {
    use std::io::Write;
    let mut f =
        fs::File::create(out_path).with_context(|| format!("creating {}", out_path.display()))?;
    writeln!(
        f,
        "// Auto-generated from {LSB_ITEM_USABLE_SQL} by build.rs — do not edit."
    )?;
    writeln!(f, "pub static ITEM_USABLE: &[(u16, u16, u8)] = &[")?;
    for row in entries {
        writeln!(
            f,
            "    ({}, {}, {}),",
            row.item_id, row.valid_targets, row.max_charges
        )?;
    }
    writeln!(f, "];")?;
    Ok(())
}

fn write_equip_info_table(out_path: &PathBuf, entries: &[EquipRow]) -> Result<()> {
    use std::io::Write;
    let mut f =
        fs::File::create(out_path).with_context(|| format!("creating {}", out_path.display()))?;
    writeln!(
        f,
        "// Auto-generated from {LSB_ITEM_EQUIPMENT_SQL} by build.rs — do not edit."
    )?;
    writeln!(f, "pub static EQUIP_INFO: &[(u16, u8, u32, u16)] = &[")?;
    for row in entries {
        writeln!(
            f,
            "    ({}, {}, {}, {}),",
            row.item_id, row.level, row.jobs_mask, row.slot_mask
        )?;
    }
    writeln!(f, "];")?;
    Ok(())
}

fn lua_table_label(table: &str) -> &'static str {
    match table {
        "channel" => "msg_channel",
        "area" => "msg_area",
        "actionModifier" => "msg_action_modifier",
        "system" => "msg_system",
        _ => "msg_<unknown>",
    }
}

fn parse_lua_table(src: &str, table: &str) -> Result<Vec<(u16, String)>> {
    let needle = format!("xi.msg.{table} =");
    let header = src
        .find(&needle)
        .with_context(|| format!("could not locate `{needle}` in msg.lua"))?;

    let body_start = src[header..]
        .find('{')
        .with_context(|| format!("no opening `{{` after `{needle}`"))?
        + header
        + 1;
    let body_end = src[body_start..]
        .find('}')
        .with_context(|| format!("no closing `}}` after `{needle}`"))?
        + body_start;
    let body = &src[body_start..body_end];

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in body.lines() {
        let line = line.trim();
        let Some(eq) = line.find('=') else { continue };
        let ident = line[..eq].trim();
        if ident.is_empty()
            || !ident
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        if !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }

        let tail = &line[eq + 1..];
        let Some(comma) = tail.find(',') else {
            continue;
        };
        let num_str = tail[..comma].trim();
        let id: u16 = lsb_scrape::parse_int_lit(num_str).unwrap_or(continue_marker());
        if id == continue_marker() {
            continue;
        }
        let after_comma = &tail[comma + 1..];
        let Some(slash) = after_comma.find("--") else {
            continue;
        };
        let comment = after_comma[slash + 2..].trim().to_string();
        if comment.is_empty() || is_non_message_annotation(&comment) {
            continue;
        }
        if seen.insert(id) {
            out.push((id, comment));
        }
    }
    if out.is_empty() {
        bail!("parsed zero entries for xi.msg.{table} — msg.lua format may have changed");
    }
    Ok(out)
}

fn continue_marker() -> u16 {
    u16::MAX
}

fn parse_msg_basic(src: &str) -> Result<Vec<(u16, String)>> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();

        let Some(eq) = line.find('=') else { continue };

        let Some(comma) = line[eq..].find(',') else {
            continue;
        };
        let comma = eq + comma;

        let ident = line[..eq].trim();
        if ident.is_empty()
            || !ident
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        if !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }

        let num_str = line[eq + 1..comma].trim();
        let id: u16 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let tail = &line[comma + 1..];
        let Some(slash) = tail.find("//") else {
            continue;
        };
        let mut comment = tail[slash + 2..].trim().to_string();

        if let Some(stripped) = comment.strip_suffix("*/") {
            comment = stripped.trim_end().to_string();
        }
        if is_non_message_annotation(&comment) {
            continue;
        }
        if comment.is_empty() {
            continue;
        }
        out.push((id, comment));
    }
    if out.is_empty() {
        bail!("parsed zero msg_basic entries — header format may have changed");
    }
    Ok(out)
}

fn is_non_message_annotation(comment: &str) -> bool {
    let low = comment.to_ascii_lowercase();

    low.contains("display nothing")
        || low.contains("does not print")
        || low.contains("does not work")
        || low.contains("does nothing")
        || low.starts_with("(assumed")
        || low.starts_with("assumed")
        || low.starts_with("todo")
        || low.starts_with("unused")
}

fn parse_spell_skill_rows(src: &str) -> Result<Vec<(u16, u8)>> {
    let mut skill_consts: std::collections::HashMap<&str, u8> = std::collections::HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("SET ") else {
            continue;
        };
        let Some((name, val)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("@SKILL_") {
            continue;
        }
        let val = val.trim().trim_end_matches(';').trim();
        if let Ok(n) = val.parse::<u8>() {
            skill_consts.insert(name, n);
        }
    }

    let needle = "INSERT INTO `spell_list` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(id_str) = fields.first().map(|s| s.trim()) else {
                continue;
            };
            let Ok(id) = id_str.parse::<u16>() else {
                continue;
            };
            let skill_raw = fields.get(8).map(|s| s.trim()).unwrap_or("");
            let skill = skill_consts
                .get(skill_raw)
                .copied()
                .or_else(|| skill_raw.parse::<u8>().ok());
            let Some(skill) = skill else { continue };
            if skill == 0 {
                continue;
            }
            out.push((id, skill));
        }
    }
    if out.is_empty() {
        bail!("parsed zero spell-skill rows — spell_list.sql format may have changed");
    }
    Ok(out)
}

/// Scrape `item_basic` rows into (itemid, flags), resolving the `SET @FLAG_* = n;`
/// variables the flags column references (e.g. `@FLAG_EX | @FLAG_NODELIVERY`).
/// Zero-flag rows are dropped: the table is sparse and lookup defaults to 0.
fn parse_sql_item_flags(src: &str) -> Result<Vec<(u16, u32)>> {
    let mut vars: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("SET @") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_end_matches(';');
        let value = value.split("--").next().unwrap_or("").trim();
        let value = value.trim_end_matches(';').trim();
        if let Ok(v) = value.parse::<u32>() {
            vars.insert(name.trim().to_string(), v);
        }
    }

    let eval = |expr: &str| -> Option<u32> {
        let mut acc = 0u32;
        for term in expr.split('|') {
            let term = term.trim();
            let v = if let Some(name) = term.strip_prefix('@') {
                *vars.get(name)?
            } else {
                term.parse::<u32>().ok()?
            };
            acc |= v;
        }
        Some(acc)
    };

    let needle = "INSERT INTO `item_basic` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(Ok(id)) = fields.first().map(|s| s.trim().parse::<u16>()) else {
                continue;
            };
            let Some(flags) = fields.get(7).and_then(|s| eval(s.trim())) else {
                bail!(
                    "item_basic row {id}: unresolvable flags expression {:?}",
                    fields.get(7)
                );
            };
            if flags != 0 {
                out.push((id, flags));
            }
        }
    }
    if out.is_empty() {
        bail!("parsed zero item_basic flag rows — SQL format may have changed");
    }
    Ok(out)
}

/// Scrape `item_basic` rows into (itemid, stackSize). Rows that do not stack
/// are dropped: the table is sparse and lookup defaults to 1.
fn parse_sql_item_stack_sizes(src: &str) -> Result<Vec<(u16, u8)>> {
    const ITEMID_FIELD: usize = 0;
    const STACK_SIZE_FIELD: usize = 6;
    let needle = "INSERT INTO `item_basic` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(Ok(id)) = fields.get(ITEMID_FIELD).map(|s| s.trim().parse::<u16>()) else {
                continue;
            };
            let Some(Ok(stack)) = fields.get(STACK_SIZE_FIELD).map(|s| s.trim().parse::<u8>())
            else {
                bail!(
                    "item_basic row {id}: unparseable stackSize {:?}",
                    fields.get(STACK_SIZE_FIELD)
                );
            };
            if stack > 1 {
                out.push((id, stack));
            }
        }
    }
    if out.is_empty() {
        bail!("parsed zero item_basic stackSize rows — SQL format may have changed");
    }
    Ok(out)
}

/// Scrape `status_effects` rows into (id, flags), resolving the `SET @FLAG_* = n;`
/// variables the flags column references (e.g. `@FLAG_DEATH | @FLAG_NO_CANCEL`).
/// The client keys the buff-cancel packet (0x0F1) and its status icons on the
/// effect id, so this table is consumed by icon id (icon == effect id in LSB's
/// default assignment). Zero-flag rows are dropped: lookup defaults to 0.
struct StatusEffect {
    key: String,
    id: u16,
    flags: Vec<String>,
}

/// The `status_effects:` map of data/status_effects.yaml, in file order. The
/// key is the effect's enum identifier (xi.effect.SLEEP_I is `sleep_i`); the
/// optional `name:` field is the server's own display name and is not read
/// here because the client keys names on the identifier.
fn parse_status_effects(src: &str) -> Result<Vec<StatusEffect>> {
    let root = parse_yaml(src)?;
    let entries = root
        .get("status_effects")
        .and_then(Yaml::as_map)
        .context("no `status_effects:` map")?;
    let mut out = Vec::with_capacity(entries.len());
    for (key, effect) in entries {
        let id: u16 = effect
            .field("id")?
            .with_context(|| format!("status effect `{key}` has no id"))?;
        let flags = match effect.get("flags") {
            None => Vec::new(),
            Some(node) => node
                .as_seq()
                .with_context(|| format!("status effect `{key}`: `flags` is not a list"))?
                .iter()
                .map(|flag| {
                    flag.as_str()
                        .map(str::to_string)
                        .with_context(|| format!("status effect `{key}`: non-scalar flag"))
                })
                .collect::<Result<Vec<String>>>()?,
        };
        out.push(StatusEffect {
            key: key.clone(),
            id,
            flags,
        });
    }
    if out.is_empty() {
        bail!("parsed zero status effects");
    }
    Ok(out)
}

/// Each effect's flag names OR-ed into the u32 the server keeps
/// (vendor/server/src/map/data/datasets/status_effects/dataset.cpp); effects
/// with no bits set are left out so a lookup miss and "no flags" agree.
fn status_effect_flag_words(
    effects: &[StatusEffect],
    flag_bits: &[(String, u32)],
) -> Result<Vec<(u16, u32)>> {
    let mut out = Vec::new();
    for effect in effects {
        let mut word = 0u32;
        for flag in &effect.flags {
            let (_, bits) = flag_bits
                .iter()
                .find(|(name, _)| name == flag)
                .with_context(|| {
                    format!(
                        "status effect `{}`: flag `{flag}` is not in {LSB_STATUS_EFFECT_FLAG_YAML}",
                        effect.key
                    )
                })?;
            word |= bits;
        }
        if word != 0 {
            out.push((effect.id, word));
        }
    }
    if out.is_empty() {
        bail!("parsed zero status_effects flag rows");
    }
    Ok(out)
}

/// `item_weapon` (itemId, skill). Skill 0 rows carry no skill type and are
/// dropped so a lookup miss and "no skill" are the same answer.
fn parse_sql_weapon_skill_rows(src: &str) -> Result<Vec<(u16, u8)>> {
    const ITEM_ID_FIELD: usize = 0;
    const SKILL_FIELD: usize = 2;

    let needle = "INSERT INTO `item_weapon` VALUES ";
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(rest) = line.trim().strip_prefix(needle) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let id = fields
                .get(ITEM_ID_FIELD)
                .and_then(|s| s.trim().parse::<u16>().ok());
            let skill = fields
                .get(SKILL_FIELD)
                .and_then(|s| s.trim().parse::<u8>().ok());
            if let (Some(id), Some(skill)) = (id, skill) {
                if skill != 0 {
                    out.push((id, skill));
                }
            }
        }
    }
    if out.is_empty() {
        bail!(
            "parsed zero rows from `INSERT INTO item_weapon` — \
             the LSB dump layout changed under {LSB_ITEM_WEAPON_SQL}"
        );
    }
    Ok(out)
}

/// One `Schedule` per (run, crossing zone) of every data/zones/<zone>/zone.yaml
/// `transport.runs` entry that carries riders, mirroring
/// vendor/server/src/map/transports/ship_handler.cpp ShipHandler::registerVoyage:
/// a run with no `docked` phase opens no door, so it feeds no crossing. Phase
/// bounds follow convertPhases in
/// vendor/server/src/map/data/datasets/zones/settings/dataset.cpp: phases run
/// back to back from the cycle start and at most one may leave its length out
/// to take whatever the cycle has left.
fn scrape_transport() -> Result<()> {
    println!("cargo:rerun-if-changed={LSB_ZONES_DATA_DIR}");
    println!("cargo:rerun-if-changed={LSB_ZONE_ENUM_YAML}");
    let zone_src = fs::read_to_string(LSB_ZONE_ENUM_YAML)
        .with_context(|| format!("reading {LSB_ZONE_ENUM_YAML}"))?;
    let zone_ids = parse_yaml_enum_values(&zone_src)
        .with_context(|| format!("parsing {LSB_ZONE_ENUM_YAML}"))?;
    let zone_id = |name: &str| -> Result<u16> {
        let (_, id) = zone_ids
            .iter()
            .find(|(key, _)| key == name)
            .with_context(|| format!("`{name}` is not in {LSB_ZONE_ENUM_YAML}"))?;
        u16::try_from(*id).with_context(|| format!("zone `{name}` id {id} overflows u16"))
    };

    let mut output = String::from("pub const SCHEDULES: &[Schedule] = &[\n");
    let mut count = 0;
    for (zone_key, path) in zone_data_files(LSB_ZONES_DATA_DIR)? {
        let src =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let root = parse_yaml(&src).with_context(|| format!("parsing {}", path.display()))?;
        let Some(transport) = root.get("transport") else {
            continue;
        };
        let ship: u32 = transport
            .field("ship")?
            .with_context(|| format!("{zone_key}: transport without a ship"))?;
        let Some(runs) = transport.get("runs").and_then(Yaml::as_map) else {
            continue;
        };
        for (name, run) in runs {
            let crossings: Vec<&str> = match run.get("voyage") {
                None => Vec::new(),
                Some(node) => node
                    .as_seq()
                    .with_context(|| format!("transport {name}: `voyage` is not a list"))?
                    .iter()
                    .map(|zone| {
                        zone.as_str()
                            .with_context(|| format!("transport {name}: non-scalar voyage zone"))
                    })
                    .collect::<Result<_>>()?,
            };
            if crossings.is_empty() {
                continue;
            }
            let phases = run
                .get("phases")
                .and_then(Yaml::as_seq)
                .with_context(|| format!("transport {name}: no phases"))?;
            let every: u32 = run.field("every")?.unwrap_or(0);
            let mut stated = 0u32;
            let mut open = 0usize;
            for phase in phases {
                match phase.field::<u32>("seconds")? {
                    Some(seconds) => stated += seconds,
                    None => open += 1,
                }
            }
            anyhow::ensure!(
                open <= 1 && stated <= every,
                "transport {name}: phases run {stated}s of a {every}s cycle with {open} open-ended"
            );
            let mut cursor = 0u32;
            let mut boarding_ends = None;
            let mut departs = None;
            for phase in phases {
                let length = phase.field::<u32>("seconds")?.unwrap_or(every - stated);
                let state: String = phase
                    .field("state")?
                    .with_context(|| format!("transport {name}: phase without a state"))?;
                match state.as_str() {
                    "docked" => boarding_ends = Some(cursor + length),
                    "departing" => {
                        departs = Some(cursor + phase.field::<u32>("hide")?.unwrap_or(0))
                    }
                    _ => {}
                }
                cursor += length;
            }
            let Some(boarding_ends) = boarding_ends else {
                continue;
            };
            let departs = departs.with_context(|| {
                format!("transport {name}: docked run without a departing phase")
            })?;
            let disembark: u32 = run.field("disembark")?.with_context(|| {
                format!("transport {name}: carries riders but states no disembark point")
            })?;
            let boundary: u16 = run.field("boundary")?.unwrap_or(0);
            let offset: u32 = run.field("offset")?.unwrap_or(0);
            for crossing in crossings {
                let voyage_zone = zone_id(crossing)?;
                output.push_str(&format!("Schedule {{ voyage_zone: {voyage_zone}, ship: {ship}, boundary: {boundary}, offset: {offset}, every: {every}, boarding_ends: {boarding_ends}, departs: {departs}, disembark: {disembark} }},\n"));
                count += 1;
            }
        }
    }
    check_scrape_count(
        "transport voyages",
        LSB_ZONES_DATA_DIR,
        count,
        floor::TRANSPORT,
    )?;
    output.push_str("];\n");
    const MODEL_SOURCE: &str = "../vendor/server/src/map/packets/entity_update.h";
    println!("cargo:rerun-if-changed={MODEL_SOURCE}");
    let model_types =
        lsb_scrape::parse_cpp_plain_enum(&fs::read_to_string(MODEL_SOURCE)?, "MODELTYPE")?;
    for name in ["MODEL_ELEVATOR", "MODEL_SHIP"] {
        let (value, _) = model_types
            .iter()
            .find(|(_, key)| key == name)
            .with_context(|| format!("missing {name} in MODELTYPE"))?;
        output.push_str(&format!("pub const {name}: u16 = {value};\n"));
    }
    fs::write(
        PathBuf::from(std::env::var("OUT_DIR")?).join("transport_table.rs"),
        output,
    )?;
    Ok(())
}
