//! Runs every event in every zone through the VM and reports what stops it, so a
//! change to opcode coverage can be measured against the whole retail corpus
//! rather than the one cutscene that prompted it.
//!
//! `cargo run -p ffxi-event --example zz-event-sweep [-- <zone>...]`

use std::collections::BTreeMap;

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::{EventVm, StepResult};

/// Enough to walk any authored event to its end; a bytecode loop would run
/// forever otherwise.
const STEP_LIMIT: usize = 4096;

/// Offline there is no host clock, so expire any authored wait in one jump.
const OFFLINE_WAIT_SKIP_SECS: f32 = 3600.0;

/// Answering every menu with option 0 re-opens the ones whose option 0 is
/// "browse again", so the sweep would loop on a shop forever and score a
/// perfectly good event as non-terminating. After this many visits to the same
/// menu, back out the way a player would.
const MENU_REVISITS_BEFORE_CANCEL: usize = 4;

fn main() {
    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let zones: Vec<u16> = {
        let named: Vec<u16> = std::env::args()
            .skip(1)
            .filter_map(|s| s.parse().ok())
            .collect();
        if named.is_empty() {
            (0..=299).collect()
        } else {
            named
        }
    };

    let mut ended = 0usize;
    let mut cancelled = 0usize;
    let mut ran_out = 0usize;
    let mut past_end = 0usize;
    let mut refusals: BTreeMap<u8, usize> = BTreeMap::new();
    let mut spins: BTreeMap<u8, usize> = BTreeMap::new();
    let mut limited_by: BTreeMap<&'static str, usize> = BTreeMap::new();

    for zone in zones {
        let Some(loc) = ffxi_dat::event_locate::zone_id_to_event_location(zone) else {
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
                let mut steps = 0;
                let mut last = "";
                let mut menu_visits: BTreeMap<u32, usize> = BTreeMap::new();
                loop {
                    steps += 1;
                    if steps > STEP_LIMIT {
                        ran_out += 1;
                        *limited_by.entry(last).or_default() += 1;
                        break;
                    }
                    match vm.step() {
                        StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => {
                            last = "message";
                            vm.dismiss_message()
                        }
                        StepResult::AwaitChoice(c) => {
                            last = "choice";
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
                            if vm.ran_past_end() {
                                past_end += 1;
                            }
                            break;
                        }
                        StepResult::Cancelled => {
                            cancelled += 1;
                            break;
                        }
                        StepResult::Unimplemented(op) => {
                            *refusals.entry(op).or_default() += 1;
                            break;
                        }
                        StepResult::Spun(op) => {
                            *spins.entry(op).or_default() += 1;
                            break;
                        }
                        StepResult::Waiting => {
                            last = "wait";
                            vm.tick(OFFLINE_WAIT_SKIP_SECS)
                        }
                    }
                }
            }
        }
    }

    let refused: usize = refusals.values().sum();
    let spun: usize = spins.values().sum();
    println!(
        "ended {ended} (ran past end {past_end})  cancelled {cancelled}  \
         refused {refused}  spun {spun}  step-limited {ran_out}"
    );
    print_ranked("refused (an opcode to implement)", refusals);
    print_ranked("spun (a loop, not a refusal -- names no work)", spins);
    println!("step-limited, by what the event kept yielding on:");
    for (kind, n) in &limited_by {
        println!("  {kind:<8} {n}");
    }
}

fn print_ranked(title: &str, counts: BTreeMap<u8, usize>) {
    println!("{title}:");
    let mut by_count: Vec<(u8, usize)> = counts.into_iter().collect();
    by_count.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (op, n) in by_count.iter().take(20) {
        println!("  0x{op:02X}  {n}");
    }
}
