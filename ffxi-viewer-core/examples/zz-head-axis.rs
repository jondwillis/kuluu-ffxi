use bevy::math::Vec3;
use ffxi_actor::skeleton_instance::{
    find_head_neck, neck_subtree, pose_world, standard_joint_world_position, RootTransform,
};
use ffxi_viewer_core::ffxi_actor_render::{load_npc, load_pc};

fn nearest_axis(v: Vec3) -> String {
    let cands = [
        ("+X", Vec3::X),
        ("-X", Vec3::NEG_X),
        ("+Y", Vec3::Y),
        ("-Y", Vec3::NEG_Y),
        ("+Z", Vec3::Z),
        ("-Z", Vec3::NEG_Z),
    ];
    let mut best = ("?", -2.0f32);
    for (n, a) in cands {
        let d = a.dot(v);
        if d > best.1 {
            best = (n, d);
        }
    }
    format!("{} (dot {:.3})", best.0, best.1)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let loaded = if args.get(1).map(|s| s == "npc").unwrap_or(false) {
        let id: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2056);
        println!("(npc {id})");
        load_npc(id).expect("load_npc failed")
    } else {
        let race: u8 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        load_pc(race, &[], None, None).expect("load_pc failed")
    };
    let skel = &loaded.skeleton;

    let pose = pose_world(skel, |_| None, RootTransform::identity(), &[]);

    let p = |i: usize| standard_joint_world_position(&pose, skel, i);
    let (abovehead, rfoot, lfoot, lhand, rhand) = (p(2), p(8), p(9), p(126), p(127));

    println!("=== joints {} ===", skel.joints.len());
    println!("AboveHead {abovehead:?}");
    println!("RightFoot {rfoot:?}  LeftFoot {lfoot:?}");
    println!("LeftHand  {lhand:?}  RightHand {rhand:?}");

    let Some((neck, head)) = find_head_neck(skel) else {
        println!("find_head_neck => None (no head tracking for this skeleton)");
        return;
    };
    println!(
        "neck={neck} head={head}  subtree={:?}",
        neck_subtree(skel, neck)
    );

    if let (Some(ah), Some(rf), Some(lf), Some(lh), Some(rh)) =
        (abovehead, rfoot, lfoot, lhand, rhand)
    {
        let feet = (rf + lf) * 0.5;
        let up = (ah - feet).normalize();
        let left = (lh - rh).normalize();
        let fwd_a = up.cross(left).normalize();
        let fwd_b = left.cross(up).normalize();
        println!("UP    {up:?}  ~ {}", nearest_axis(up));
        println!("LEFT  {left:?}  ~ {}", nearest_axis(left));
        println!("FWD up x left {fwd_a:?}  ~ {}", nearest_axis(fwd_a));
        println!("FWD left x up {fwd_b:?}  ~ {}", nearest_axis(fwd_b));
    }

    for (name, idx) in [("neck", neck), ("head", head)] {
        let (_, r, _) = pose[idx].to_scale_rotation_translation();
        println!(
            "{name} local axes in pose: X={} Y={} Z={}",
            nearest_axis(r * Vec3::X),
            nearest_axis(r * Vec3::Y),
            nearest_axis(r * Vec3::Z),
        );
    }
}
