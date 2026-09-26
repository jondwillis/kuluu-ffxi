//! The DAT-grounded census: every event in every zone is driven through the
//! real VM (the only alignment authority — a linear walk desyncs on any
//! width the walk table gets wrong, and desynced bytes read as garbage
//! opcodes), and the VM's own emissions are tallied:
//!
//!  * every scheduler reference it resolves (0x45 family `Scheduler`,
//!    0x5B/0x66 `ExtScheduler`, 0x2C `ActorMotion`, 0x2D `ZoneScheduler`),
//!    each (dat_id, tag) checked against the DAT on disk;
//!  * every refusal (`Unimplemented`), with the exact `exec_pointer` and a
//!    hex dump of the authored bytes there, so a genuine unknown opcode is
//!    separable from a width-table desync that lands the pointer in data;
//!  * every spin and the summary counts.
//!
//! The output is the raw material for the done/not-done opcode coverage
//! table (one row per opcode, tag, and scheduler stage kind).
//!
//! `cargo run -p ffxi-event --example zz-cs-vm-census`

use std::collections::BTreeMap;

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::cue::{EventCue, ExtSchedulerMotion};
use ffxi_event::{EventVm, StepResult};

/// Enough to walk any authored event to its end; a bytecode loop would run
/// forever otherwise.
const STEP_LIMIT: usize = 4096;

/// Offline there is no host clock, so expire any authored wait in one jump.
const OFFLINE_WAIT_SKIP_SECS: f32 = 3600.0;

/// Answering every menu with option 0 re-opens the ones whose option 0 is
/// "browse again", so the sweep would loop on a shop forever. After this many
/// visits to the same menu, back out the way a player would.
const MENU_REVISITS_BEFORE_CANCEL: usize = 4;

fn fourcc(t: [u8; 4]) -> String {
    String::from_utf8_lossy(&t).into_owned()
}

