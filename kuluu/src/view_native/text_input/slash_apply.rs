use super::*;

fn minimap_retail_desc(state: &kuluu_render::minimap::MinimapState, zone: Option<u16>) -> String {
    use kuluu_render::minimap::RetailStatus;
    let Some(z) = zone else {
        return "no active zone".into();
    };
    if state.retail_zone == Some(z) {
        match &state.retail_status {
            RetailStatus::Loaded => return "loaded".into(),
            RetailStatus::Failed(why) => return format!("unavailable — {why}"),
            RetailStatus::Idle => {}
        }
    }
    // The file id is not named here: the loader picks it out of the FFXiMain
    // zone-map record, and quoting the POLUtils table's id instead would report
    // a different map than the one being loaded (kuluu-bqm5).
    format!(
        "pending (zone {z}; img={} rzone={:?} failed={})",
        state.retail_image.is_some(),
        state.retail_zone,
        state.retail_failed_zones.contains(&z),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_slash_outcome(
    outcome: SlashOutcome,
    target: &mut Target,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    exit: &mut MessageWriter<AppExit>,
    navmesh_visible: &mut crate::view_native::navmesh_overlay::NavmeshOverlayVisible,
    navmesh_state: &crate::view_native::navmesh_overlay::NavmeshState,
    self_pos: kuluu_snapshot::Vec3,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    #[cfg(unix)] agent_paused: Option<&crate::view_native::AgentPaused>,
    _session_event_tx: Option<&crate::view_native::SessionEventTx>,
    slash_writers: &mut SlashWriters,
    draw_distance: &mut kuluu_render::dat_mzb::DrawDistance,
) {
    match outcome {
        SlashOutcome::Command(cmd) => {
            if let Some(toast) = reqlogout_ack_text(&cmd) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                slash_writers
                    .logout_requested
                    .write(kuluu_render::hud::logout_countdown::LogoutRequested { shutdown });
            }
            mirror_heal_stance(&cmd, &mut slash_writers.rest_stance);
            let send_result = cmd_tx.try_send(cmd);
            if let Err(e) = send_result {
                push_system_chat_line(scene_state, format!("command dropped (channel issue): {e}"));
            }
        }
        SlashOutcome::CommandWithNotice { cmd, notice } => {
            push_system_chat_line(scene_state, notice);
            if let Err(e) = cmd_tx.try_send(cmd) {
                push_system_chat_line(scene_state, format!("command dropped (channel issue): {e}"));
            }
        }
        SlashOutcome::Commands(cmds) => {
            for cmd in cmds {
                if let Some(toast) = reqlogout_ack_text(&cmd) {
                    push_system_chat_line(scene_state, toast.into());
                }
                mirror_heal_stance(&cmd, &mut slash_writers.rest_stance);
                if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                    slash_writers
                        .logout_requested
                        .write(kuluu_render::hud::logout_countdown::LogoutRequested { shutdown });
                }
                if let Err(e) = cmd_tx.try_send(cmd) {
                    push_system_chat_line(
                        scene_state,
                        format!("command dropped (channel issue): {e}"),
                    );
                }
            }
        }
        SlashOutcome::SetTarget(id) => {
            target.id = id;
        }
        SlashOutcome::Quit => {
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
            crate::view_native::exit_watchdog::arm();
        }
        SlashOutcome::QuitWithLogout(kind) => {
            let req = AgentCommand::ReqLogout { kind };
            if let Some(toast) = reqlogout_ack_text(&req) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Some(shutdown) = reqlogout_starts_countdown(&req) {
                slash_writers
                    .logout_requested
                    .write(kuluu_render::hud::logout_countdown::LogoutRequested { shutdown });
            }
            let _ = cmd_tx.try_send(req);
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
            crate::view_native::exit_watchdog::arm();
        }
        SlashOutcome::SystemMessage(text) => {
            for line in text.split('\n') {
                push_system_chat_line(scene_state, line.to_string());
            }
        }
        SlashOutcome::SetWeatherClient(w) => {
            scene_state.snapshot.weather = Some(w);
            push_system_chat_line(scene_state, format!("weather override: {w:?}"));
        }
        SlashOutcome::SetSitStance(toggle) => {
            use crate::view_native::slash_commands::SitToggle;
            use kuluu_render::combat_stance::RestKind;
            let next = match toggle {
                SitToggle::On => RestKind::Sit,
                SitToggle::Off => RestKind::None,
                SitToggle::Toggle => match slash_writers.rest_stance.kind {
                    RestKind::Sit => RestKind::None,

                    RestKind::Heal => {
                        let _ = cmd_tx.try_send(AgentCommand::Heal {
                            mode: kuluu_session::state::HealMode::Off,
                        });
                        RestKind::Sit
                    }
                    RestKind::None => RestKind::Sit,
                },
            };
            slash_writers.rest_stance.kind = next;
            let label = match next {
                RestKind::Sit => "sitting",
                RestKind::Heal => "healing",
                RestKind::None => "standing",
            };
            push_system_chat_line(scene_state, format!("/sit: {label}"));
        }
        SlashOutcome::ToggleNavmesh(setting) => {
            let next = setting.unwrap_or(!navmesh_visible.0);
            navmesh_visible.0 = next;
            let label = if next { "ON" } else { "OFF" };
            push_system_chat_line(scene_state, format!("navmesh overlay: {label}"));
        }
        SlashOutcome::LoadMmb {
            file_id,
            chunk_idx,
            world_pos,
            entity_id,
        } => {
            let bevy_pos = kuluu_render::ffxi_to_bevy(world_pos);
            slash_writers.load_mmb.write(LoadMmbRequest {
                file_id,
                chunk_idx,
                world_pos: bevy_pos,
                entity_id,
                world_transform: None,
                water: None,
                lod: None,
                door: None,
                slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
                sub_area_link: 0,
            });
            let label = match entity_id {
                Some(id) => format!("/load_mmb_on {id} {file_id} {chunk_idx}: spawning…"),
                None => format!("/load_mmb {file_id} {chunk_idx}: spawning…"),
            };
            push_system_chat_line(scene_state, label);
        }
        SlashOutcome::DebugHeights => {
            slash_writers.debug_heights.write(DebugHeightsRequest);
        }
        SlashOutcome::Screenshot { path } => {
            let resolved = path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(crate::view_native::screenshot::next_default_path);
            slash_writers
                .screenshot
                .write(crate::view_native::screenshot::ScreenshotRequest { path: resolved });
        }
        SlashOutcome::PlayBgm { track_id } => {
            slash_writers
                .event_log
                .recent
                .push_back(kuluu_snapshot::ViewerEvent::MusicChanged { slot: 0, track_id });
            push_system_chat_line(scene_state, format!("/bgm {track_id}: queued"));
        }
        SlashOutcome::PlaySfx { se_id } => {
            slash_writers
                .sfx_event
                .write(kuluu_render::audio::SfxEvent::new(se_id));
            push_system_chat_line(scene_state, format!("/sfx {se_id}: fired"));
        }
        SlashOutcome::EndCutscene { event_num } => {
            let resolved_csid = event_num
                .or_else(|| scene_state.snapshot.dialog.as_ref().map(|d| d.event_para))
                .or_else(|| {
                    scene_state
                        .snapshot
                        .zone_id
                        .and_then(crate::view_native::slash_commands::start_zone_cutscene)
                });
            let Some(csid) = resolved_csid else {
                push_system_chat_line(
                    scene_state,
                    "/endcutscene: no active event and current zone isn't a \
                     starting nation; pass an explicit CSID \
                     (`/endcutscene <csid>`) or use `/release`"
                        .into(),
                );
                return;
            };

            let self_char_id = scene_state.snapshot.self_char_id;
            let self_act_index = self_char_id.and_then(|id| {
                scene_state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.act_index)
            });
            match (self_char_id, self_act_index) {
                (Some(event_id), Some(act_index)) => {
                    push_system_chat_line(
                        scene_state,
                        format!(
                            "/endcutscene: sending EVENT_END (csid={csid}, \
                             unique_no=0x{event_id:08X}, act_index={act_index})"
                        ),
                    );
                    if let Err(e) = cmd_tx.try_send(AgentCommand::EndEventChoice {
                        event_id,
                        act_index,
                        event_num: csid,
                        choice: 0,
                    }) {
                        push_system_chat_line(
                            scene_state,
                            format!("/endcutscene: dropped (channel issue): {e}"),
                        );
                    }
                }
                _ => {
                    push_system_chat_line(
                        scene_state,
                        "/endcutscene: self entity not in snapshot yet — wait for \
                         zone-in to complete and retry"
                            .into(),
                    );
                }
            }
        }
        SlashOutcome::SetTargetFps(target) => {
            use bevy_framepace::Limiter;
            match target {
                Some(n) => {
                    slash_writers.framepace.limiter = Limiter::from_framerate(n as f64);
                    push_system_chat_line(scene_state, format!("/fps: capped at {n}"));
                }
                None => {
                    slash_writers.framepace.limiter = Limiter::Off;
                    push_system_chat_line(scene_state, "/fps: cap disabled".into());
                }
            }
        }
        SlashOutcome::SetCaptureMode(arg) => {
            use bevy_framepace::Limiter;
            let want_on = arg.unwrap_or(!slash_writers.capture_mode.active);
            if want_on == slash_writers.capture_mode.active {
                let label = if want_on { "on" } else { "off" };
                push_system_chat_line(
                    scene_state,
                    format!("/capture: already {label} (no change)"),
                );
            } else if want_on {
                slash_writers.capture_mode.restore_limiter =
                    Some(slash_writers.framepace.limiter.clone());
                slash_writers.framepace.limiter = Limiter::Off;
                if let Ok(mut window) = slash_writers.primary_window.single_mut() {
                    window.present_mode = PresentMode::Fifo;
                }
                slash_writers.capture_mode.active = true;
                push_system_chat_line(
                    scene_state,
                    "/capture: on (framepace off, present_mode=Fifo) — \
                     prefer OBS/Cmd+Shift+5 over QuickTime if recording still stalls"
                        .into(),
                );
            } else {
                let restored = slash_writers
                    .capture_mode
                    .restore_limiter
                    .take()
                    .unwrap_or(Limiter::Auto);
                slash_writers.framepace.limiter = restored;
                if let Ok(mut window) = slash_writers.primary_window.single_mut() {
                    window.present_mode = PresentMode::AutoVsync;
                }
                slash_writers.capture_mode.active = false;
                push_system_chat_line(scene_state, "/capture: off (settings restored)".into());
            }
        }
        SlashOutcome::SetZoneGeom(setting) => {
            let next = setting.unwrap_or_else(|| draw_distance.zone_geom_mode.cycle());
            draw_distance.zone_geom_mode = next;
            push_system_chat_line(scene_state, format!("/zonegeom: {}", next.label()));
        }
        SlashOutcome::SetCameraCollisionSource(setting) => {
            let next = setting.unwrap_or_else(|| draw_distance.camera_collision_source.cycle());
            draw_distance.camera_collision_source = next;
            push_system_chat_line(scene_state, format!("/zonegeom source: {}", next.label()));
        }
        SlashOutcome::SetDevHud(setting) => {
            let next = setting.unwrap_or(!slash_writers.hud_verbosity.dev_hud);
            slash_writers.hud_verbosity.dev_hud = next;
            push_system_chat_line(
                scene_state,
                format!("/devhud: {}", if next { "on" } else { "off" }),
            );
        }
        SlashOutcome::SetNetStatus(setting) => {
            let next = setting.unwrap_or(!slash_writers.net_status_visible.0);
            slash_writers.net_status_visible.0 = next;
            push_system_chat_line(
                scene_state,
                format!("/netstat: {}", if next { "on" } else { "off" }),
            );
        }
        SlashOutcome::SetNoClip(setting) => {
            let next = setting.unwrap_or(!slash_writers.hud_panels.noclip);
            slash_writers.hud_panels.noclip = next;
            push_system_chat_line(
                scene_state,
                format!(
                    "/noclip: {} (wall collision {})",
                    if next { "on" } else { "off" },
                    if next { "bypassed" } else { "active" }
                ),
            );
        }
        SlashOutcome::SetVanaClock(setting) => {
            let next = setting.unwrap_or(!slash_writers.vana_clock_visible.0);
            slash_writers.vana_clock_visible.0 = next;
            push_system_chat_line(
                scene_state,
                format!("/clock: {}", if next { "shown" } else { "hidden" }),
            );
        }
        SlashOutcome::SetRenderScale(setting) => {
            let g = &mut *slash_writers.graphics;
            if let Some(v) = setting {
                g.render_scale = v.clamp(0.25, 2.0);
                g.preset = kuluu_render::QualityPreset::Custom;
            }
            push_system_chat_line(
                scene_state,
                format!(
                    "/renderscale: {:.0}%{}",
                    g.render_scale() * 100.0,
                    if g.wants_render_scale() {
                        ""
                    } else {
                        " (native)"
                    }
                ),
            );
        }
        SlashOutcome::SetZoneLines(op) => {
            use crate::view_native::slash_commands::ZoneLineOp;
            use kuluu_render::ZoneLineDisplay;

            let g = &mut *slash_writers.graphics;
            let next = match op {
                ZoneLineOp::Status => g.zone_line_display,
                ZoneLineOp::Set(mode) => mode,
                ZoneLineOp::Toggle => match g.zone_line_display {
                    ZoneLineDisplay::Off => ZoneLineDisplay::Pillar,
                    ZoneLineDisplay::Pillar => ZoneLineDisplay::Gate,
                    ZoneLineDisplay::Gate => ZoneLineDisplay::Off,
                },
            };
            g.zone_line_display = next;
            push_system_chat_line(scene_state, format!("/zoneline: {}", next.label()));
        }
        SlashOutcome::SetLights(op) => {
            use crate::view_native::slash_commands::LightsOp;
            use kuluu_render::graphics_settings::DynamicLights;

            let g = &mut *slash_writers.graphics;
            let chat = match op {
                LightsOp::Status => format!(
                    "/lights: {} · threshold {:.2} · intensity {:.0} · range {:.1} · flicker {}",
                    g.dynamic_lights.label(),
                    g.light_threshold,
                    g.light_intensity,
                    g.light_range,
                    if g.light_flicker { "on" } else { "off" },
                ),
                LightsOp::Enable(v) => {
                    let on = v.unwrap_or(!g.dynamic_lights.emitters_enabled());
                    g.dynamic_lights = if on {
                        DynamicLights::Enhanced
                    } else {
                        DynamicLights::Off
                    };
                    format!("/lights: {}", g.dynamic_lights.label())
                }
                LightsOp::Threshold(v) => {
                    g.light_threshold = v;
                    format!("/lights threshold: {v:.2} (re-enter zone to re-detect)")
                }
                LightsOp::Intensity(v) => {
                    g.light_intensity = v;
                    format!("/lights intensity: {v:.0}")
                }
                LightsOp::Range(v) => {
                    g.light_range = v;
                    format!("/lights range: {v:.1}")
                }
                LightsOp::Flicker(v) => {
                    let f = v.unwrap_or(!g.light_flicker);
                    g.light_flicker = f;
                    format!("/lights flicker: {}", if f { "on" } else { "off" })
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetMinimap(op) => {
            use crate::view_native::slash_commands::MinimapOp;
            use kuluu_render::minimap::MinimapMode;
            let chat = match op {
                MinimapOp::Status => {
                    let zone = scene_state.snapshot.zone_id;
                    let resolved = slash_writers
                        .minimap_state
                        .resolved_mode(*slash_writers.minimap_mode);
                    let top_down = if slash_writers.minimap_state.aabb.is_some() {
                        "baked"
                    } else {
                        "not baked"
                    };
                    format!(
                        "/minimap: mode={:?}→{:?} visible={} cull={:.1} zone={} | retail: {} | top-down: {}",
                        *slash_writers.minimap_mode,
                        resolved,
                        slash_writers.minimap_visible.0,
                        slash_writers.topdown_cull.top_cull_yalms,
                        zone.map(|z| z.to_string()).unwrap_or_else(|| "—".into()),
                        minimap_retail_desc(&slash_writers.minimap_state, zone),
                        top_down,
                    )
                }
                MinimapOp::Show => {
                    slash_writers.minimap_visible.0 = true;
                    "/minimap: shown".into()
                }
                MinimapOp::Hide => {
                    slash_writers.minimap_visible.0 = false;
                    "/minimap: hidden".into()
                }
                MinimapOp::Toggle => {
                    let next = !slash_writers.minimap_visible.0;
                    slash_writers.minimap_visible.0 = next;
                    format!("/minimap: {}", if next { "shown" } else { "hidden" })
                }
                MinimapOp::ModeTopDown => {
                    *slash_writers.minimap_mode = MinimapMode::TopDown;
                    "/minimap: mode=top-down".into()
                }
                MinimapOp::ModeRetail => {
                    *slash_writers.minimap_mode = MinimapMode::Retail;
                    let zone = scene_state.snapshot.zone_id;
                    format!(
                        "/minimap: mode=retail | {}",
                        minimap_retail_desc(&slash_writers.minimap_state, zone)
                    )
                }
                MinimapOp::ModeAuto => {
                    *slash_writers.minimap_mode = MinimapMode::Auto;
                    "/minimap: mode=auto".into()
                }
                MinimapOp::SetCull(v) => {
                    slash_writers.topdown_cull.top_cull_yalms = v;
                    format!("/minimap: cull={v:.1} yalms (re-baking next frame)")
                }
                MinimapOp::ZoomIn => {
                    let half = kuluu_render::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(1.0 / kuluu_render::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomOut => {
                    let half = kuluu_render::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(kuluu_render::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomFit => {
                    slash_writers.minimap_zoom.radius_yalms = None;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: fit-to-zone".into()
                }
                MinimapOp::ZoomSet(r) => {
                    let clamped = r.max(kuluu_render::minimap::ZOOM_MIN_RADIUS);
                    slash_writers.minimap_zoom.radius_yalms = Some(clamped);
                    slash_writers.minimap_view.idle_frames = 0;
                    format!("/minimap zoom: radius={clamped:.0} yalms")
                }
                MinimapOp::ZoomReset => {
                    *slash_writers.minimap_zoom = kuluu_render::minimap::MinimapZoom::default();
                    slash_writers.minimap_view.pan_offset_xz = bevy::math::Vec2::ZERO;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: reset to defaults".into()
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::ActorDiag { use_target } => {
            let id = if use_target {
                target.id
            } else {
                scene_state.snapshot.self_char_id
            };
            let lines = match id {
                None if use_target => vec!["/actordiag: no target selected".to_string()],
                None => vec!["/actordiag: self id unknown (not in game yet?)".to_string()],
                Some(id) => match scene_state.snapshot.entities.iter().find(|e| e.id == id) {
                    None => vec![format!("/actordiag: entity {id} not in the snapshot")],
                    Some(e) => match &e.look {
                        None => vec![format!("/actordiag: entity {id} carries no look data")],
                        Some(look) => kuluu_render::actor_diag::report(
                            id,
                            e.name.as_deref().unwrap_or("?"),
                            look,
                        ),
                    },
                },
            };
            for line in lines {
                push_system_chat_line(scene_state, line);
            }
        }
        SlashOutcome::Overlay(op) => {
            use crate::view_native::slash_commands::OverlayOp;
            let chat = match slash_writers.dat_root.0.as_ref() {
                None => "/overlay: no DAT install loaded".to_string(),
                Some(root) => {
                    let active = root.overlays();
                    let store = slash_writers.overlay_store.as_ref().map(|r| &r.store);
                    // `None` from the store means no override file, so the
                    // active list is whatever discovery found.
                    let overridden = store.and_then(|s| s.load().ok().flatten()).is_some();
                    let mut next: Option<Vec<std::path::PathBuf>> = None;
                    let mut reset = false;
                    let mut msg = match &op {
                        OverlayOp::List => {
                            let source = if overridden { "override" } else { "discovered" };
                            if active.is_empty() {
                                format!("/overlay: none active ({source})")
                            } else {
                                let list = active
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| format!("  {}. {}", i + 1, p.display()))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                format!("/overlay: {} active ({source})\n{list}", active.len())
                            }
                        }
                        OverlayOp::Add(dir) => {
                            if !dir.is_dir() {
                                format!("/overlay add: not a directory: {}", dir.display())
                            } else {
                                let mut v = active.clone();
                                v.push(dir.clone());
                                let n = v.len();
                                next = Some(v);
                                format!("/overlay: added {} ({n} active)", dir.display())
                            }
                        }
                        OverlayOp::Remove(n) => match active.get(n - 1) {
                            None => {
                                format!("/overlay remove: no entry {n} (have {})", active.len())
                            }
                            Some(gone) => {
                                let gone = gone.display().to_string();
                                let mut v = active.clone();
                                v.remove(n - 1);
                                next = Some(v);
                                format!("/overlay: removed {gone}")
                            }
                        },
                        OverlayOp::Clear => {
                            next = Some(Vec::new());
                            "/overlay: cleared — the install's own DATs only".to_string()
                        }
                        OverlayOp::Reset => {
                            reset = true;
                            let found = ffxi_dat::archive::discover_overlays(root.root());
                            let n = found.len();
                            root.set_overlays(found);
                            format!("/overlay: back to discovery ({n} found)")
                        }
                    };

                    if let Some(v) = next {
                        root.set_overlays(v.clone());
                        match store {
                            Some(s) => {
                                if let Err(e) = s.save(&v) {
                                    msg.push_str(&format!("\n  (not saved: {e})"));
                                }
                            }
                            None => msg.push_str("\n  (not saved: no config dir)"),
                        }
                    }
                    if reset {
                        match store {
                            Some(s) => {
                                if let Err(e) = s.clear() {
                                    msg.push_str(&format!("\n  (override not removed: {e})"));
                                }
                            }
                            None => msg.push_str("\n  (no override file to remove)"),
                        }
                    }
                    if !matches!(op, OverlayOp::List) {
                        // Only later reads go through the new path; anything
                        // already decoded into an asset keeps the old bytes.
                        msg.push_str(
                            "\n  (already-loaded DAT assets keep the old bytes until reload)",
                        );
                    }
                    msg
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetSound(op) => {
            use crate::view_native::slash_commands::SoundOp;
            let mute = &mut *slash_writers.audio_mute;

            let apply = |cur: &mut bool, target: Option<bool>| {
                *cur = target.unwrap_or(!*cur);
            };
            let chat = match op {
                SoundOp::Status => format!(
                    "/sound: bgm={} sfx={}",
                    if mute.bgm { "off" } else { "on" },
                    if mute.sfx { "off" } else { "on" },
                ),
                SoundOp::SetBoth(target) => {
                    apply(&mut mute.bgm, target);
                    apply(&mut mute.sfx, target);
                    format!(
                        "/sound: bgm={} sfx={}",
                        if mute.bgm { "off" } else { "on" },
                        if mute.sfx { "off" } else { "on" },
                    )
                }
                SoundOp::SetBgm(target) => {
                    apply(&mut mute.bgm, target);
                    format!("/sound bgm: {}", if mute.bgm { "off" } else { "on" })
                }
                SoundOp::SetSfx(target) => {
                    apply(&mut mute.sfx, target);
                    format!("/sound sfx: {}", if mute.sfx { "off" } else { "on" })
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetDrawDistance(op) => {
            use crate::view_native::slash_commands::DrawDistanceOp;
            match op {
                DrawDistanceOp::Show => {
                    push_system_chat_line(
                        scene_state,
                        format!(
                            "/drawdistance world={:.0} mob={:.0} (yalms)",
                            draw_distance.world, draw_distance.mob
                        ),
                    );
                }
                DrawDistanceOp::SetWorld(v) => {
                    draw_distance.world = v;
                    push_system_chat_line(
                        scene_state,
                        format!("/drawdistance: setworld {v:.0} yalms"),
                    );
                }
                DrawDistanceOp::SetMob(v) => {
                    draw_distance.mob = v;
                    push_system_chat_line(
                        scene_state,
                        format!("/drawdistance: setmob {v:.0} yalms"),
                    );
                }
            }
        }
        SlashOutcome::LoadMzb {
            file_id,
            chunk_idx,
            world_pos,
        } => {
            let bevy_pos = kuluu_render::ffxi_to_bevy(world_pos);
            slash_writers.load_mzb.write(LoadMzbRequest {
                file_id,
                chunk_idx,
                world_pos: bevy_pos,

                auto_loaded: false,
                slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
                active_sub_area: None,
            });
            let idx_desc = match chunk_idx {
                Some(i) => format!("chunk {i}"),
                None => "first MZB chunk".to_string(),
            };
            push_system_chat_line(
                scene_state,
                format!("/load_mzb {file_id} ({idx_desc}): spawning…"),
            );
        }
        SlashOutcome::SubArea { op, self_pos } => {
            apply_sub_area(op, self_pos, scene_state, &mut slash_writers.set_sub_area);
        }
        SlashOutcome::ShopBuyRow { shop_index, qty } => match scene_state.snapshot.shop.as_ref() {
            Some(shop) => {
                let _ = cmd_tx.try_send(AgentCommand::ShopBuy {
                    shop_no: shop.offset_index,
                    shop_index,
                    qty,
                });
            }
            None => push_system_chat_line(scene_state, "/buy: no shop is open".into()),
        },
        SlashOutcome::ShopSellSlot { inv_slot, qty } => match scene_state.snapshot.shop.as_ref() {
            Some(_) => {
                let item_no = scene_state
                    .snapshot
                    .containers
                    .iter()
                    .find(|c| c.id == ffxi_proto::map::container::LOC_INVENTORY)
                    .and_then(|c| c.items.iter().find(|s| s.index == inv_slot))
                    .map(|s| s.item_no);
                match item_no {
                    Some(item_no) => {
                        let _ = cmd_tx.try_send(AgentCommand::ShopSellReq {
                            qty,
                            item_no,
                            item_index: inv_slot,
                        });
                    }
                    None => push_system_chat_line(
                        scene_state,
                        format!("/sell: nothing in inventory slot {inv_slot}"),
                    ),
                }
            }
            None => push_system_chat_line(scene_state, "/sell: no shop is open".into()),
        },
        SlashOutcome::ShopSellConfirm => match scene_state.snapshot.shop.as_ref() {
            Some(_) => {
                let _ = cmd_tx.try_send(AgentCommand::ShopSellConfirm);
            }
            None => push_system_chat_line(scene_state, "/sell: no shop is open".into()),
        },
        SlashOutcome::ApplyKeybinds(update) => {
            apply_keybind_update(update, bindings, keybinds_state, scene_state);
        }
        SlashOutcome::NavInfo => {
            report_nav_info(navmesh_state, self_pos, scene_state);
        }
        SlashOutcome::AgentControl(op) => {
            #[cfg(unix)]
            apply_agent_control(op, agent_paused, session_event_tx, scene_state);
            #[cfg(not(unix))]
            {
                let _ = op;
                push_system_chat_line(
                    scene_state,
                    "/agent: requires Unix-domain-socket build (non-Unix target)".into(),
                );
            }
        }
        SlashOutcome::CopyToasts { n } => {
            apply_copy_toasts(n, scene_state);
        }
        #[cfg(debug_assertions)]
        SlashOutcome::Widescan => {
            let _ = cmd_tx.try_send(AgentCommand::WidescanRequest);
            let rows = kuluu_render::hud::map_screen::widescan_rows(&scene_state.snapshot);
            if rows.is_empty() {
                push_system_chat_line(
                    scene_state,
                    "[widescan] no targets (requesting refresh…)".into(),
                );
            } else {
                push_system_chat_line(scene_state, format!("[widescan] {} target(s):", rows.len()));
                for row in rows {
                    push_system_chat_line(scene_state, format!("  {}", row.label));
                }
            }
        }
        SlashOutcome::OpenMenu(kind) => {
            let label: std::borrow::Cow<'static, str> = match kind {
                kuluu_render::MenuKind::Magic => "Magic".into(),
                kuluu_render::MenuKind::Abilities => "Abilities".into(),
                kuluu_render::MenuKind::Items => "Items".into(),
                kuluu_render::MenuKind::KeyItems => "Key Items".into(),
                kuluu_render::MenuKind::UsableItems => "Items (usable)".into(),
                kuluu_render::MenuKind::Equipment => "Equipment".into(),
                kuluu_render::MenuKind::Root => "Root".into(),
                kuluu_render::MenuKind::Config => "Config".into(),
                kuluu_render::MenuKind::Debug => "Debug".into(),
                kuluu_render::MenuKind::Graphics => "Graphics".into(),
                kuluu_render::MenuKind::GraphicsDlss => "DLSS Config".into(),
                kuluu_render::MenuKind::Status => "Status".into(),

                kuluu_render::MenuKind::Communication => "Communication".into(),
                kuluu_render::MenuKind::EmoteList => "Emote List".into(),

                kuluu_render::MenuKind::ItemAction { item_no, .. } => {
                    format!("ItemAction({item_no})").into()
                }
                kuluu_render::MenuKind::EquipSlot(slot) => format!("EquipSlot({slot})").into(),
                kuluu_render::MenuKind::Map => "Map".into(),
            };
            push_system_chat_line(scene_state, format!("[menu] opened {label}"));
        }
    }
}

fn apply_sub_area(
    op: SubAreaOp,
    self_pos: kuluu_snapshot::Vec3,
    scene_state: &mut SceneState,
    set_sub_area: &mut MessageWriter<kuluu_render::sub_area_activation::SetSubArea>,
) {
    let Some(zone_file_id) = kuluu_render::snapshot::effective_zone_file_id(&scene_state.snapshot)
    else {
        push_system_chat_line(
            scene_state,
            "/subarea: no zone DAT for the current zone".into(),
        );
        return;
    };
    let subs = match kuluu_render::dat_mzb::zone_sub_areas(zone_file_id) {
        Ok(s) => s,
        Err(e) => {
            push_system_chat_line(scene_state, format!("/subarea: {e}"));
            return;
        }
    };
    if subs.is_empty() {
        push_system_chat_line(
            scene_state,
            format!("/subarea: zone DAT {zone_file_id} declares no sub-areas"),
        );
        return;
    }

    // Snapshot order is (x, ground depth, height); the RID rects keep the DAT's
    // (x, height, ground depth). Same remap as `reactor::is_inside_dat_obb`.
    let here = [self_pos.x, self_pos.z, self_pos.y];
    let wanted = match op {
        SubAreaOp::List => {
            let mut lines = vec![format!(
                "/subarea: zone DAT {zone_file_id} declares {} interior(s)",
                subs.len()
            )];
            for s in &subs {
                lines.push(format!(
                    "  {:#x} -> DAT {}{} · {} trigger(s){}",
                    s.sub_area.id,
                    s.sub_area.file_id,
                    if s.resolves { "" } else { " (MISSING)" },
                    s.sub_area.triggers.len(),
                    if s.sub_area.contains(here) {
                        " · you are inside"
                    } else {
                        ""
                    },
                ));
            }
            for line in lines {
                push_system_chat_line(scene_state, line);
            }
            return;
        }
        SubAreaOp::Here => match subs.iter().find(|s| s.sub_area.contains(here)) {
            Some(s) => s.sub_area.id,
            None => {
                push_system_chat_line(
                    scene_state,
                    "/subarea here: no sub-area trigger holds you (you are outdoors)".into(),
                );
                return;
            }
        },
        SubAreaOp::Load(id) => id,
    };

    let Some(s) = subs.iter().find(|s| s.sub_area.id == wanted) else {
        push_system_chat_line(
            scene_state,
            format!("/subarea: zone DAT {zone_file_id} declares no sub-area {wanted:#x}"),
        );
        return;
    };
    if !s.resolves {
        push_system_chat_line(
            scene_state,
            format!(
                "/subarea {:#x}: interior DAT {} is not in this install",
                s.sub_area.id, s.sub_area.file_id
            ),
        );
        return;
    }

    // Handing the id to the latch rather than loading the DAT here keeps one
    // owner of the sub-area block, so a manual load cannot stack a second
    // interior on top of the one the player walked into. The latch governs from
    // there: an interior the player is not standing in drops again at once.
    set_sub_area.write(kuluu_render::sub_area_activation::SetSubArea {
        sub_area: Some(s.sub_area.id),
    });
    push_system_chat_line(
        scene_state,
        format!(
            "/subarea {:#x}: activating interior DAT {}…",
            s.sub_area.id, s.sub_area.file_id
        ),
    );
}

fn apply_copy_toasts(n: usize, scene_state: &mut SceneState) {
    let toasts = &scene_state.local_toasts;
    if toasts.is_empty() {
        push_system_chat_line(scene_state, "/copy: no toasts to copy".into());
        return;
    }
    let take = n.min(toasts.len());
    let start = toasts.len() - take;

    let payload: String = toasts[start..]
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(payload) {
            Ok(()) => {
                push_system_chat_line(scene_state, format!("/copy: {take} toast(s) on clipboard"));
            }
            Err(e) => {
                push_system_chat_line(scene_state, format!("/copy: clipboard write failed: {e}"));
            }
        },
        Err(e) => {
            push_system_chat_line(scene_state, format!("/copy: clipboard unavailable: {e}"));
        }
    }
}

#[cfg(unix)]
fn apply_agent_control(
    op: crate::view_native::slash_commands::AgentControlOp,
    agent_paused: Option<&crate::view_native::AgentPaused>,
    session_event_tx: Option<&crate::view_native::SessionEventTx>,
    scene_state: &mut SceneState,
) {
    use crate::view_native::slash_commands::AgentControlOp;
    use std::sync::atomic::Ordering;
    let Some(paused) = agent_paused else {
        push_system_chat_line(
            scene_state,
            "/agent: no agent attached (set --agent-listen to enable)".into(),
        );
        return;
    };
    match op {
        AgentControlOp::Pause => {
            let was_paused = paused.0.swap(true, Ordering::AcqRel);
            if was_paused {
                push_system_chat_line(scene_state, "/agent: already paused".into());
            } else {
                push_system_chat_line(scene_state, "/agent: paused (human in control)".into());
                if let Some(tx) = session_event_tx {
                    let _ = tx.0.send(AgentEvent::HumanInControl {
                        reason: "operator /agent pause".into(),
                    });
                }
            }
        }
        AgentControlOp::Resume => {
            let was_paused = paused.0.swap(false, Ordering::AcqRel);
            if !was_paused {
                push_system_chat_line(scene_state, "/agent: not currently paused".into());
            } else {
                push_system_chat_line(scene_state, "/agent: resumed".into());
                if let Some(tx) = session_event_tx {
                    let _ = tx.0.send(AgentEvent::HumanReleased);
                }
            }
        }
        AgentControlOp::Status => {
            let state = if paused.0.load(Ordering::Acquire) {
                "PAUSED (human in control)"
            } else {
                "RUNNING (agent in control)"
            };
            push_system_chat_line(scene_state, format!("/agent: {state}"));
        }
    }
}

fn report_nav_info(
    navmesh_state: &crate::view_native::navmesh_overlay::NavmeshState,
    self_pos: kuluu_snapshot::Vec3,
    scene_state: &mut SceneState,
) {
    let zone_id = scene_state.snapshot.zone_id;
    push_system_chat_line(
        scene_state,
        format!(
            "navinfo: self=(x={:.2} y={:.2} z={:.2}) zone={}",
            self_pos.x,
            self_pos.y,
            self_pos.z,
            zone_id.map_or("?".into(), |z| z.to_string()),
        ),
    );
    let Some(nav_arc) = navmesh_state.nav.as_ref() else {
        push_system_chat_line(
            scene_state,
            "navinfo: no navmesh loaded for current zone".into(),
        );
        return;
    };

    let nav = match nav_arc.lock() {
        Ok(g) => g,
        Err(_) => {
            push_system_chat_line(
                scene_state,
                "navinfo: navmesh mutex poisoned — bailing".into(),
            );
            return;
        }
    };
    let snap = nav.nearest_height_at(self_pos.x, self_pos.y, self_pos.z);
    match snap {
        Some(snapped_z) => {
            let delta_z = snapped_z - self_pos.z;
            push_system_chat_line(
                scene_state,
                format!(
                    "navinfo: nearest-poly z={:.2} (delta z={:+.2} yalms)",
                    snapped_z, delta_z
                ),
            );
        }
        None => push_system_chat_line(
            scene_state,
            "navinfo: NO walkable polygon within 100-yalm vertical search — self_pos appears off-mesh".into(),
        ),
    }

    if let Some(z) = zone_id {
        let lines = kuluu_nav::zone_lines_for(z);
        if lines.is_empty() {
            push_system_chat_line(scene_state, format!("navinfo: zone {z} has no zone-lines"));
            return;
        }
        let from = kuluu_nav::glam::Vec3::new(self_pos.x, self_pos.y, self_pos.z);
        for line in lines.iter().take(6) {
            let dx = line.from_pos[0] - self_pos.x;
            let dy = line.from_pos[1] - self_pos.y;
            let dz = line.from_pos[2] - self_pos.z;
            let dist_2d = (dx * dx + dy * dy).sqrt();
            let to =
                kuluu_nav::glam::Vec3::new(line.from_pos[0], line.from_pos[1], line.from_pos[2]);
            let path_status = match kuluu_nav::NavMesh::path(&*nav, from, to) {
                Some(p) => format!("path={}wp", p.len()),
                None => "path=NONE".into(),
            };
            let name = kuluu_nav::zone_name(line.to_zone).unwrap_or("?");
            push_system_chat_line(
                scene_state,
                format!(
                    "navinfo: →zone{:3} {:<20} dist={:.1}y dz={:+.1} {}",
                    line.to_zone, name, dist_2d, dz, path_status
                ),
            );
        }
    }
}

pub(super) fn apply_keybind_update(
    update: KeybindUpdate,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    scene_state: &mut SceneState,
) {
    match update {
        KeybindUpdate::Preset(preset) => {
            let (new_bindings, save_result) = keybinds_state.apply_preset(preset);
            *bindings = new_bindings;
            push_system_chat_line(
                scene_state,
                format!("/keybinds: preset → {}", preset.slug()),
            );
            if let Err(e) = save_result {
                push_system_chat_line(scene_state, format!("/keybinds: save failed: {e}"));
            }
        }
        KeybindUpdate::Reset => {
            let preset = keybinds_state.persisted.preset;
            let (new_bindings, save_result) = keybinds_state.apply_reset();
            *bindings = new_bindings;
            push_system_chat_line(
                scene_state,
                format!("/keybinds: reset to {} defaults", preset.slug()),
            );
            if let Err(e) = save_result {
                push_system_chat_line(scene_state, format!("/keybinds: save failed: {e}"));
            }
        }
        KeybindUpdate::List => {
            push_system_chat_line(
                scene_state,
                format!(
                    "/keybinds: preset = {}",
                    keybinds_state.persisted.preset.slug()
                ),
            );

            for (action, bind) in bindings.iter() {
                let mods = format_modifiers(bind.mods);
                push_system_chat_line(scene_state, format!("  {action:?} → {mods}{:?}", bind.key));
            }
        }
    }
}

fn format_modifiers(mods: kuluu_render::Modifiers) -> &'static str {
    match (mods.ctrl, mods.alt, mods.shift, mods.super_) {
        (false, false, false, false) => "",
        (true, false, false, false) => "Ctrl+",
        (false, true, false, false) => "Alt+",
        (false, false, true, false) => "Shift+",
        (false, false, false, true) => "Super+",

        _ => "Mod+",
    }
}

fn format_zoom_status(zoom: &kuluu_render::minimap::MinimapZoom) -> String {
    match zoom.radius_yalms {
        Some(r) => format!("/minimap zoom: radius={r:.0} yalms"),
        None => "/minimap zoom: fit-to-zone".into(),
    }
}

fn reqlogout_starts_countdown(cmd: &AgentCommand) -> Option<bool> {
    let AgentCommand::ReqLogout { kind } = cmd else {
        return None;
    };
    match kind {
        ReqLogoutKind::LogoutToggle | ReqLogoutKind::LogoutOn => Some(false),
        ReqLogoutKind::ShutdownToggle | ReqLogoutKind::ShutdownOn => Some(true),
        ReqLogoutKind::LogoutOff | ReqLogoutKind::ShutdownOff => None,
    }
}

fn reqlogout_ack_text(cmd: &AgentCommand) -> Option<&'static str> {
    let AgentCommand::ReqLogout { kind } = cmd else {
        return None;
    };
    Some(match kind {
        ReqLogoutKind::LogoutToggle | ReqLogoutKind::LogoutOn => {
            "/logout: requested (30s LeaveGame timer; movement or `/logout off` cancels)"
        }
        ReqLogoutKind::LogoutOff => "/logout: cancel requested",
        ReqLogoutKind::ShutdownToggle | ReqLogoutKind::ShutdownOn => {
            "/shutdown: requested (30s LeaveGame timer; movement or `/shutdown off` cancels)"
        }
        ReqLogoutKind::ShutdownOff => "/shutdown: cancel requested",
    })
}

fn mirror_heal_stance(cmd: &AgentCommand, rest: &mut kuluu_render::combat_stance::RestStance) {
    use kuluu_render::combat_stance::RestKind;
    let AgentCommand::Heal { mode } = cmd else {
        return;
    };
    let next = match mode {
        kuluu_session::state::HealMode::On => RestKind::Heal,
        kuluu_session::state::HealMode::Off => match rest.kind {
            RestKind::Heal => RestKind::None,
            other => other,
        },
        kuluu_session::state::HealMode::Toggle => match rest.kind {
            RestKind::Heal => RestKind::None,
            _ => RestKind::Heal,
        },
    };
    rest.kind = next;
}

#[cfg(test)]
mod reqlogout_ack_tests {
    use super::*;

    #[test]
    fn every_reqlogout_kind_has_ack_text() {
        for kind in [
            ReqLogoutKind::LogoutToggle,
            ReqLogoutKind::LogoutOn,
            ReqLogoutKind::LogoutOff,
            ReqLogoutKind::ShutdownToggle,
            ReqLogoutKind::ShutdownOn,
            ReqLogoutKind::ShutdownOff,
        ] {
            let text = reqlogout_ack_text(&AgentCommand::ReqLogout { kind })
                .unwrap_or_else(|| panic!("no toast for {kind:?}"));
            assert!(!text.is_empty(), "empty toast for {kind:?}");
        }
    }

    #[test]
    fn arming_variants_mention_cancellation() {
        for kind in [
            ReqLogoutKind::LogoutToggle,
            ReqLogoutKind::LogoutOn,
            ReqLogoutKind::ShutdownToggle,
            ReqLogoutKind::ShutdownOn,
        ] {
            let text = reqlogout_ack_text(&AgentCommand::ReqLogout { kind })
                .expect("arming variant has ack")
                .to_lowercase();
            assert!(
                text.contains("cancel") || text.contains("off"),
                "{kind:?} toast {text:?} should hint at cancellation",
            );
        }
    }

    #[test]
    fn non_reqlogout_command_returns_none() {
        let other = AgentCommand::Chat {
            kind: 0,
            text: "hi".into(),
        };
        assert!(reqlogout_ack_text(&other).is_none());
    }
}
