use bevy::prelude::*;
use ffxi_nav::glam;

use ffxi_viewer_core::components::{IsSelf, Nameplate, WorldEntity};

use super::navmesh_overlay::NavmeshState;

const REACHED_TOLERANCE_YALMS: f32 = 1.0;

pub fn occlude_nameplates_system(
    nav: Res<NavmeshState>,
    self_q: Query<&Transform, (With<IsSelf>, Without<WorldEntity>)>,
    world_q: Query<(&Transform, &WorldEntity), Without<Nameplate>>,
    mut nameplate_q: Query<(&Nameplate, &mut Visibility)>,
) {
    let Some(nav_lock) = nav.nav.as_ref() else {
        return;
    };

    let Ok(self_t) = self_q.single() else { return };
    let Ok(guard) = nav_lock.lock() else { return };

    let to_ffxi = |b: Vec3| glam::Vec3::new(b.x, -b.z, -b.y);

    let mut pos_by_id: std::collections::HashMap<u32, Vec3> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w) in &world_q {
        pos_by_id.insert(w.id, t.translation);
    }

    let self_ffxi = to_ffxi(self_t.translation);

    for (np, mut vis) in &mut nameplate_q {
        let Some(&entity_pos_bevy) = pos_by_id.get(&np.entity_id) else {
            continue;
        };
        let entity_ffxi = to_ffxi(entity_pos_bevy);

        let origin = glam::Vec3::new(self_ffxi.x, self_ffxi.y, entity_ffxi.z);
        let target = glam::Vec3::new(entity_ffxi.x, entity_ffxi.y, entity_ffxi.z);
        let want = match guard.slide_along(origin, target) {
            Some(slid) => {
                let dx = slid.x - entity_ffxi.x;
                let dy = slid.y - entity_ffxi.y;
                let reached = (dx * dx + dy * dy).sqrt() <= REACHED_TOLERANCE_YALMS;
                if reached {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                }
            }

            None => Visibility::Inherited,
        };
        if *vis != want {
            *vis = want;
        }
    }
}
