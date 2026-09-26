//! Step-boundary trace: drives one event through the VM the census way and
//! prints the exec pointer before and after every step, so the step that
//! desyncs (or refuses) is bracketed and its bytecode can be read by hand.
//!
//! `cargo run -p ffxi-event --example zz-step-trace -- <zone> <event id>`

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::{EventVm, StepResult};

const STEP_LIMIT: usize = 8192;
const OFFLINE_WAIT_SKIP_SECS: f32 = 3600.0;
const MENU_REVISITS_BEFORE_CANCEL: usize = 4;

fn main() {
    let mut args = std::env::args().skip(1);
    let zone: u16 = args.next().and_then(|s| s.parse().ok()).expect("zone id");
    let event_id: u16 = args.next().and_then(|s| s.parse().ok()).expect("event id");

    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let loc = root
        .resolve(ffxi_dat::event_locate::event_dat_file_id(zone))
        .expect("resolve event DAT");
    let bytes = std::fs::read(loc.path_under(&root)).expect("read event DAT");
    let dat = EventDat::parse(&bytes).expect("parse event DAT");

    let owners: Vec<&ffxi_dat::event_dat::EventBlock> = dat
        .blocks
        .iter()
        .filter(|b| b.event_entry(event_id).is_some())
        .collect();
    for block in &owners {
        let Some(mut vm) = EventVm::start(block, event_id, 0, vec![0; 8]) else {
            continue;
        };
        let data = &block.event_data;
        let mut menu_visits: std::collections::BTreeMap<u32, usize> =
            std::collections::BTreeMap::new();
        let mut step = 0;
        loop {
            step += 1;
            if step > STEP_LIMIT {
                println!("step {step}: step-limited");
                break;
            }
            let before = vm.exec_pointer();
            let result = vm.step();
            let after = vm.exec_pointer();
            let summary = match &result {
                StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => "message",
                StepResult::AwaitChoice(_) => "choice",
                StepResult::Done => "done",
                StepResult::Cancelled => "cancelled",
                StepResult::Unimplemented(op) => {
                    println!(
                        "  refuse 0x{op:02X} @ {after}: {}",
                        hex(
                            data,
                            after.saturating_sub(8),
                            (after + 24).min(data.len()),
                            Some(after)
                        )
                    );
                    "refused"
                }
                StepResult::Spun(op) => {
                    println!(
                        "  spun 0x{op:02X} @ {after}: {}",
                        hex(
                            data,
                            after.saturating_sub(8),
                            (after + 24).min(data.len()),
                            Some(after)
                        )
                    );
                    "spun"
                }
                StepResult::Waiting => "waiting",
                StepResult::AwaitServerAck(_) => "pending",
            };
            if step <= 40 || summary == "refused" || summary == "spun" {
                println!("step {step:3}: {before:>7} -> {after:>7} ({summary})");
            }
            match result {
                StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => vm.dismiss_message(),
                StepResult::AwaitChoice(c) => {
                    let visits = menu_visits.entry(c.message_id).or_default();
                    *visits += 1;
                    if *visits > MENU_REVISITS_BEFORE_CANCEL {
                        vm.select_choice(None)
                    } else {
                        vm.select_choice(Some(0))
                    }
                }
                StepResult::Done | StepResult::Cancelled | StepResult::Unimplemented(_) => break,
                StepResult::Spun(_) => break,
                StepResult::Waiting => vm.tick(OFFLINE_WAIT_SKIP_SECS),
                StepResult::AwaitServerAck(_) => vm.ack_server(),
            }
        }
    }
}

fn hex(data: &[u8], lo: usize, hi: usize, mark: Option<usize>) -> String {
    data[lo..hi]
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if mark == Some(lo + i) {
                format!("[{b:02X}]")
            } else {
                format!("{b:02X}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
