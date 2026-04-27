use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::particle_gen::{KeyFrameTrack, ParticleGeneratorDef};

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::dat_d3m::{d3m_material, decoded_texture_to_image, D3mBlendMode};
use crate::scheduler_runtime::{ActionAssets, SchedulerStageEvent, FFXI_FPS};
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
        });
    }
}

pub fn tick_particle_simulator(time: Res<Time>, mut sim: ResMut<ParticleSimulator>) {
    let frames = time.delta_secs() * FFXI_FPS;
    for g in &mut sim.generators {
        g.age_frames += frames;

        // research/xim: a maxLifeSpan of 0 marks a singleton — emit one particle once.
        let singleton = g.def.is_singleton();
        let emitting = g.age_frames <= g.emit_window_frames.max(1.0);
        if singleton {
            if g.particles.is_empty() && g.age_frames <= frames {
                emit(g, g.emit_window_frames.max(g.def.max_life_frames).max(1.0));
            }
        } else if emitting {
            g.emit_accum += frames;
            while g.emit_accum >= g.def.frames_per_emission {
                g.emit_accum -= g.def.frames_per_emission;
                for _ in 0..g.def.particles_per_emission {
                    emit(g, g.def.max_life_frames);
                }
            }
        }

        for p in &mut g.particles {
            p.age_frames += frames;
            p.pos += p.vel * frames;
        }
        g.particles.retain(|p| p.age_frames < p.life_frames);
    }
}

fn emit(g: &mut LiveGenerator, life_frames: f32) {
    g.particles.push(Particle {
        pos: Vec3::ZERO,
        vel: Vec3::from_array(g.def.init_velocity),
        age_frames: 0.0,
        life_frames: life_frames.max(1.0),
        rgb: Vec3::from_slice(&g.def.init_color[..3]),
        scale: Vec2::new(g.def.init_scale[0], g.def.init_scale[1]),
    });
}

pub fn sync_particle_meshes(
    cam: Query<&GlobalTransform, With<OperatorCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    let cam_rot = cam.iter().next().map(|t| t.rotation()).unwrap_or_default();

    let mut dead = Vec::new();
    for (i, g) in sim.generators.iter().enumerate() {
        if let Some(mesh) = meshes.get_mut(&g.mesh) {
            rebuild_mesh(g, cam_rot, mesh);
        }
        let done = g.particles.is_empty() && g.age_frames > g.emit_window_frames.max(1.0);
        if done {
            dead.push(i);
        }
    }

    for &i in dead.iter().rev() {
        let g = sim.generators.swap_remove(i);
        commands.entity(g.entity).despawn();
    }
}

fn rebuild_mesh(g: &LiveGenerator, cam_rot: Quat, mesh: &mut Mesh) {
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
        // Additive blend ignores alpha, so the alpha track drives brightness; with no track,
        // fade linearly to nothing over life.
        let alpha = g
            .alpha
            .as_ref()
            .map(|t| t.sample_from(progress, Some(g.def.init_color[3])))
            .unwrap_or(1.0 - progress);
        // Additive/subtract ignore the alpha channel, so the alpha curve modulates brightness;
        // alpha-blended particles keep full-brightness colour and use the alpha channel.
        let (rgb, vert_a) = match g.def.blend {
            ffxi_dat::particle_gen::ParticleBlend::Blend => (g.template.brightness * p.rgb, alpha),
            _ => (g.template.brightness * p.rgb * alpha, 1.0),
        };
        let world = g.origin + p.pos;

        let base = positions.len() as u32;
        for (tp, uv) in g.template.positions.iter().zip(&g.template.uvs) {
            let local = Vec3::new(tp.x * sx, tp.y * sy, tp.z);
            positions.push((world + cam_rot * local).to_array());
            uvs.push(*uv);
            colors.push([rgb.x, rgb.y, rgb.z, vert_a]);
        }
        indices.extend(g.template.indices.iter().map(|&idx| base + idx));
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
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

fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new());
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new());
    mesh.insert_indices(Indices::U32(Vec::new()));
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
            init_scale: [0.1, 0.1, 1.0],
            init_color: [0.2, 0.2, 0.6, 0.5],
            init_velocity: [0.0, 0.01, 0.0],
            init_rotation: [0.0; 3],
            blend: ffxi_dat::particle_gen::ParticleBlend::Additive,
            scale_x_track: None,
            scale_y_track: None,
            alpha_track: None,
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
        }
    }

    // Drive the emission math directly (no Bevy world): one frame's worth of advance per call.
    fn advance(g: &mut LiveGenerator, frames: f32) {
        g.age_frames += frames;
        if g.def.is_singleton() {
            if g.particles.is_empty() && g.age_frames <= frames {
                let l = g.emit_window_frames.max(g.def.max_life_frames).max(1.0);
                emit(g, l);
            }
        } else if g.age_frames <= g.emit_window_frames.max(1.0) {
            g.emit_accum += frames;
            while g.emit_accum >= g.def.frames_per_emission {
                g.emit_accum -= g.def.frames_per_emission;
                for _ in 0..g.def.particles_per_emission {
                    emit(g, g.def.max_life_frames);
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
    fn particle_expires_at_life() {
        let mut g = live(def(3.0, 1.0, 1), 1.0);
        advance(&mut g, 1.0); // emit one at age 0
        assert_eq!(g.particles.len(), 1);
        advance(&mut g, 5.0); // past life
        assert!(g.particles.is_empty());
    }
}
