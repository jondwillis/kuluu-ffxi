use bevy::diagnostic::{DiagnosticPath, DiagnosticsStore, SystemInformationDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::diagnostic::{MeshAllocatorDiagnosticPlugin, RenderAssetDiagnosticPlugin};
use bevy::render::texture::GpuImage;
use bevy::time::Real;
use kuluu_render::components::MmbDebugInfo;
use kuluu_render::ffxi_actor_render::{ActorLoadInFlight, FfxiActorMeshChild};
use kuluu_render::hud::HudPanels;
use kuluu_render::snapshot::SceneState;

const REFRESH_SECONDS: f32 = 0.25;
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

#[derive(Component)]
pub struct DevelopmentSummaryText;

pub fn update_summary(
    panels: Res<HudPanels>,
    time: Res<Time<Real>>,
    diagnostics: Res<DiagnosticsStore>,
    scene: Res<SceneState>,
    loads: Option<Res<ActorLoadInFlight>>,
    meshes: Query<(&ViewVisibility, Has<MmbDebugInfo>, Has<FfxiActorMeshChild>), With<Mesh3d>>,
    images: Res<Assets<Image>>,
    mesh_assets: Res<Assets<Mesh>>,
    mut text: Query<&mut Text, With<DevelopmentSummaryText>>,
    mut next_refresh: Local<f32>,
    mut process_clock: Local<Option<(cpu_time::ProcessTime, std::time::Instant)>>,
) {
    if !panels.perf {
        *next_refresh = 0.0;
        *process_clock = None;
        return;
    }
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    if time.elapsed_secs() < *next_refresh {
        return;
    }
    *next_refresh = time.elapsed_secs() + REFRESH_SECONDS;
    let mut visible = 0;
    let mut zone = 0;
    let mut actors = 0;
    let mut other = 0;
    for (visibility, is_zone, is_actor) in &meshes {
        if visibility.get() {
            visible += 1;
            if is_zone {
                zone += 1;
            } else if is_actor {
                actors += 1;
            } else {
                other += 1;
            }
        }
    }
    let cpu_bytes: usize = images
        .iter()
        .filter_map(|(_, image)| image.data.as_ref())
        .map(Vec::len)
        .sum();
    let descriptor_bytes: usize = images
        .iter()
        .map(|(_, image)| kuluu_render::gpu_assets::image_bytes(image))
        .sum();
    let current_clock = cpu_time::ProcessTime::try_now().ok();
    let process_cpu = process_clock
        .take()
        .and_then(|(clock, wall)| {
            let seconds = wall.elapsed().as_secs_f64();
            clock
                .try_elapsed()
                .ok()
                .filter(|_| seconds > 0.0)
                .map(|cpu| {
                    format!(
                        "{:.1}% (100% = one logical CPU)",
                        cpu.as_secs_f64() / seconds * 100.0
                    )
                })
        })
        .unwrap_or_else(|| {
            if current_clock.is_some() {
                "collecting CPU time"
            } else {
                "unavailable (CPU clock)"
            }
            .into()
        });
    *process_clock = current_clock.map(|clock| (clock, std::time::Instant::now()));
    let process_memory = process_measurement(
        &diagnostics,
        &SystemInformationDiagnosticsPlugin::PROCESS_MEM_USAGE,
        "GiB",
    );
    let system_cpu = measurement(
        &diagnostics,
        &SystemInformationDiagnosticsPlugin::SYSTEM_CPU_USAGE,
        "%",
    );
    let system_memory = measurement(
        &diagnostics,
        &SystemInformationDiagnosticsPlugin::SYSTEM_MEM_USAGE,
        "%",
    );
    let mesh_memory = diagnostics
        .get(MeshAllocatorDiagnosticPlugin::slabs_size_diagnostic_path())
        .and_then(|d| d.value())
        .map(|bytes| format!("{:.1} MiB", bytes / BYTES_PER_MIB))
        .unwrap_or_else(|| "unavailable".into());
    let gpu_images = measurement(
        &diagnostics,
        &RenderAssetDiagnosticPlugin::<GpuImage>::render_asset_diagnostic_path(),
        "images",
    );
    let d = &scene.snapshot.diagnostics;
    let packet_age = d
        .last_server_packet_age_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "unavailable".into());
    let task_line = loads
        .as_ref()
        .map(|loads| {
            let counts = loads.diagnostic_counts();
            format!(
                "actor loads: queued {} | preparing {} | uploading {} | GPU wait {}",
                counts.queued, counts.preparing, counts.uploading, counts.awaiting_gpu
            )
        })
        .unwrap_or_else(|| "actor loads: unavailable".into());
    **text = format!(
        "CPU / MEMORY\nMachine CPU {system_cpu} | memory {system_memory}\nProcess CPU {process_cpu}\nProcess resident memory {process_memory}\n\nSCENE\nVisible mesh entities {visible} / {} | zone {zone} | actor parts {actors} | other {other}\nVisibility spans all views; mesh entities are not draw calls.\n\nASSETS\nImages {} | mesh assets {} | GPU prepared {gpu_images}\nImage CPU data {:.1} MiB | texture estimate {:.1} MiB | mesh buffers {mesh_memory}\nTexture estimate excludes driver overhead and is not measured VRAM.\n\nTASKS / I/O\n{task_line}\nTask durations, wait dependencies, and DAT I/O bytes: unavailable\n\nSESSION\nStage {:?} | zone {:?} | last server packet {packet_age}\nNetwork send {} B/s | receive {} B/s\n\nGPU\nLongest recorded pass: {}",
        meshes.iter().len(), images.len(), mesh_assets.len(), cpu_bytes as f64 / BYTES_PER_MIB, descriptor_bytes as f64 / BYTES_PER_MIB, scene.snapshot.stage, scene.snapshot.zone_id, scene.snapshot.net_stats.send_bps, scene.snapshot.net_stats.recv_bps, longest_gpu_pass(&diagnostics),
    );
}

