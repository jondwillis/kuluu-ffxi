use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::particle_gen::{KeyFrameTrack, ParticleGeneratorDef};

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::dat_d3m::{d3m_material, decoded_texture_to_image, D3mBlendMode};
use crate::scheduler_runtime::{ActionAssets, MmbSpriteMesh, SchedulerStageEvent, FFXI_FPS};
use ffxi_dat::scheduler::StageKind;

// CPU particle simulation. research/xim ParticleGenerator + Particle: a Particle stage (0x02)
// spawns a `LiveGenerator` that streams billboard particles over its window, each integrating
// velocity and following per-particle keyframe tracks (scale/alpha) by life progress. One retained
// mesh entity per generator is rebuilt each frame from its live particles — not an entity per
// particle.
#[derive(Resource, Default)]
pub struct ParticleSimulator {
    generators: Vec<LiveGenerator>,
}

impl ParticleSimulator {
    pub fn drain_entities(&mut self) -> Vec<Entity> {
        self.generators.drain(..).map(|g| g.entity).collect()
    }
}

struct SpriteTemplate {
    positions: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    brightness: Vec3,
}

struct LiveGenerator {
    def: ParticleGeneratorDef,
    template: SpriteTemplate,
    scale_x: Option<KeyFrameTrack>,
    scale_y: Option<KeyFrameTrack>,
    alpha: Option<KeyFrameTrack>,
    origin: Vec3,
    particles: Vec<Particle>,
    emit_accum: f32,
    age_frames: f32,
    emit_window_frames: f32,
    mesh: Handle<Mesh>,
    entity: Entity,
    // research/xim ParticleGenerator.kt:56 — auto-run generators never finish
    // emitting; they live until their mesh entity (a child of the actor root)
    // is despawned.
    auto_run: bool,
    // Fixed particle orientation (init_rotation); None = camera billboard.
    orientation: Option<Quat>,
    // The mesh entity is a child of the actor root, so vertex positions are
    // built in the actor's FFXI-local frame instead of world space.
    actor_local: bool,
    // Accumulated UV-translate (def.uv_scroll integrated over life) added to every
    // template UV so a scrolling water sheet/cascade slides its texture.
    tex_translate: Vec2,
    // Per-axis sign applied to init_velocity/accel. Actor-local generators integrate
    // in the DAT frame (ONE); world-space zone generators build positions directly in
    // Bevy space, so velocity gets the same mzb->bevy basis (x,-y,-z) as the origin.
    vel_basis: Vec3,
}

// Auto-run particle generators embedded in an actor DAT (research/xim
// Actor.kt:724-734 startAutoRunParticles), attached at actor spawn by
// ffxi_actor_render and started by `spawn_actor_auto_run_particles`.
#[derive(Component)]
pub struct ActorAutoRunEffects {
    pub assets: std::sync::Arc<ActionAssets>,
}

struct Particle {
    pos: Vec3,
    vel: Vec3,
    age_frames: f32,
    life_frames: f32,
    rgb: Vec3,
    scale: Vec2,
}

