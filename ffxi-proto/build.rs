use std::{fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use lsb_scrape::{
    check_scrape_count, parse_cpp_plain_enum, parse_cpp_u32_str_map, parse_int_lit,
    parse_lua_scalar_field, parse_packet_enum, parse_yaml_enum_values, write_u16_table,
    write_u16_u16_table,
};

const LSB_BLOWFISH_CPP: &str = "../vendor/server/src/common/blowfish.cpp";
const LSB_COMPRESS_DAT: &str = "../vendor/server/res/compress.dat";
const LSB_DECOMPRESS_DAT: &str = "../vendor/server/res/decompress.dat";
const LSB_ZONE_YAML: &str = "../vendor/server/data/enums/zone.yaml";
const LSB_ZONE_SCRIPTS_DIR: &str = "../vendor/server/scripts/zones";
const LSB_FISHINGUTILS_H: &str = "../vendor/server/src/map/utils/fishingutils.h";
const LSB_PACKET_S2C_H: &str = "../vendor/server/src/map/enums/packet_s2c.h";
const LSB_PACKET_C2S_H: &str = "../vendor/server/src/map/enums/packet_c2s.h";
const LSB_AUTH_SESSION_H: &str = "../vendor/server/src/login/auth_session.h";
const LSB_LOGIN_HELPERS_H: &str = "../vendor/server/src/login/login_helpers.h";
const LSB_LOGIN_ERRORS_H: &str = "../vendor/server/src/login/login_errors.h";
const LSB_ZONE_ENTITIES_CPP: &str = "../vendor/server/src/map/zone_entities.cpp";
const LSB_SEARCH_HANDLER_H: &str = "../vendor/server/src/search/search_handler.h";
const LSB_LOGIN_LUA: &str = "../vendor/server/settings/default/login.lua";
const LSB_NETWORK_LUA: &str = "../vendor/server/settings/default/network.lua";
const LSB_AUTOTRANSLATE_CPP: &str = "../vendor/server/src/map/autotranslate.cpp";
/// The retail DAT LSB's map was generated from, per the script quoted atop
/// autotranslate.cpp.
const AUTOTRANSLATE_DAT: &str = "ROM/168/25.DAT";
const LSB_AUTOTRANSLATE_MAP_DECL: &str = "const std::map<unsigned int, const char*> values =";
const LSB_S2C_PACKET_DIR: &str = "../vendor/server/src/map/packets/s2c";

/// The `xi::` enums LSB generates from `data/enums/*.yaml` at its own build
/// time: their headers are not in the tree, so a struct member typed with one
/// only has a width if the yaml's `meta.cpp.underlying` supplies it.
const LSB_GENERATED_ENUMS: &[(&str, &str)] = &[
    ("xi::Job", "../vendor/server/data/enums/job.yaml"),
    ("xi::Weather", "../vendor/server/data/enums/weather.yaml"),
];

/// The s2c bodies ffxi-proto decodes by hard-coded offset, as
/// (emitted module, header, struct whose `offsetof` the decoder must match).
/// Each entry's offsets reach the decoders as `ffxi_proto::s2c_layout::<module>`
/// consts and are const-asserted there against the hand-written ones.
const S2C_LAYOUTS: &[(&str, &str, &str)] = &[
    (
        "login",
        "0x00a_login.h",
        "GP_SERV_COMMAND_LOGIN::PacketData",
    ),
    (
        "grap_list",
        "0x051_grap_list.h",
        "GP_SERV_COMMAND_GRAP_LIST::PacketData",
    ),
    (
        "weather",
        "0x057_weather.h",
        "GP_SERV_COMMAND_WEATHER::PacketData",
    ),
    (
        "clistatus",
        "0x061_clistatus.h",
        "GP_SERV_COMMAND_CLISTATUS::PacketData",
    ),
    (
        "group_list",
        "0x0dd_group_list.h",
        "GP_SERV_COMMAND_GROUP_LIST::PacketData",
    ),
    (
        "abil_recast",
        "0x119_abil_recast.h",
        "GP_SERV_COMMAND_ABIL_RECAST::PacketData",
    ),
];
const PATCH_STAMP_DATE_LEN: usize = 8;
const SUBKEY_LEN: usize = 4168;
const SEARCH_BASE_KEY_LEN: usize = 24;

// Sanity band for the scraped streaming radius: a yalm figure outside it means the parse
// grabbed the wrong token, not that LSB retuned.
const MIN_PLAUSIBLE_YALMS: f32 = 1.0;
const MAX_PLAUSIBLE_YALMS: f32 = 1000.0;

/// Smallest row count each scrape can return and still plausibly have parsed
/// its source; the argument is the count the pinned vendor tree yields today
/// (kuluu-m4yk).
mod floor {
    use lsb_scrape::scrape_floor;

    pub const FISHING_ZONE_OFFSET: usize = scrape_floor(115);
    pub const FISHING_MESSAGE_KIND: usize = scrape_floor(41);
    pub const PACKET_NAMES_S2C: usize = scrape_floor(148);
    pub const PACKET_NAMES_C2S: usize = scrape_floor(130);
    pub const TCP_REQUEST_TYPE: usize = scrape_floor(8);
    pub const AUTOTRANSLATE: usize = scrape_floor(28347);
    pub const S2C_LAYOUT_FIELD: usize = scrape_floor(158);
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={LSB_BLOWFISH_CPP}");
    println!("cargo:rerun-if-changed={LSB_COMPRESS_DAT}");
    println!("cargo:rerun-if-changed={LSB_DECOMPRESS_DAT}");
    println!("cargo:rerun-if-changed={LSB_ZONE_YAML}");
    println!("cargo:rerun-if-changed={LSB_ZONE_SCRIPTS_DIR}");
    println!("cargo:rerun-if-changed={LSB_FISHINGUTILS_H}");
    println!("cargo:rerun-if-changed={LSB_PACKET_S2C_H}");
    println!("cargo:rerun-if-changed={LSB_PACKET_C2S_H}");
    println!("cargo:rerun-if-changed={LSB_AUTH_SESSION_H}");
    println!("cargo:rerun-if-changed={LSB_ZONE_ENTITIES_CPP}");
    println!("cargo:rerun-if-changed={LSB_SEARCH_HANDLER_H}");
    println!("cargo:rerun-if-changed={LSB_LOGIN_LUA}");
    println!("cargo:rerun-if-changed={LSB_NETWORK_LUA}");
    println!("cargo:rerun-if-changed={LSB_AUTOTRANSLATE_CPP}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("OUT_DIR not set")?);

    let src = fs::read_to_string(LSB_BLOWFISH_CPP)
        .with_context(|| format!("reading {LSB_BLOWFISH_CPP}"))?;
    let subkey_sig = format!("uint8 subkey[{SUBKEY_LEN}]");
    let start = src
        .find(&subkey_sig)
        .with_context(|| format!("could not locate `{subkey_sig}` in blowfish.cpp"))?;
    let body_start = src[start..]
        .find('{')
        .context("could not locate opening `{` of subkey table")?
        + start
        + 1;
    let body_end = src[body_start..]
        .find('}')
        .context("could not locate closing `}` of subkey table")?
        + body_start;
    let body = &src[body_start..body_end];

    let mut bytes = Vec::with_capacity(SUBKEY_LEN);
    for tok in body.split([',', ' ', '\n', '\r', '\t']) {
        let t = tok.trim();
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            let b =
                u8::from_str_radix(hex, 16).with_context(|| format!("parsing hex byte `{t}`"))?;
            bytes.push(b);
        } else if !t.is_empty() {
            bail!("unexpected token in subkey table: {t:?}");
        }
    }
    if bytes.len() != SUBKEY_LEN {
        bail!(
            "extracted {} subkey bytes, expected {SUBKEY_LEN}",
            bytes.len()
        );
    }
    fs::write(out_dir.join("blowfish_subkey.bin"), &bytes)?;

    let compress =
        fs::read(LSB_COMPRESS_DAT).with_context(|| format!("reading {LSB_COMPRESS_DAT}"))?;
    let decompress =
        fs::read(LSB_DECOMPRESS_DAT).with_context(|| format!("reading {LSB_DECOMPRESS_DAT}"))?;
    if compress.len() % 4 != 0 || decompress.len() % 4 != 0 {
        bail!(
            "compress.dat ({}) / decompress.dat ({}) byte counts must be multiples of 4",
            compress.len(),
            decompress.len()
        );
    }
    fs::write(out_dir.join("compress.dat"), &compress)?;
    fs::write(out_dir.join("decompress.dat"), &decompress)?;

    let fishing_offsets = parse_zone_fishing_message_offsets()?;
    write_u16_u16_table(
        &out_dir.join("fishing_zone_offset_table.rs"),
        "FISHING_ZONE_OFFSET",
        LSB_ZONE_SCRIPTS_DIR,
        &fishing_offsets,
    )?;
    let fishing_kinds = parse_fish_message_offset_enum()?;
    write_fish_message_consts(&out_dir.join("fishing_message_consts.rs"), &fishing_kinds)?;
    write_fish_message_tables(&out_dir.join("fishing_message_tables.rs"), &fishing_kinds)?;
    check_scrape_count(
        "zone fishing-message offsets",
        LSB_ZONE_SCRIPTS_DIR,
        fishing_offsets.len(),
        floor::FISHING_ZONE_OFFSET,
    )?;
    check_scrape_count(
        "fishing message kinds",
        LSB_FISHINGUTILS_H,
        fishing_kinds.len(),
        floor::FISHING_MESSAGE_KIND,
    )?;

    let pkt_s2c_src = fs::read_to_string(LSB_PACKET_S2C_H)
        .with_context(|| format!("reading {LSB_PACKET_S2C_H}"))?;
    let s2c_names = parse_packet_enum(&pkt_s2c_src, "GP_SERV_COMMAND_")?;
    write_u16_table(
        &out_dir.join("packet_names_s2c_table.rs"),
        "PACKET_NAMES_S2C",
        LSB_PACKET_S2C_H,
        &s2c_names,
    )?;
    check_scrape_count(
        "s2c packet names",
        LSB_PACKET_S2C_H,
        s2c_names.len(),
        floor::PACKET_NAMES_S2C,
    )?;

    let pkt_c2s_src = fs::read_to_string(LSB_PACKET_C2S_H)
        .with_context(|| format!("reading {LSB_PACKET_C2S_H}"))?;
    let c2s_names = parse_packet_enum(&pkt_c2s_src, "GP_CLI_COMMAND_")?;
    write_u16_table(
        &out_dir.join("packet_names_c2s_table.rs"),
        "PACKET_NAMES_C2S",
        LSB_PACKET_C2S_H,
        &c2s_names,
    )?;
    check_scrape_count(
        "c2s packet names",
        LSB_PACKET_C2S_H,
        c2s_names.len(),
        floor::PACKET_NAMES_C2S,
    )?;

    check_map_opcodes_against_lsb(&s2c_names, &c2s_names)?;
    let event_position_opcode = c2s_names
        .iter()
        .find(|(_, name)| name == "EVENTENDXZY")
        .context("EVENTENDXZY opcode")?
        .0;
    let mut event_wire = format!("pub const OPCODE: u16 = {event_position_opcode};\n");
    for (path, enum_name, member, constant) in [
        (
            "../vendor/server/src/map/packets/c2s/0x05b_eventend.h",
            "GP_CLI_COMMAND_EVENTEND_MODE",
            "UpdatePending",
            "UPDATE_PENDING",
        ),
        (
            "../vendor/server/src/map/packets/s2c/0x052_eventucoff.h",
            "GP_SERV_COMMAND_EVENTUCOFF_MODE",
            "EventRecvPending",
            "EVENT_RECV_PENDING",
        ),
    ] {
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(path)?;
        let values = lsb_scrape::parse_cpp_enum_class(&source, enum_name)?;
        let value = values
            .iter()
            .find(|(_, name)| name == member)
            .context(member)?
            .0;
        event_wire.push_str(&format!("pub const {constant}: u32 = {value};\n"));
    }
    fs::write(out_dir.join("event_position_wire.rs"), event_wire)?;

    let auth_session_src = fs::read_to_string(LSB_AUTH_SESSION_H)
        .with_context(|| format!("reading {LSB_AUTH_SESSION_H}"))?;
    let xiloader_version = parse_supported_xiloader_version(&auth_session_src)?;
    let mut out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_AUTH_SESSION_H}.\n\
         // Do not edit by hand.\n\
         pub const SUPPORTED_XILOADER_VERSION: [u8; 3] = [{}, {}, {}];\n",
        xiloader_version[0], xiloader_version[1], xiloader_version[2],
    );
    for enum_name in ["login_cmd", "login_result"] {
        let members = lsb_scrape::parse_cpp_enum_class(&auth_session_src, enum_name)
            .with_context(|| format!("{enum_name} in {LSB_AUTH_SESSION_H}"))?;
        out.push_str(&format!("pub mod {enum_name} {{\n"));
        for (value, name) in &members {
            let value = u8::try_from(*value)
                .with_context(|| format!("{enum_name}::{name} = {value} exceeds uint8_t"))?;
            out.push_str(&format!("    pub const {name}: u8 = {value:#04x};\n"));
        }
        out.push_str("}\n");
    }
    fs::write(out_dir.join("xiloader_version_table.rs"), &out)?;
    write_lobby_tables(&out_dir.join("lobby_tables.rs"))?;
    println!(
        "ffxi-proto: scraped SupportedXiloaderVersion {}.{}.{}",
        xiloader_version[0], xiloader_version[1], xiloader_version[2],
    );

    let zone_entities_src = fs::read_to_string(LSB_ZONE_ENTITIES_CPP)
        .with_context(|| format!("reading {LSB_ZONE_ENTITIES_CPP}"))?;
    let entity_render_distance =
        parse_cpp_constexpr_f32(&zone_entities_src, "ENTITY_RENDER_DISTANCE")?;
    let out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_ZONE_ENTITIES_CPP}.\n\
         // Do not edit by hand.\n\
         pub const ENTITY_RENDER_DISTANCE_YALMS: f32 = {entity_render_distance:?};\n",
    );
    fs::write(out_dir.join("entity_stream_table.rs"), &out)?;
    println!("ffxi-proto: scraped ENTITY_RENDER_DISTANCE {entity_render_distance}");

    let search_handler_src = fs::read_to_string(LSB_SEARCH_HANDLER_H)
        .with_context(|| format!("reading {LSB_SEARCH_HANDLER_H}"))?;
    let base_key = parse_search_base_key(&search_handler_src)?;
    let tcp_types = parse_cpp_plain_enum(&search_handler_src, "TCPREQUESTTYPE")?;
    for (id, name) in &tcp_types {
        if *id > u8::MAX as u32 {
            bail!("TCPREQUESTTYPE {name} = {id} overflows u8 — search_handler.h widened its enum?");
        }
    }
    let mut out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_SEARCH_HANDLER_H}.\n\
         // Do not edit by hand.\n\
         pub const SEARCH_BASE_KEY: [u8; {SEARCH_BASE_KEY_LEN}] = [\n"
    );
    for b in &base_key {
        out.push_str(&format!("    {b:#04x},\n"));
    }
    out.push_str("];\n");
    for (id, name) in &tcp_types {
        out.push_str(&format!("pub const {name}: u8 = {id:#04x};\n"));
    }
    fs::write(out_dir.join("search_handler_table.rs"), &out)?;
    check_scrape_count(
        "TCPREQUESTTYPE entries (plus the search base key)",
        LSB_SEARCH_HANDLER_H,
        tcp_types.len(),
        floor::TCP_REQUEST_TYPE,
    )?;

    write_s2c_layouts(&out_dir.join("s2c_layout.rs"))?;
    write_login_settings(
        &out_dir.join("login_settings_table.rs"),
        &out_dir.join("map_settings_table.rs"),
    )?;
    write_autotranslate_table(&out_dir.join("autotranslate_table.rs"))?;

    Ok(())
}