fn process_measurement(
    diagnostics: &DiagnosticsStore,
    path: &DiagnosticPath,
    unit: &str,
) -> String {
    if cfg!(target_os = "macos") {
        "unavailable (Bevy macOS backend)".into()
    } else {
        measurement(diagnostics, path, unit)
    }
}

fn measurement(diagnostics: &DiagnosticsStore, path: &DiagnosticPath, unit: &str) -> String {
    diagnostics
        .get(path)
        .and_then(|d| d.value())
        .map(|v| format!("{v:.1} {unit}"))
        .unwrap_or_else(|| "unavailable".into())
}

fn longest_gpu_pass(diagnostics: &DiagnosticsStore) -> String {
    diagnostics
        .iter()
        .filter(|d| {
            d.path().as_str().starts_with("render/") && d.path().as_str().ends_with("/elapsed_gpu")
        })
        .filter_map(|d| {
            d.value()
                .filter(|value| value.is_finite())
                .map(|value| (d.path(), value))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(path, value)| {
            format!(
                "{} {value:.2} ms",
                path.as_str()
                    .trim_start_matches("render/")
                    .trim_end_matches("/elapsed_gpu")
            )
        })
        .unwrap_or_else(|| "unavailable (no GPU timestamp measurements)".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::diagnostic::{Diagnostic, DiagnosticMeasurement};
    use bevy::platform::time::Instant;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_backend_stubs_are_not_reported_as_measurements() {
        let mut diagnostics = DiagnosticsStore::default();
        let path = SystemInformationDiagnosticsPlugin::PROCESS_MEM_USAGE;
        let mut diagnostic = Diagnostic::new(path.clone());
        diagnostic.add_measurement(DiagnosticMeasurement {
            time: Instant::now(),
            value: 0.0,
        });
        diagnostics.add(diagnostic);
        assert_eq!(
            process_measurement(&diagnostics, &path, "GiB"),
            "unavailable (Bevy macOS backend)"
        );
    }

    #[test]
    fn cpu_timings_and_pipeline_counts_do_not_become_gpu_measurements() {
        let mut diagnostics = DiagnosticsStore::default();
        for path in ["render/main/elapsed_cpu", "render/main/primitives"] {
            let mut diagnostic = Diagnostic::new(DiagnosticPath::new(path));
            diagnostic.add_measurement(DiagnosticMeasurement {
                time: Instant::now(),
                value: 1.0,
            });
            diagnostics.add(diagnostic);
        }
        assert!(longest_gpu_pass(&diagnostics).starts_with("unavailable"));
        let mut diagnostic = Diagnostic::new(DiagnosticPath::new("render/shadows/elapsed_gpu"));
        diagnostic.add_measurement(DiagnosticMeasurement {
            time: Instant::now(),
            value: 0.0,
        });
        diagnostics.add(diagnostic);
        assert_eq!(longest_gpu_pass(&diagnostics), "shadows 0.00 ms");
    }
}
