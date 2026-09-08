//! `/actordiag` — the field-report answer machine for "my character has no
//! head" (kuluu-39fi). Re-runs the exact look -> file-id -> DAT -> mesh ->
//! texture chain the renderer uses and reports every step into chat, so a
//! user who cannot capture stderr can screenshot the diagnosis instead.

use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::texture::{decode_texture, extract_texture_name};
use ffxi_dat::{walk_tree, ChunkKind, ChunkNode, DatRoot};
use kuluu_snapshot::EntityLook;

use crate::look_resolver::{npc_dat_id, resolve_equipment_model, resolve_face};
use crate::skinned_ffxi_material::SKINNED_ALPHA_DISCARD;

const EQUIP_SLOT_NAMES: [&str; 8] = [
    "head", "body", "hands", "legs", "feet", "main", "sub", "ranged",
];

// The actor path feeds decoded alpha to the shader unremapped
// (`decoded_texture_to_image`), so the discard threshold compares against the
// raw decoded byte.
const ALPHA_DISCARD_RAW: u8 = (SKINNED_ALPHA_DISCARD * 255.0) as u8;

struct FileProbe {
    label: String,
    bytes: Option<Vec<u8>>,
}

fn probe_file(root: &DatRoot, fid: u32) -> FileProbe {
    let loc = match root.resolve(fid) {
        Ok(loc) => loc,
        Err(e) => {
            return FileProbe {
                label: format!("fid {fid}: NOT IN ANY VTABLE ({e})"),
                bytes: None,
            }
        }
    };
    let path = loc.path_under(root);
    let rel = path
        .strip_prefix(root.root())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string());
    match std::fs::read(&path) {
        Ok(bytes) => FileProbe {
            label: format!("fid {fid} -> {rel} ({} bytes)", bytes.len()),
            bytes: Some(bytes),
        },
        Err(e) => FileProbe {
            label: format!("fid {fid} -> {rel} UNREADABLE ({e})"),
            bytes: None,
        },
    }
}

struct TextureProbe {
    name: String,
    detail: String,
    max_alpha: Option<u8>,
}

fn probe_textures(node: &ChunkNode<'_>, out: &mut Vec<TextureProbe>) {
    if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::Img) {
        let name = extract_texture_name(node.chunk.data).unwrap_or_else(|| "<unnamed>".into());
        match decode_texture(node.chunk.data) {
            Ok(t) => {
                let max_alpha = t.rgba.chunks_exact(4).map(|px| px[3]).max().unwrap_or(0);
                out.push(TextureProbe {
                    name,
                    detail: format!(
                        "{}x{} {:?} max_alpha={max_alpha}",
                        t.width, t.height, t.format_tag
                    ),
                    max_alpha: Some(max_alpha),
                });
            }
            Err(e) => out.push(TextureProbe {
                name,
                detail: format!("DECODE FAILED ({e})"),
                max_alpha: None,
            }),
        }
    }
    for child in &node.children {
        probe_textures(child, out);
    }
}

fn face_lines(root: &DatRoot, face: u8, race: u8, lines: &mut Vec<String>) {
    let Some(fid) = resolve_face(face, race) else {
        lines.push(format!(
            "  face: UNRESOLVED (race {race} is not a PC race) -> no head/hair"
        ));
        return;
    };
    let probe = probe_file(root, fid);
    let Some(bytes) = probe.bytes else {
        lines.push(format!("  face: {} -> DECAPITATED", probe.label));
        return;
    };
    let mesh_count = ResourceDir::from_bytes(bytes.clone())
        .collect_skel_meshes()
        .len();
    lines.push(format!("  face: {} meshes={mesh_count}", probe.label));
    if mesh_count == 0 {
        lines.push("  face: 0 meshes -> DECAPITATED".into());
        return;
    }
    let mut textures = Vec::new();
    probe_textures(&walk_tree(&bytes), &mut textures);
    for t in &textures {
        lines.push(format!("    tex \"{}\" {}", t.name, t.detail));
    }
    if textures.is_empty() {
        lines.push("  face: no textures (renders untextured, but visible)".into());
    } else if textures
        .iter()
        .all(|t| t.max_alpha.is_none_or(|a| a < ALPHA_DISCARD_RAW))
    {
        lines.push(format!(
            "  face: every texture is below the alpha test ({ALPHA_DISCARD_RAW}/255) -> INVISIBLE head"
        ));
    } else {
        lines.push("  face: data OK on this install".into());
    }
}