fn write_login_settings(out_path: &std::path::Path, map_out_path: &std::path::Path) -> Result<()> {
    let src =
        fs::read_to_string(LSB_LOGIN_LUA).with_context(|| format!("reading {LSB_LOGIN_LUA}"))?;
    let client_ver = parse_lua_scalar_field(&src, "CLIENT_VER")?;
    if !is_patch_stamp(&client_ver) {
        bail!("login.CLIENT_VER {client_ver:?} is not a YYYYMMDD_N patch stamp");
    }
    let ver_lock = parse_lua_scalar_field(&src, "VER_LOCK")?;
    let ver_lock: u8 = ver_lock
        .parse()
        .with_context(|| format!("login.VER_LOCK {ver_lock:?} is not a u8"))?;
    let net = fs::read_to_string(LSB_NETWORK_LUA)
        .with_context(|| format!("reading {LSB_NETWORK_LUA}"))?;
    let port = |key: &str| -> Result<u16> {
        let raw = parse_lua_scalar_field(&net, key)?;
        raw.parse()
            .with_context(|| format!("network.{key} {raw:?} is not a u16"))
    };
    let auth_port = port("LOGIN_AUTH_PORT")?;
    let data_port = port("LOGIN_DATA_PORT")?;
    let view_port = port("LOGIN_VIEW_PORT")?;
    let map_port = port("MAP_PORT")?;
    let out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_LOGIN_LUA} and {LSB_NETWORK_LUA}.\n\
         // Do not edit by hand.\n\
         pub const LSB_CLIENT_VER: &str = {client_ver:?};\n\
         pub const LSB_DEFAULT_VER_LOCK: u8 = {ver_lock};\n\
         pub const LOGIN_AUTH_PORT: u16 = {auth_port};\n\
         pub const LOGIN_DATA_PORT: u16 = {data_port};\n\
         pub const LOGIN_VIEW_PORT: u16 = {view_port};\n",
    );
    fs::write(out_path, &out)?;
    let map_out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_NETWORK_LUA}.\n\
         // Do not edit by hand.\n\
         pub const MAP_PORT: u16 = {map_port};\n",
    );
    fs::write(map_out_path, &map_out)?;
    println!(
        "ffxi-proto: scraped login.CLIENT_VER {client_ver} login.VER_LOCK {ver_lock} \
         network ports auth={auth_port} data={data_port} view={view_port} map={map_port}"
    );
    Ok(())
}