fn main() {
    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let zones = ffxi_dat::event_locate::event_dat_zones(&root);
    let zone_count = zones.len();

    // (dat_id, tag) -> (count, first site)
    let mut sched_refs: BTreeMap<(u32, [u8; 4]), (u64, String)> = BTreeMap::new();
    // (zone, key) -> (count, first site) for 0x2D MAPSCHEDULOR
    let mut zone_refs: BTreeMap<(u16, [u8; 4]), (u64, String)> = BTreeMap::new();
    // key -> (count, first site) for 0x2C SCHEDULOR (the actor's own model DAT)
    let mut actor_motion_keys: BTreeMap<[u8; 4], (u64, String)> = BTreeMap::new();
    // (dat_id, key) -> (count, first site) for 0x5B/0x66; dat_id u32::MAX = Tpc
    let mut ext_refs: BTreeMap<(u32, [u8; 4]), (u64, String)> = BTreeMap::new();
    // op -> (count, first three (zone, event, exec_pointer, hex dump) sites)
    let mut refused: BTreeMap<u8, (u64, Vec<String>)> = BTreeMap::new();
    let mut spun: BTreeMap<u8, u64> = BTreeMap::new();
    let mut ended = 0u64;
    let mut cancelled = 0u64;
    let mut limited = 0u64;

    for &zone in &zones {
        let Ok(loc) = root.resolve(ffxi_dat::event_locate::event_dat_file_id(zone)) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            continue;
        };
        let Ok(dat) = EventDat::parse(&bytes) else {
            continue;
        };

        for block in &dat.blocks {
            for &event_id in &block.event_ids {
                let Some(mut vm) = EventVm::start(block, event_id, 0, vec![0; 8]) else {
                    continue;
                };
                let site = format!("z{zone} e{event_id}");
                let mut menu_visits: BTreeMap<u32, usize> = BTreeMap::new();
                let mut steps = 0;
                loop {
                    steps += 1;
                    if steps > STEP_LIMIT {
                        limited += 1;
                        break;
                    }
                    let result = vm.step();
                    // Drain the cues this step emitted before the result
                    // decides whether there is a next step.
                    for cue in vm.take_cues() {
                        match cue {
                            EventCue::Scheduler { dat_id, tag, .. } => {
                                let e =
                                    sched_refs.entry((dat_id, tag)).or_insert((0, site.clone()));
                                e.0 += 1;
                            }
                            EventCue::ZoneScheduler { key, .. } => {
                                let e = zone_refs.entry((zone, key)).or_insert((0, site.clone()));
                                e.0 += 1;
                            }
                            EventCue::ActorMotion { key, .. } => {
                                let e = actor_motion_keys.entry(key).or_insert((0, site.clone()));
                                e.0 += 1;
                            }
                            EventCue::ExtScheduler { motion, key, .. } => {
                                let dat_id = match motion {
                                    Some(ExtSchedulerMotion::Event(id)) => id,
                                    Some(ExtSchedulerMotion::Tpc(_)) => u32::MAX,
                                    None => u32::MAX,
                                };
                                let e = ext_refs.entry((dat_id, key)).or_insert((0, site.clone()));
                                e.0 += 1;
                            }
                            _ => {}
                        }
                    }
                    match result {
                        StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => {
                            vm.dismiss_message()
                        }
                        StepResult::AwaitChoice(c) => {
                            let visits = menu_visits.entry(c.message_id).or_default();
                            *visits += 1;
                            if *visits > MENU_REVISITS_BEFORE_CANCEL {
                                vm.select_choice(None)
                            } else {
                                vm.select_choice(Some(0))
                            }
                        }
                        StepResult::Done => {
                            ended += 1;
                            break;
                        }
                        StepResult::Cancelled => {
                            cancelled += 1;
                            break;
                        }
                        StepResult::Unimplemented(op) => {
                            let off = vm.exec_pointer();
                            let data = &block.event_data;
                            let lo = off.saturating_sub(8);
                            let hi = (off + 16).min(data.len());
                            let hex: Vec<String> = data[lo..hi]
                                .iter()
                                .enumerate()
                                .map(|(i, b)| {
                                    if lo + i == off {
                                        format!("[{b:02X}]")
                                    } else {
                                        format!("{b:02X}")
                                    }
                                })
                                .collect();
                            let entry = refused.entry(op).or_insert((0, Vec::new()));
                            entry.0 += 1;
                            entry.1.push(format!(
                                "z{zone} e{event_id} @block_off {off} block=0x{:08X} | {}",
                                block.actor,
                                hex.join(" ")
                            ));
                            break;
                        }
                        StepResult::Spun(op) => {
                            *spun.entry(op).or_default() += 1;
                            break;
                        }
                        StepResult::Waiting => vm.tick(OFFLINE_WAIT_SKIP_SECS),
                        StepResult::AwaitServerAck(_) => vm.ack_server(),
                    }
                }
            }
        }
    }

    println!(
        "# VM-driven census: {} zones; ended {ended} cancelled {cancelled} \
         step-limited {limited} refused {} spun {}",
        zone_count,
        refused.values().map(|(n, _)| n).sum::<u64>(),
        spun.values().sum::<u64>()
    );

    println!("\n## refused opcodes (genuine: the VM is the alignment authority)");
    let mut by_count: Vec<(&u8, &(u64, Vec<String>))> = refused.iter().collect();
    by_count.sort_by_key(|&(_, (n, _))| std::cmp::Reverse(*n));
    for (op, (n, sites)) in by_count {
        println!("0x{op:02X} x{n}");
        for s in sites {
            println!("    {s}");
        }
    }

    println!("\n## spun opcodes (a loop; names no work)");
    let mut spun_sorted: Vec<(&u8, &u64)> = spun.iter().collect();
    spun_sorted.sort_by_key(|&(_, n)| std::cmp::Reverse(*n));
    for (op, n) in spun_sorted {
        println!("0x{op:02X} x{n}");
    }

    println!(
        "\n## scheduler references (0x45 family): {} distinct (dat_id, tag)",
        sched_refs.len()
    );
    for ((dat_id, tag), (n, site)) in &sched_refs {
        println!(
            "    dat {dat_id} tag {} x{n} first {site} {}",
            fourcc(*tag),
            tag_exists(&root, *dat_id, *tag)
        );
    }

    println!(
        "\n## ext scheduler references (0x5B/0x66): {} distinct (dat_id, key)",
        ext_refs.len()
    );
    for ((dat_id, key), (n, site)) in &ext_refs {
        let exists = if *dat_id == u32::MAX {
            "tpc-or-none".to_string()
        } else {
            tag_exists(&root, *dat_id, *key)
        };
        println!(
            "    dat {dat_id} key {} x{n} first {site} {exists}",
            fourcc(*key)
        );
    }

    println!(
        "\n## zone scheduler references (0x2D): {} distinct (zone, key)",
        zone_refs.len()
    );
    for ((zone, key), (n, site)) in &zone_refs {
        let exists = ffxi_dat::scheduler::zone_scene_file_id(&root, *zone, *key)
            .map(|file_id| format!("yes (file {file_id})"))
            .unwrap_or_else(|| "NO".to_string());
        println!(
            "    zone {zone} key {} x{n} first {site} {exists}",
            fourcc(*key)
        );
    }

    println!(
        "\n## actor motion keys (0x2C, the actor's own model DAT): {} distinct",
        actor_motion_keys.len()
    );
    let mut am_sorted: Vec<(&[u8; 4], &(u64, String))> = actor_motion_keys.iter().collect();
    am_sorted.sort_by_key(|&(_, (n, _))| std::cmp::Reverse(*n));
    for (key, (n, site)) in am_sorted.iter().take(400) {
        println!("    key {} x{n} first {site}", fourcc(**key));
    }
}

/// Whether `dat_id` resolves on disk and carries a chunk named `tag`; the
/// answer is the census's existence column ("yes (kind N)" / "NO: ...").
fn tag_exists(root: &DatRoot, dat_id: u32, tag: [u8; 4]) -> String {
    let Ok(loc) = root.resolve(dat_id) else {
        return "NO: dat_id does not resolve".to_string();
    };
    let path = loc.path_under(root);
    let Ok(bytes) = std::fs::read(&path) else {
        return format!("NO: unreadable {}", path.display());
    };
    ffxi_dat::chunk::walk(&bytes)
        .find_map(|c| match c {
            Ok(c) if c.name == tag => Some(c.kind),
            Ok(_) => None,
            Err(_) => None,
        })
        .map(|kind| format!("yes (kind {kind})"))
        .unwrap_or_else(|| "NO: tag not in file".to_string())
}