pub fn report(entity_id: u32, name: &str, look: &EntityLook) -> Vec<String> {
    let mut lines = vec![format!("/actordiag: {name} (id {entity_id})")];
    let root = match DatRoot::from_env_or_default() {
        Ok(root) => root,
        Err(e) => {
            lines.push(format!("  no DAT install: {e}"));
            return lines;
        }
    };
    match look {
        EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        } => {
            lines.push(format!("  look: race={race} face={face}"));
            face_lines(&root, *face, *race, &mut lines);
            let slot_models = [*head, *body, *hands, *legs, *feet, *main, *sub, *ranged];
            for (i, &model_id) in slot_models.iter().enumerate() {
                let slot_index = (i + 1) as u8;
                let status = match resolve_equipment_model(slot_index, model_id, *race) {
                    None => "unresolved (not drawn)".to_string(),
                    Some(fid) => {
                        let probe = probe_file(&root, fid);
                        match probe.bytes {
                            None => probe.label,
                            Some(bytes) => {
                                let meshes =
                                    ResourceDir::from_bytes(bytes).collect_skel_meshes().len();
                                format!("{} meshes={meshes}", probe.label)
                            }
                        }
                    }
                };
                lines.push(format!("  {}={model_id}: {status}", EQUIP_SLOT_NAMES[i]));
            }
        }
        EntityLook::Standard { modelid } => {
            let fid = npc_dat_id(*modelid);
            let probe = probe_file(&root, fid);
            let status = match probe.bytes {
                None => probe.label,
                Some(bytes) => {
                    let meshes = ResourceDir::from_bytes(bytes).collect_skel_meshes().len();
                    format!("{} meshes={meshes}", probe.label)
                }
            };
            lines.push(format!("  npc model {modelid}: {status}"));
        }
        EntityLook::Door { .. } | EntityLook::Transport { .. } => {
            lines.push("  door/transport look: no PC model chain to diagnose".into());
        }
    }
    let overlays = root.overlays();
    lines.push(format!(
        "  root: {} (overlays: {})",
        root.root().display(),
        overlays.len()
    ));
    for o in &overlays {
        lines.push(format!("    overlay: {}", o.display()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    // The renderer discards at SKINNED_ALPHA_DISCARD without remapping actor
    // alpha; the diag verdict must sit on the same raw threshold or it will
    // clear textures the shader actually discards.
    #[test]
    fn alpha_verdict_threshold_matches_the_shader_discard() {
        assert_eq!(ALPHA_DISCARD_RAW, 69);
    }

    #[test]
    fn equipped_report_covers_face_and_every_slot() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no DAT install");
            return;
        }
        let look = EntityLook::Equipped {
            face: 0,
            race: 6,
            head: 0,
            body: 0,
            hands: 0,
            legs: 0,
            feet: 0,
            main: 0,
            sub: 0,
            ranged: 0,
        };
        let lines = report(1, "TestTaru", &look);
        let joined = lines.join("\n");
        assert!(joined.contains("face: fid"), "face line missing:\n{joined}");
        for slot in EQUIP_SLOT_NAMES {
            assert!(
                joined.contains(&format!("{slot}=")),
                "{slot} missing:\n{joined}"
            );
        }
        assert!(
            joined.contains("data OK") || joined.contains("DECAPITATED"),
            "no face verdict:\n{joined}"
        );
    }
}