pub fn spawn_particle_generators(
    mut events: MessageReader<SchedulerStageEvent>,
    q_actors: Query<(&Transform, &ActionAssets)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    for ev in events.read() {
        if ev.stage.stage.kind != StageKind::Particle {
            continue;
        }
        let Ok((actor_xf, assets)) = q_actors.get(ev.actor) else {
            continue;
        };
        let Some(def) = assets.particle_defs.get(&ev.stage.stage.id).copied() else {
            continue;
        };
        let Some(d3m) = assets.d3ms.get(&def.mesh_id) else {
            continue;
        };
        let Some(template) = sprite_template(d3m) else {
            continue;
        };

        let tex = d3m.texture_name[8..12]
            .try_into()
            .ok()
            .and_then(|name: [u8; 4]| assets.images.get(&name))
            .map(|t| images.add(decoded_texture_to_image(t)));
        let blend = match def.blend {
            ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
            ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
            ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
        };
        let mat = mats.add(d3m_material(blend, tex));
        let mesh = meshes.add(empty_mesh());

        let entity = commands
            .spawn((
                InGameEntity,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                Visibility::default(),
                // The mesh is rebuilt in place every frame; Bevy computes a frustum-culling Aabb
                // once from the initially-empty mesh and never recomputes it, so the entity would
                // be culled forever. Opt out of culling instead.
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();

        debug!(
            "spawned particle generator {} mesh {} life {}",
            String::from_utf8_lossy(&ev.stage.stage.id),
            String::from_utf8_lossy(&def.mesh_id),
            def.max_life_frames
        );

        let resolve = |id: Option<[u8; 4]>| -> Option<KeyFrameTrack> {
            id.and_then(|i| assets.keyframes.get(&i).cloned())
        };

        let emit_window_frames = ev.stage.stage.duration_frames as f32;
        sim.generators.push(LiveGenerator {
            scale_x: resolve(def.scale_x_track),
            scale_y: resolve(def.scale_y_track),
            alpha: resolve(def.alpha_track),
            template,
            def,
            origin: actor_xf.translation + Vec3::Y * def.base_position[1],
            particles: Vec::new(),
            emit_accum: 0.0,
            age_frames: 0.0,
            emit_window_frames,
            mesh,
            entity,
            auto_run: false,
            orientation: None,
            actor_local: false,
            tex_translate: Vec2::ZERO,
            vel_basis: Vec3::ONE,
        });
    }
}

// research/xim Actor.kt:127,724-734 — at model-ready, every generator in the
// actor DAT flagged auto-run starts immediately and emits forever. The mesh
// entity is a child of the actor root (which carries the FFXI->Bevy basis), so
// particle math stays in the DAT's own FFXI-local frame and the effect follows
// and despawns with the actor.
pub fn spawn_actor_auto_run_particles(
    q_added: Query<(Entity, &ActorAutoRunEffects), Added<ActorAutoRunEffects>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    for (actor_root, fx) in &q_added {
        for (name, def) in fx.assets.particle_defs.iter() {
            if !def.auto_run {
                continue;
            }
            let def = *def;
            let Some(d3m) = fx.assets.d3ms.get(&def.mesh_id) else {
                continue;
            };
            let Some(template) = sprite_template(d3m) else {
                continue;
            };

            let tex = d3m.texture_name[8..12]
                .try_into()
                .ok()
                .and_then(|tex_name: [u8; 4]| fx.assets.images.get(&tex_name))
                .map(|t| images.add(decoded_texture_to_image(t)));
            let blend = match def.blend {
                ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
                ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
                ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
            };
            let mat = mats.add(d3m_material(blend, tex));
            let mesh = meshes.add(empty_mesh());

            let entity = commands
                .spawn((
                    InGameEntity,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(mat),
                    Transform::IDENTITY,
                    ChildOf(actor_root),
                    bevy::camera::visibility::NoFrustumCulling,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                ))
                .id();

            debug!(
                "auto-run particle generator {} mesh {} blend {:?}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&def.mesh_id),
                def.blend,
            );

            let resolve = |id: Option<[u8; 4]>| -> Option<KeyFrameTrack> {
                id.and_then(|i| fx.assets.keyframes.get(&i).cloned())
            };
            let rot = def.init_rotation;
            sim.generators.push(LiveGenerator {
                scale_x: resolve(def.scale_x_track),
                scale_y: resolve(def.scale_y_track),
                alpha: resolve(def.alpha_track),
                template,
                origin: Vec3::from_array(def.base_position),
                particles: Vec::new(),
                emit_accum: 0.0,
                age_frames: 0.0,
                emit_window_frames: 0.0,
                mesh,
                entity,
                auto_run: true,
                orientation: (!def.camera_billboard)
                    .then(|| Quat::from_euler(EulerRot::XYZ, rot[0], rot[1], rot[2])),
                actor_local: true,
                tex_translate: Vec2::ZERO,
                vel_basis: Vec3::ONE,
                def,
            });
        }
    }
}

// research/xim EnvironmentManager zone-static Generator: an auto-run particle
// generator embedded in the zone MZB DAT (Bastok Mines pump spray), placed in
// world space rather than parented to an actor. `origin` is already mzb->bevy;
// velocity/accel take the same basis so the spray arcs in Bevy space.
pub fn spawn_zone_particle_generator(
    def: ParticleGeneratorDef,
    assets: &ActionAssets,
    origin: Vec3,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    sim: &mut ParticleSimulator,
    commands: &mut Commands,
) -> Option<Entity> {
    // Zone sprays link either a D3M billboard or an MMB mesh by DatId (e.g. Bastok
    // "abuk", Port Windurst "rivsea"); the MMB texture resolves by internal name.
    let (template, tex) = if let Some(d3m) = assets.d3ms.get(&def.mesh_id) {
        let template = sprite_template(d3m)?;
        let tex = d3m.texture_name[8..12]
            .try_into()
            .ok()
            .and_then(|name: [u8; 4]| assets.images.get(&name))
            .map(|t| images.add(decoded_texture_to_image(t)));
        (template, tex)
    } else {
        let mmb = assets.mmbs.get(&def.mesh_id)?;
        let template = mmb_sprite_template(mmb)?;
        let tex = assets
            .images_by_name
            .get(&mmb.texture_name)
            .map(|t| images.add(decoded_texture_to_image(t)));
        (template, tex)
    };
    let blend = match def.blend {
        ffxi_dat::particle_gen::ParticleBlend::Additive => D3mBlendMode::Additive,
        ffxi_dat::particle_gen::ParticleBlend::Blend => D3mBlendMode::Blended,
        ffxi_dat::particle_gen::ParticleBlend::Subtract => D3mBlendMode::Subtractive,
    };
    let mat = mats.add(d3m_material(blend, tex));
    let mesh = meshes.add(empty_mesh());

    let entity = commands
        .spawn((
            InGameEntity,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat),
            Transform::IDENTITY,
            Visibility::default(),
            bevy::camera::visibility::NoFrustumCulling,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ))
        .id();

    let resolve = |id: Option<[u8; 4]>| -> Option<KeyFrameTrack> {
        id.and_then(|i| assets.keyframes.get(&i).cloned())
    };
    let rot = def.init_rotation;
    sim.generators.push(LiveGenerator {
        scale_x: resolve(def.scale_x_track),
        scale_y: resolve(def.scale_y_track),
        alpha: resolve(def.alpha_track),
        template,
        origin,
        particles: Vec::new(),
        emit_accum: 0.0,
        age_frames: 0.0,
        emit_window_frames: 0.0,
        mesh,
        entity,
        auto_run: true,
        orientation: (!def.camera_billboard)
            .then(|| Quat::from_euler(EulerRot::XYZ, rot[0], rot[1], rot[2])),
        actor_local: false,
        tex_translate: Vec2::ZERO,
        vel_basis: Vec3::new(1.0, -1.0, -1.0),
        def,
    });
    Some(entity)
}

pub fn tick_particle_simulator(time: Res<Time>, mut sim: ResMut<ParticleSimulator>) {
    let frames = time.delta_secs() * FFXI_FPS;
    for g in &mut sim.generators {
        g.age_frames += frames;

        // research/xim ParticleGenerator.kt:66 — completed particles are swept
        // before emission, so a continuous singleton re-emits the same tick its
        // predecessor expires.
        g.particles.retain(|p| p.age_frames < p.life_frames);

        // research/xim: a maxLifeSpan of 0 marks a singleton — emit one particle once.
        let singleton = g.def.is_singleton();
        let emitting = g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0);
        if singleton {
            if g.particles.is_empty() && g.age_frames <= frames {
                emit(g, g.emit_window_frames.max(g.def.max_life_frames).max(1.0));
            }
        } else if emitting {
            g.emit_accum += frames;
            while g.emit_accum >= g.def.frames_per_emission {
                // research/xim ParticleGenerator.kt:80 — a continuous-singleton
                // generator holds one live particle and re-emits the moment it
                // expires (the accumulator stays primed, capped to one period).
                if g.def.continuous && !g.particles.is_empty() {
                    g.emit_accum = g.def.frames_per_emission;
                    break;
                }
                g.emit_accum -= g.def.frames_per_emission;
                for _ in 0..g.def.particles_per_emission {
                    emit(g, g.def.max_life_frames);
                    if g.def.continuous {
                        break;
                    }
                }
            }
        }

        // research/xim ParticleUpdaters TextureCoordinateUpdater: scroll velocity is
        // per-generator (frames of life advance the shared UV offset), not per-particle.
        g.tex_translate += Vec2::from_array(g.def.uv_scroll) * frames;

        let accel = g
            .def
            .accel
            .map(|a| Vec3::from_array(a) * g.vel_basis * frames);
        for p in &mut g.particles {
            p.age_frames += frames;
            if let Some(a) = accel {
                p.vel += a;
            }
            p.pos += p.vel * frames;
        }
        g.particles.retain(|p| p.age_frames < p.life_frames);
    }
}