fn is_patch_stamp(stamp: &str) -> bool {
    let Some((date, seq)) = stamp.split_once('_') else {
        return false;
    };
    date.len() == PATCH_STAMP_DATE_LEN
        && date.bytes().all(|b| b.is_ascii_digit())
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit())
}

fn write_autotranslate_table(out_path: &std::path::Path) -> Result<()> {
    let src = fs::read_to_string(LSB_AUTOTRANSLATE_CPP)
        .with_context(|| format!("reading {LSB_AUTOTRANSLATE_CPP}"))?;
    let mut rows = parse_cpp_u32_str_map(&src, LSB_AUTOTRANSLATE_MAP_DECL)?;
    check_scrape_count(
        "autotranslate phrases",
        LSB_AUTOTRANSLATE_CPP,
        rows.len(),
        floor::AUTOTRANSLATE,
    )?;
    rows.sort_by_key(|(key, _)| *key);
    let mut out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_AUTOTRANSLATE_CPP}, LSB's dump of\n\
         // the retail auto-translate dictionary {AUTOTRANSLATE_DAT}. Do not edit by hand.\n\
         // Keys are LSB's doLookup key: type | lang << 8 | category << 16 | index << 24.\n\
         pub static AUTOTRANSLATE_TABLE: &[(u32, &str)] = &[\n"
    );
    for (key, text) in &rows {
        out.push_str(&format!("    ({key:#010x}, {text:?}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

fn parse_search_base_key(src: &str) -> Result<[u8; SEARCH_BASE_KEY_LEN]> {
    let needle = format!("uint8 key[{SEARCH_BASE_KEY_LEN}]");
    let start = src
        .find(&needle)
        .with_context(|| format!("could not locate `{needle}` in search_handler.h"))?;
    let body_start = src[start..]
        .find('{')
        .context("could not locate opening `{` of search key table")?
        + start
        + 1;
    let body_end = src[body_start..]
        .find('}')
        .context("could not locate closing `}` of search key table")?
        + body_start;

    let mut bytes = Vec::with_capacity(SEARCH_BASE_KEY_LEN);
    for tok in src[body_start..body_end].split([',', ' ', '\n', '\r', '\t']) {
        let t = tok.trim();
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            bytes.push(
                u8::from_str_radix(hex, 16).with_context(|| format!("parsing hex byte `{t}`"))?,
            );
        } else if !t.is_empty() {
            bail!("unexpected token in search key table: {t:?}");
        }
    }
    let arr: [u8; SEARCH_BASE_KEY_LEN] = bytes.as_slice().try_into().map_err(|_| {
        anyhow::anyhow!(
            "extracted {} search key bytes, expected {SEARCH_BASE_KEY_LEN}",
            bytes.len()
        )
    })?;
    Ok(arr)
}

fn parse_cpp_constexpr_f32(src: &str, name: &str) -> Result<f32> {
    let mut hits = src.lines().filter_map(|line| {
        let (decl, rhs) = line.split_once('=')?;
        let decl = decl.trim();
        if !decl.ends_with(name) || !decl.starts_with("constexpr") {
            return None;
        }
        Some(rhs.trim().trim_end_matches(';').trim())
    });
    let lit = hits
        .next()
        .with_context(|| format!("could not locate `constexpr … {name} =` in source"))?;
    if hits.next().is_some() {
        bail!("`{name}` is declared more than once — which one is authoritative?");
    }
    let value = lit
        .trim_end_matches(['f', 'F'])
        .parse::<f32>()
        .with_context(|| format!("`{name}` initialiser {lit:?} is not a float literal"))?;
    if !(MIN_PLAUSIBLE_YALMS..=MAX_PLAUSIBLE_YALMS).contains(&value) {
        bail!("`{name}` scraped as {value}, outside the plausible yalm band — parse is wrong");
    }
    Ok(value)
}

/// The lobby's S2C 0x05 bitmasks and 0x24 error codes as u16 constants, with
/// `ALL_KNOWN` (every member not named UNUSED_*) on the bit-flag enums for the
/// C2S 0x26 excode the client advertises, and a `name` lookup on each. Opcodes:
/// research/XiPackets/lobby/S2C_0x0005_ResponseKey.md and
/// research/XiPackets/lobby/C2S_0x0026_RequestLobbyLogin.md.
fn write_lobby_tables(path: &std::path::Path) -> Result<()> {
    let mut out = String::from(
        "// AUTO-GENERATED by ffxi-proto/build.rs from login_helpers.h and login_errors.h.\n\
         // Do not edit by hand.\n",
    );
    for (src_path, enum_name, module, bit_flags) in [
        (
            LSB_LOGIN_HELPERS_H,
            "EXPANSION_DISPLAY",
            "expansion_display",
            true,
        ),
        (
            LSB_LOGIN_HELPERS_H,
            "FEATURE_DISPLAY",
            "feature_display",
            true,
        ),
        (LSB_LOGIN_ERRORS_H, "errorCode", "lobby_error", false),
    ] {
        println!("cargo:rerun-if-changed={src_path}");
        let src = fs::read_to_string(src_path).with_context(|| format!("reading {src_path}"))?;
        let members = lsb_scrape::parse_cpp_plain_enum(&src, enum_name)
            .with_context(|| format!("{enum_name} in {src_path}"))?;
        out.push_str(&format!("pub mod {module} {{\n"));
        let mut all_known: u32 = 0;
        for (value, name) in &members {
            u16::try_from(*value)
                .with_context(|| format!("{enum_name}::{name} = {value} exceeds uint16"))?;
            out.push_str(&format!("    pub const {name}: u16 = {value:#06x};\n"));
            if !name.starts_with("UNUSED_") {
                all_known |= value;
            }
        }
        if bit_flags {
            out.push_str(&format!(
                "    pub const ALL_KNOWN: u16 = {all_known:#06x};\n"
            ));
        }
        out.push_str("    pub const NAMES: &[(u16, &str)] = &[\n");
        for (value, name) in &members {
            out.push_str(&format!("        ({value:#06x}, \"{name}\"),\n"));
        }
        out.push_str(
            "    ];\n    pub fn name(value: u16) -> Option<&'static str> {\n        \
             NAMES.iter().find(|(v, _)| *v == value).map(|(_, n)| *n)\n    }\n}\n",
        );
    }
    fs::write(path, out)?;
    Ok(())
}

fn parse_supported_xiloader_version(src: &str) -> Result<[u8; 3]> {
    let start = src
        .find("SupportedXiloaderVersion")
        .context("could not locate `SupportedXiloaderVersion` in auth_session.h")?;
    let open = src[start..]
        .find('{')
        .context("could not locate opening `{` of SupportedXiloaderVersion")?
        + start
        + 1;
    let close = src[open..]
        .find('}')
        .context("could not locate closing `}` of SupportedXiloaderVersion")?
        + open;
    let mut parts = src[open..close].split(',').map(str::trim);
    let mut version = [0u8; 3];
    for slot in &mut version {
        let part = parts
            .next()
            .context("SupportedXiloaderVersion has fewer than 3 components")?;
        *slot = part
            .parse::<u8>()
            .with_context(|| format!("SupportedXiloaderVersion component {part:?} is not a u8"))?;
    }
    if parts.next().is_some() {
        bail!("SupportedXiloaderVersion has more than 3 components");
    }
    Ok(version)
}

/// `src/map.rs` declares each opcode by hand so it can carry the LSB citation and
/// field-layout prose the generated name tables have nowhere to put. The values
/// still have to match upstream, and a hand-kept guard list only covered 40 of
/// the 78 — so read the declarations back out and check every one (kuluu-i9k0).
const MAP_RS: &str = "src/map.rs";

fn check_map_opcodes_against_lsb(
    s2c_names: &[(u32, String)],
    c2s_names: &[(u32, String)],
) -> Result<()> {
    println!("cargo:rerun-if-changed={MAP_RS}");
    let src = fs::read_to_string(MAP_RS).with_context(|| format!("reading {MAP_RS}"))?;

    let mut checked = 0usize;
    for (module, upstream, prefix) in [
        ("s2c", s2c_names, "GP_SERV_COMMAND_"),
        ("c2s", c2s_names, "GP_CLI_COMMAND_"),
    ] {
        let known: std::collections::HashSet<u32> = upstream.iter().map(|(id, _)| *id).collect();
        for (name, id) in parse_module_u16_consts(&src, module)? {
            if !known.contains(&id) {
                bail!(
                    "{MAP_RS} `{module}::{name} = {id:#05X}` is not in the scraped \
                     {prefix}* enum — the opcode drifted from upstream, or the \
                     packet was renumbered"
                );
            }
            checked += 1;
        }
    }
    if checked == 0 {
        bail!("parsed zero opcode consts from {MAP_RS} — its `pub mod s2c`/`c2s` shape changed");
    }
    println!("ffxi-proto: checked {checked} map opcodes against the LSB enums");
    Ok(())
}

/// Every `pub const NAME: u16 = <int>;` directly inside `pub mod <module> {`.
fn parse_module_u16_consts(src: &str, module: &str) -> Result<Vec<(String, u32)>> {
    let header = format!("pub mod {module} {{");
    let start = src
        .find(&header)
        .with_context(|| format!("no `{header}` in {MAP_RS}"))?;

    let mut depth = 0i32;
    let mut out = Vec::new();
    for line in src[start..].lines() {
        if depth == 1 {
            if let Some(rest) = line.trim().strip_prefix("pub const ") {
                if let Some((name, value)) = rest.split_once(": u16 = ") {
                    let value = value.trim().trim_end_matches(';').trim();
                    if let Some(id) = parse_int_lit(value) {
                        out.push((name.trim().to_string(), id as u32));
                    }
                }
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth == 0 {
            break;
        }
    }
    Ok(out)
}

/// Each zone's `FISHING_MESSAGE_OFFSET` text id, keyed by zone id. LSB reads the
/// same value at runtime (`fishingutils::LoadFishingMessages`), then adds a
/// FISHMESSAGEOFFSET to it before putting the result on the wire — so the client
/// needs the base to recover which fishing message a MesNum is.
fn parse_zone_fishing_message_offsets() -> Result<Vec<(u16, u16)>> {
    let zone_src =
        fs::read_to_string(LSB_ZONE_YAML).with_context(|| format!("reading {LSB_ZONE_YAML}"))?;
    // IDs.lua keys `zones[xi.zone.SELBINA]`; the lua enum is the yaml key upper-cased.
    let zone_ids: std::collections::HashMap<String, u16> = parse_yaml_enum_values(&zone_src)
        .with_context(|| format!("parsing {LSB_ZONE_YAML}"))?
        .into_iter()
        .map(|(name, id)| {
            u16::try_from(id)
                .map(|id| (name.to_ascii_uppercase(), id))
                .with_context(|| format!("zone `{name}` id {id} overflows u16"))
        })
        .collect::<Result<_>>()?;

    let mut out = Vec::new();
    let dir = fs::read_dir(LSB_ZONE_SCRIPTS_DIR)
        .with_context(|| format!("reading {LSB_ZONE_SCRIPTS_DIR}"))?;
    for entry in dir.flatten() {
        let ids_lua = entry.path().join("IDs.lua");
        let Ok(src) = fs::read_to_string(&ids_lua) else {
            continue;
        };
        let zone_name = src
            .lines()
            .find_map(|l| l.trim().strip_prefix("zones[xi.zone.")?.split(']').next())
            .map(str::to_string);
        let offset = src.lines().find_map(|l| {
            let rest = l.split_once("FISHING_MESSAGE_OFFSET")?.1;
            rest.split_once('=')?
                .1
                .split(&[',', '-'][..])
                .next()?
                .trim()
                .parse::<u16>()
                .ok()
        });
        if let (Some(name), Some(offset)) = (zone_name, offset) {
            if let Some(&id) = zone_ids.get(name.trim()) {
                out.push((id, offset));
            }
        }
    }
    if out.is_empty() {
        bail!("parsed no FISHING_MESSAGE_OFFSET entries under {LSB_ZONE_SCRIPTS_DIR}");
    }
    Ok(out)
}

/// The FISHMESSAGEOFFSET enum: how far past a zone's base each fishing message
/// sits. The trailing `//` comment is the retail line the message prints, kept
/// alongside — the client uses a few of them as landmark strings to locate the
/// fishing block inside an installed dialog DAT of a different client era.
/// vendor/server/src/map/utils/fishingutils.h
fn parse_fish_message_offset_enum() -> Result<Vec<(String, u8, Option<String>)>> {
    const PREFIX: &str = "FISHMESSAGEOFFSET_";
    let src = fs::read_to_string(LSB_FISHINGUTILS_H)
        .with_context(|| format!("reading {LSB_FISHINGUTILS_H}"))?;
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(PREFIX) else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let (value, comment) = match value.split_once("//") {
            Some((v, c)) => (v, Some(c.trim().to_string())),
            None => (value, None),
        };
        let value = value.trim();
        let value = value
            .split_whitespace()
            .next()
            .unwrap_or(value)
            .trim_end_matches(',');
        let parsed = match value.strip_prefix("0x") {
            Some(hex) => u8::from_str_radix(hex, 16).ok(),
            None => value.parse::<u8>().ok(),
        };
        if let Some(v) = parsed {
            out.push((name.trim().to_string(), v, comment));
        }
    }
    if out.is_empty() {
        bail!("parsed no {PREFIX} entries out of {LSB_FISHINGUTILS_H}");
    }
    Ok(out)
}

fn write_fish_message_consts(
    out_path: &std::path::Path,
    entries: &[(String, u8, Option<String>)],
) -> Result<()> {
    let mut out = String::new();
    out.push_str(&format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_FISHINGUTILS_H}.\n"
    ));
    out.push_str("// Do not edit by hand.\n");
    for (name, value, _) in entries {
        out.push_str(&format!("pub const {name}: u8 = {value};\n"));
    }
    fs::write(out_path, &out)?;
    Ok(())
}

/// The offset set as a sorted table, plus each offset's retail line text where
/// LSB's header records one.
fn write_fish_message_tables(
    out_path: &std::path::Path,
    entries: &[(String, u8, Option<String>)],
) -> Result<()> {
    let mut offsets: Vec<u8> = entries.iter().map(|(_, v, _)| *v).collect();
    offsets.sort_unstable();
    offsets.dedup();
    let mut out = String::new();
    out.push_str(&format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_FISHINGUTILS_H}.\n"
    ));
    out.push_str("// Do not edit by hand.\n");
    out.push_str(&format!("pub const OFFSETS: &[u8] = &{:?};\n", offsets));
    out.push_str("pub const TEXTS: &[(u8, &str)] = &[\n");
    let mut sorted: Vec<&(String, u8, Option<String>)> = entries.iter().collect();
    sorted.sort_by_key(|(_, v, _)| *v);
    for (_, value, comment) in sorted {
        if let Some(text) = comment {
            out.push_str(&format!("    ({value}, {text:?}),\n"));
        }
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

/// `offsetof` for each [`S2C_LAYOUTS`] body, emitted as a module of `usize`
/// consts per packet. Array members also get `_COUNT`/`_STRIDE`/`_LEN`, and
/// bit-fields a `_SHIFT`/`_BITS`/`_MASK` triple over the storage unit at their
/// offset.
fn write_s2c_layouts(out_path: &std::path::Path) -> Result<()> {
    let mut out = format!(
        "// AUTO-GENERATED by ffxi-proto/build.rs from {LSB_S2C_PACKET_DIR}/*.h.\n\
         // Do not edit by hand.\n"
    );
    let mut total_fields = 0usize;
    for (module, header, struct_name) in S2C_LAYOUTS {
        let path = format!("{LSB_S2C_PACKET_DIR}/{header}");
        println!("cargo:rerun-if-changed={path}");
        let src = fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;

        let mut layouts = lsb_scrape::Layouts::new();
        for (cpp_name, yaml_path) in LSB_GENERATED_ENUMS {
            println!("cargo:rerun-if-changed={yaml_path}");
            let yaml_src =
                fs::read_to_string(yaml_path).with_context(|| format!("reading {yaml_path}"))?;
            let underlying = lsb_scrape::parse_yaml(&yaml_src)
                .with_context(|| format!("parsing {yaml_path}"))?
                .get("meta")
                .and_then(|meta| meta.get("cpp"))
                .and_then(|cpp| cpp.get("underlying"))
                .and_then(|node| node.as_str())
                .map(str::to_string)
                .with_context(|| format!("{yaml_path} has no meta.cpp.underlying"))?;
            layouts.register_scalar(cpp_name, &underlying)?;
        }
        layouts
            .scan(&src)
            .with_context(|| format!("scanning {path}"))?;
        let layout = layouts
            .layout(struct_name)
            .with_context(|| format!("laying out {struct_name} from {path}"))?;

        out.push_str(&format!(
            "\n/// `{struct_name}` (LSB {LSB_S2C_PACKET_DIR}/{header}).\npub mod {module} {{\n"
        ));
        out.push_str(&format!(
            "    pub const SIZE: usize = {:#X};\n",
            layout.size
        ));
        let mut emitted = std::collections::HashSet::new();
        for field in &layout.fields {
            let name = screaming_snake_path(&field.path);
            let mut push = |suffix: &str, decl: String| -> Result<()> {
                if !emitted.insert(format!("{name}{suffix}")) {
                    bail!("{struct_name} emits `{name}{suffix}` twice — two members collide");
                }
                out.push_str(&decl);
                Ok(())
            };
            push(
                "",
                format!("    pub const {name}: usize = {:#X};\n", field.offset),
            )?;
            if let Some(count) = field.count {
                push(
                    "_COUNT",
                    format!("    pub const {name}_COUNT: usize = {count};\n"),
                )?;
                push(
                    "_STRIDE",
                    format!("    pub const {name}_STRIDE: usize = {};\n", field.stride),
                )?;
                push(
                    "_LEN",
                    format!("    pub const {name}_LEN: usize = {};\n", field.len()),
                )?;
            }
            if let Some(bits) = field.bits {
                let mask = (u64::MAX >> (u64::BITS - bits.width)) as u32;
                push(
                    "_SHIFT",
                    format!("    pub const {name}_SHIFT: u32 = {};\n", bits.shift),
                )?;
                push(
                    "_BITS",
                    format!("    pub const {name}_BITS: u32 = {};\n", bits.width),
                )?;
                push(
                    "_MASK",
                    format!("    pub const {name}_MASK: u32 = {mask:#X};\n"),
                )?;
            }
            total_fields += 1;
        }
        out.push_str("}\n");
    }
    fs::write(out_path, &out)?;
    check_scrape_count(
        "s2c PacketData members",
        LSB_S2C_PACKET_DIR,
        total_fields,
        floor::S2C_LAYOUT_FIELD,
    )
}

/// A C++ member chain as one SCREAMING_SNAKE const name: `PosHead.GrapIDTbl`
/// becomes `POS_HEAD_GRAP_ID_TBL`.
fn screaming_snake_path(path: &[String]) -> String {
    path.iter()
        .map(|part| screaming_snake(part))
        .collect::<Vec<_>>()
        .join("_")
}

fn screaming_snake(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::with_capacity(ident.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let starts_word = c.is_ascii_uppercase()
            && match prev {
                Some(p) if p.is_ascii_lowercase() => true,
                Some(p) if p.is_ascii_uppercase() => {
                    chars.get(i + 1).is_some_and(char::is_ascii_lowercase)
                }
                _ => false,
            };
        if starts_word && !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}