fn emit(g: &mut LiveGenerator, life_frames: f32) {
    g.particles.push(Particle {
        pos: Vec3::ZERO,
        vel: Vec3::from_array(g.def.init_velocity) * g.vel_basis,
        age_frames: 0.0,
        life_frames: life_frames.max(1.0),
        rgb: Vec3::from_slice(&g.def.init_color[..3]),
        scale: Vec2::new(g.def.init_scale[0], g.def.init_scale[1]),
    });
}

pub fn sync_particle_meshes(
    cam: Query<&GlobalTransform, With<OperatorCamera>>,
    q_mesh_xf: Query<&GlobalTransform, With<Mesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    let cam_rot = cam.iter().next().map(|t| t.rotation()).unwrap_or_default();

    // (index, despawn-needed); indices ascending so the reverse sweep below can
    // swap_remove safely.
    let mut reap: Vec<(usize, bool)> = Vec::new();
    for (i, g) in sim.generators.iter().enumerate() {
        // The mesh entity despawns with its actor (auto-run generators are
        // children of the actor root); reap the simulator entry when it's gone.
        let Ok(entity_xf) = q_mesh_xf.get(g.entity) else {
            reap.push((i, false));
            continue;
        };
        // In the actor-local frame a billboard must cancel the parent's
        // FFXI->Bevy basis: parent_rot * rot == cam_rot. Fixed-orientation
        // meshes use their DAT rotation directly in the local frame.
        let rot = match (g.orientation, g.actor_local) {
            (Some(q), _) => q,
            (None, true) => entity_xf.rotation().inverse() * cam_rot,
            (None, false) => cam_rot,
        };
        if let Some(mut mesh) = meshes.get_mut(&g.mesh) {
            rebuild_mesh(g, rot, &mut mesh);
        }
        let done =
            !g.auto_run && g.particles.is_empty() && g.age_frames > g.emit_window_frames.max(1.0);
        if done {
            reap.push((i, true));
        }
    }

    for &(i, despawn) in reap.iter().rev() {
        let g = sim.generators.swap_remove(i);
        if despawn {
            commands.entity(g.entity).despawn();
        }
    }
}

fn rebuild_mesh(g: &LiveGenerator, rot: Quat, mesh: &mut Mesh) {
    let verts_per = g.template.positions.len();
    let n = g.particles.len();
    let mut positions = Vec::with_capacity(n * verts_per);
    let mut uvs = Vec::with_capacity(n * verts_per);
    let mut colors = Vec::with_capacity(n * verts_per);
    let mut indices = Vec::with_capacity(n * g.template.indices.len());

    for p in &g.particles {
        let progress = (p.age_frames / p.life_frames).clamp(0.0, 1.0);
        let sx = g
            .scale_x
            .as_ref()
            .map(|t| t.sample_from(progress, Some(p.scale.x)))
            .unwrap_or(p.scale.x);
        let sy = g
            .scale_y
            .as_ref()
            .map(|t| t.sample_from(progress, Some(p.scale.y)))
            .unwrap_or(p.scale.y);
        // Additive blend ignores alpha, so the alpha track drives brightness. With
        // no track, a transient spray fades linearly to nothing over life; a
        // continuous generator (one particle re-emitted on expiry — the steady
        // crystal body) holds full opacity, or each re-emit cycle would fade the
        // single particle out and strobe the whole model transparent.
        let alpha = g
            .alpha
            .as_ref()
            .map(|t| t.sample_from(progress, Some(g.def.init_color[3])))
            .unwrap_or(if g.def.continuous {
                1.0
            } else {
                1.0 - progress
            });
        // Additive/subtract ignore the alpha channel, so the alpha curve modulates brightness;
        // alpha-blended particles keep full-brightness colour and use the alpha channel.
        let (rgb, vert_a) = match g.def.blend {
            ffxi_dat::particle_gen::ParticleBlend::Blend => (g.template.brightness * p.rgb, alpha),
            _ => (g.template.brightness * p.rgb * alpha, 1.0),
        };
        let world = g.origin + p.pos;

        // Billboard sprites are flat (z unused); a fixed-orientation 3D particle
        // mesh keeps its DAT depth axis scaled by the untracked init z-scale.
        let sz = if g.orientation.is_some() {
            g.def.init_scale[2]
        } else {
            1.0
        };
        let base = positions.len() as u32;
        for (tp, uv) in g.template.positions.iter().zip(&g.template.uvs) {
            let local = Vec3::new(tp.x * sx, tp.y * sy, tp.z * sz);
            positions.push((world + rot * local).to_array());
            uvs.push([uv[0] + g.tex_translate.x, uv[1] + g.tex_translate.y]);
            colors.push([rgb.x, rgb.y, rgb.z, vert_a]);
        }
        indices.extend(g.template.indices.iter().map(|&idx| base + idx));
    }

    if positions.is_empty() {
        push_hidden_primitive(&mut positions, &mut uvs, &mut colors, &mut indices);
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}

// A generator with zero live particles (on spawn, and in the gaps between emit
// windows) would otherwise rebuild an empty mesh. Bevy's MeshAllocator skips the
// slab allocation for a zero-length vertex buffer but still runs the upload copy,
// logging "Use-after-free: attempted to copy element data for an unallocated key"
// (bevy_render slab_allocator.rs) every such frame. Keep the buffer non-empty with
// one zero-area, fully-transparent triangle so it uploads cleanly and draws nothing.
fn push_hidden_primitive(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
) {
    let base = positions.len() as u32;
    for _ in 0..3 {
        positions.push([0.0, 0.0, 0.0]);
        uvs.push([0.0, 0.0]);
        colors.push([0.0, 0.0, 0.0, 0.0]);
    }
    indices.extend([base, base + 1, base + 2]);
}

fn sprite_template(d3m: &ffxi_dat::d3m::D3m) -> Option<SpriteTemplate> {
    if d3m.vertices.is_empty() {
        return None;
    }
    let positions = d3m
        .vertices
        .iter()
        .map(|v| Vec3::from_array(v.pos))
        .collect();
    let uvs = d3m.vertices.iter().map(|v| v.uv).collect();
    let indices = (0..d3m.vertices.len() as u32).collect();
    let c = d3m.vertices[0].color;
    Some(SpriteTemplate {
        positions,
        uvs,
        indices,
        brightness: Vec3::new(c[0], c[1], c[2]),
    })
}

fn mmb_sprite_template(mmb: &MmbSpriteMesh) -> Option<SpriteTemplate> {
    if mmb.positions.is_empty() || mmb.indices.is_empty() {
        return None;
    }
    Some(SpriteTemplate {
        positions: mmb.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
        uvs: mmb.uvs.clone(),
        indices: mmb.indices.clone(),
        brightness: Vec3::from_array(mmb.brightness),
    })
}

fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let (mut positions, mut uvs, mut colors, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    push_hidden_primitive(&mut positions, &mut uvs, &mut colors, &mut indices);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::particle_gen::ParticleGeneratorDef;

    fn def(life: f32, fpe: f32, ppe: u32) -> ParticleGeneratorDef {
        ParticleGeneratorDef {
            frames_per_emission: fpe,
            particles_per_emission: ppe,
            emission_variance: 0.0,
            mesh_id: *b"gr  ",
            base_position: [0.0, 0.5, 0.0],
            max_life_frames: life,
            camera_billboard: true,
            continuous: false,
            auto_run: false,
            init_scale: [0.1, 0.1, 1.0],
            init_color: [0.2, 0.2, 0.6, 0.5],
            init_velocity: [0.0, 0.01, 0.0],
            init_rotation: [0.0; 3],
            blend: ffxi_dat::particle_gen::ParticleBlend::Additive,
            scale_x_track: None,
            scale_y_track: None,
            alpha_track: None,
            day_of_week_color: None,
            moon_phase_color: None,
            uv_scroll: [0.0, 0.0],
            accel: None,
        }
    }

    fn live(def: ParticleGeneratorDef, window: f32) -> LiveGenerator {
        LiveGenerator {
            def,
            template: SpriteTemplate {
                positions: vec![Vec3::ZERO; 3],
                uvs: vec![[0.0, 0.0]; 3],
                indices: vec![0, 1, 2],
                brightness: Vec3::ONE,
            },
            scale_x: None,
            scale_y: None,
            alpha: None,
            origin: Vec3::ZERO,
            particles: Vec::new(),
            emit_accum: 0.0,
            age_frames: 0.0,
            emit_window_frames: window,
            mesh: Handle::default(),
            entity: Entity::PLACEHOLDER,
            auto_run: false,
            orientation: None,
            actor_local: false,
            tex_translate: Vec2::ZERO,
            vel_basis: Vec3::ONE,
        }
    }

    // Drive the emission math directly (no Bevy world): one frame's worth of advance per call.
    // Mirrors tick_particle_simulator's per-generator body.
    fn advance(g: &mut LiveGenerator, frames: f32) {
        g.age_frames += frames;
        g.particles.retain(|p| p.age_frames < p.life_frames);
        if g.def.is_singleton() {
            if g.particles.is_empty() && g.age_frames <= frames {
                let l = g.emit_window_frames.max(g.def.max_life_frames).max(1.0);
                emit(g, l);
            }
        } else if g.auto_run || g.age_frames <= g.emit_window_frames.max(1.0) {
            g.emit_accum += frames;
            while g.emit_accum >= g.def.frames_per_emission {
                if g.def.continuous && !g.particles.is_empty() {
                    g.emit_accum = g.def.frames_per_emission;
                    break;
                }
                g.emit_accum -= g.def.frames_per_emission;
                for _ in 0..g.def.particles_per_emission {
                    emit(g, g.def.max_life_frames);
                    if g.def.continuous {
                        break;
                    }
                }
            }
        }
        for p in &mut g.particles {
            p.age_frames += frames;
            p.pos += p.vel * frames;
        }
        g.particles.retain(|p| p.age_frames < p.life_frames);
    }

    #[test]
    fn mesh_is_never_zero_length() {
        // Bevy's MeshAllocator errors on a zero-length vertex buffer, so an
        // empty generator (fresh spawn / between emit windows) must still
        // upload a non-empty mesh. Covers empty_mesh() and the empty rebuild.
        let count = |m: &Mesh| m.count_vertices();
        assert!(
            count(&empty_mesh()) > 0,
            "empty_mesh must not be zero-length"
        );

        let g = live(def(2.0, 1.0, 1), 3.0);
        assert!(g.particles.is_empty());
        let mut mesh = empty_mesh();
        rebuild_mesh(&g, Quat::IDENTITY, &mut mesh);
        assert!(count(&mesh) > 0, "empty rebuild must not be zero-length");
    }

    #[test]
    fn emits_one_per_period_over_window() {
        let mut g = live(def(100.0, 5.0, 1), 20.0);
        // 20 frames at 1/frame, period 5 -> 4 emits within window (the emit at accum reset).
        for _ in 0..20 {
            advance(&mut g, 1.0);
        }
        assert_eq!(g.particles.len(), 4);
    }

    #[test]
    fn stops_emitting_after_window() {
        let mut g = live(def(2.0, 1.0, 1), 3.0);
        for _ in 0..10 {
            advance(&mut g, 1.0);
        }
        // window 3 -> ~3 emitted, each lives 2 frames, all expired by frame 10.
        assert!(g.particles.is_empty());
    }

    #[test]
    fn singleton_emits_once() {
        let mut g = live(def(0.0, 1.0, 1), 30.0);
        for _ in 0..5 {
            advance(&mut g, 1.0);
        }
        assert_eq!(g.particles.len(), 1, "singleton emits exactly once");
        assert!(g.particles[0].pos.y > 0.0, "velocity integrated");
    }

    #[test]
    fn auto_run_keeps_emitting_past_window() {
        let mut g = live(def(2.0, 1.0, 1), 3.0);
        g.auto_run = true;
        for _ in 0..30 {
            advance(&mut g, 1.0);
        }
        assert!(
            !g.particles.is_empty(),
            "auto-run generators never stop emitting"
        );
    }

    #[test]
    fn continuous_singleton_holds_one_particle_and_replaces_on_expiry() {
        let mut d = def(4.0, 1.0, 3);
        d.continuous = true;
        let mut g = live(d, 1.0);
        g.auto_run = true;
        let mut max_alive = 0usize;
        let mut empty_streak = 0usize;
        let mut max_empty_streak = 0usize;
        for _ in 0..20 {
            advance(&mut g, 1.0);
            max_alive = max_alive.max(g.particles.len());
            if g.particles.is_empty() {
                empty_streak += 1;
                max_empty_streak = max_empty_streak.max(empty_streak);
            } else {
                empty_streak = 0;
            }
        }
        assert_eq!(
            max_alive, 1,
            "continuous singleton caps at one live particle"
        );
        assert!(
            max_empty_streak <= 1,
            "an expired particle is replaced within one tick (gap was {max_empty_streak})"
        );
    }

    #[test]
    fn continuous_trackless_generator_holds_constant_alpha() {
        // A continuous generator holds one particle re-emitted on expiry (the
        // steady crystal body). Track-less, it must stay fully opaque — if it fell
        // back to the 1.0-progress spray fade, the single particle would fade out
        // each cycle and strobe the whole model transparent.
        use ffxi_dat::particle_gen::ParticleBlend;
        let mut base = def(4.0, 1.0, 1);
        base.blend = ParticleBlend::Blend;
        base.init_color = [1.0, 1.0, 1.0, 0.8];

        let mut cont = live(base, 1.0);
        cont.def.continuous = true;
        let mut spray = live(base, 1.0);

        let particle = |age: f32| Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            age_frames: age,
            life_frames: 4.0,
            rgb: Vec3::ONE,
            scale: Vec2::splat(0.1),
        };
        cont.particles = vec![particle(3.0)];
        spray.particles = vec![particle(3.0)];

        let alpha_of = |g: &LiveGenerator| -> f32 {
            let mut mesh = empty_mesh();
            rebuild_mesh(g, Quat::IDENTITY, &mut mesh);
            match mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
                bevy::mesh::VertexAttributeValues::Float32x4(c) => c[0][3],
                _ => panic!("expected Float32x4 colours"),
            }
        };

        assert!(
            (alpha_of(&cont) - 1.0).abs() < 1e-4,
            "continuous body stays fully opaque, not the life fade"
        );
        assert!(
            (alpha_of(&spray) - 0.25).abs() < 1e-4,
            "a transient spray still fades 1.0-progress over life"
        );
    }

    #[test]
    fn particle_expires_at_life() {
        let mut g = live(def(3.0, 1.0, 1), 1.0);
        advance(&mut g, 1.0); // emit one at age 0
        assert_eq!(g.particles.len(), 1);
        advance(&mut g, 5.0); // past life
        assert!(g.particles.is_empty());
    }
}
