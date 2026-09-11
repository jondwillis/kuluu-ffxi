use std::sync::Arc;

use ffxi_dat::{event_dat::EventDat, DatRoot};
use ffxi_event::vm::scene::{EventPosition, SceneAction};
use ffxi_event::{EventVm, StepResult};

const SELBINA: u16 = 248;
const LUCIA: u32 = 17_793_078;
const TICKET_EVENT: u16 = 221;
const FARE: i32 = 100;
const WEALTHY_GIL: i32 = 1_300_000;
const TICK_SECONDS: f32 = 0.2;
const MAX_STEPS: usize = 1000;

fn dat(zone: u16) -> Option<Arc<EventDat>> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = ffxi_dat::event_locate::zone_id_to_event_location(zone)?;
    let bytes = std::fs::read(loc.path_under(&root)).ok()?;
    Some(Arc::new(EventDat::parse(&bytes).unwrap()))
}

fn lucia(dat: Arc<EventDat>, gil: i32) -> EventVm {
    let block = dat.block_for_actor(LUCIA).unwrap();
    let mut vm = EventVm::start(block, TICKET_EVENT, 0, vec![gil, FARE]).unwrap();
    vm.attach_scene(
        dat,
        LUCIA,
        EventPosition {
            x: 30_552,
            y: -2_558,
            z: -27_800,
            heading: 0,
        },
    );
    vm
}

#[test]
fn retail_lucia_checks_balance_and_crosses_dock_only_on_purchase() {
    let Some(dat) = dat(SELBINA) else {
        eprintln!("SKIP: retail DAT unavailable");
        return;
    };
    for (gil, choice, purchases) in [
        (FARE - 1, 0, false),
        (FARE, 0, true),
        (WEALTHY_GIL, 0, true),
        (WEALTHY_GIL, 1, false),
    ] {
        let mut vm = lucia(dat.clone(), gil);
        let mut final_position = None;
        let mut ended = false;
        let mut messages = Vec::new();
        for _ in 0..MAX_STEPS {
            match vm.step() {
                StepResult::AwaitMessage(message) => {
                    messages.push(message.message_id);
                    vm.dismiss_message();
                }
                StepResult::AwaitMessageAck => vm.dismiss_message(),
                StepResult::AwaitChoice(_) => vm.select_choice(Some(choice)),
                StepResult::Waiting => vm.tick(TICK_SECONDS),
                StepResult::Done => {
                    ended = true;
                    break;
                }
                other => panic!("gil={gil} choice={choice}: {other:?}"),
            }
            for action in vm.take_scene_actions() {
                if let SceneAction::PositionUpdate { position, end_para } = action {
                    assert_eq!(end_para, 0);
                    assert!(position.z < -28_750, "{position:?}");
                    assert_eq!(vm.step(), StepResult::Waiting);
                    vm.acknowledge_event();
                    assert_eq!(
                        vm.step(),
                        StepResult::Waiting,
                        "event ack alone cannot finish the position update"
                    );
                    vm.acknowledge_position(position);
                    final_position = Some(position);
                }
            }
        }
        assert!(
            ended,
            "gil={gil} choice={choice}: did not finish; messages={messages:?}"
        );
        assert_eq!(
            final_position.is_some(),
            purchases,
            "gil={gil} choice={choice}; messages={messages:?}"
        );
    }
}

#[test]
fn cancel_during_dock_walk_discards_child_and_movement() {
    let Some(dat) = dat(SELBINA) else {
        eprintln!("SKIP: retail DAT unavailable");
        return;
    };
    let mut vm = lucia(dat, WEALTHY_GIL);
    for _ in 0..MAX_STEPS {
        match vm.step() {
            StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => vm.dismiss_message(),
            StepResult::AwaitChoice(_) => vm.select_choice(Some(0)),
            StepResult::Waiting => vm.tick(TICK_SECONDS),
            other => panic!("walk never started: {other:?}"),
        }
        if vm
            .take_scene_actions()
            .iter()
            .any(|a| matches!(a, SceneAction::PlayerPosition(_)))
        {
            vm.cancel_message();
            vm.tick(TICK_SECONDS);
            assert_eq!(vm.step(), StepResult::Cancelled);
            assert!(vm.take_scene_actions().is_empty());
            return;
        }
    }
    panic!("walk never started");
}

#[test]
fn retail_airship_exit_waits_for_both_acks_then_finishes_through_runner() {
    use ffxi_dat::dmsg::StringDat;
    use ffxi_event::{DialogRunner, DialogStep};
    const PORT_JEUNO: u16 = 246;
    const DEPARTURES_EXIT: u32 = 17_784_923;
    const ZEDDUVA: u32 = 17_784_853;
    const ADMISSION_EVENT: u16 = 36;
    let Some(dat) = dat(PORT_JEUNO) else {
        eprintln!("SKIP: retail DAT unavailable");
        return;
    };
    let root = DatRoot::from_env_or_default().unwrap();
    let strings_id = ffxi_dat::zone_dat::zone_id_to_string_file_id(PORT_JEUNO).unwrap();
    let strings = StringDat::parse(
        &std::fs::read(root.resolve(strings_id).unwrap().path_under(&root)).unwrap(),
    )
    .unwrap();
    let (block, _) = EventVm::driving_block(&dat, ZEDDUVA, ADMISSION_EVENT).unwrap();
    assert_eq!(block.actor, DEPARTURES_EXIT);
    let mut runner = DialogRunner::start(block, ADMISSION_EVENT, 0, vec![]).unwrap();
    runner.attach_scene(
        dat.clone(),
        DEPARTURES_EXIT,
        EventPosition {
            x: -61_000,
            y: 8_000,
            z: -55_000,
            heading: 0,
        },
    );
    let mut outcome = runner.advance(None, &strings);
    let mut requests = 0;
    let mut accepted_position = None;
    let mut published_position = None;
    for _ in 0..MAX_STEPS {
        for action in runner.take_scene_actions() {
            if let SceneAction::PlayerPosition(position) = action {
                published_position = Some(position);
            }
            if let SceneAction::PositionUpdate { position, .. } = action {
                requests += 1;
                accepted_position = Some(position);
                runner.acknowledge_position(position);
                assert_eq!(runner.tick(TICK_SECONDS, &strings), DialogStep::Waiting);
                runner.acknowledge_event();
            }
        }
        outcome = match outcome {
            DialogStep::Frame(frame) => {
                if !frame.choices.is_empty() {
                    assert_eq!(frame.params[7], 200);
                }
                runner.advance(Some(0), &strings)
            }
            DialogStep::Waiting => runner.tick(TICK_SECONDS, &strings),
            DialogStep::Ended { .. } => {
                assert_eq!(requests, 1);
                for action in runner.take_scene_actions() {
                    if let SceneAction::PlayerPosition(position) = action {
                        published_position = Some(position);
                    }
                }
                assert_eq!(published_position, accepted_position);
                return;
            }
            DialogStep::Stopped(op) => panic!("stopped on {op:#x}"),
        };
    }
    panic!("airship admission did not finish");
}

#[test]
fn retail_mhaura_purchase_returns_the_servers_ticket_option() {
    const MHAURA: u16 = 249;
    const FELISA: u32 = 17_797_172;
    // vendor/server/scripts/zones/Mhaura/npcs/Felisa.lua entity.onEventFinish.
    const PURCHASE_OPTION: i32 = 333;
    let Some(dat) = dat(MHAURA) else {
        eprintln!("SKIP: retail DAT unavailable");
        return;
    };
    let block = dat.block_for_actor(FELISA).unwrap();
    let mut vm = EventVm::start(block, TICKET_EVENT, 0, vec![WEALTHY_GIL, FARE]).unwrap();
    vm.attach_scene(
        dat.clone(),
        FELISA,
        EventPosition {
            x: 45_441,
            y: -7_999,
            z: 40_000,
            heading: 0,
        },
    );
    let mut crossed = false;
    for _ in 0..MAX_STEPS {
        match vm.step() {
            StepResult::AwaitMessage(_) | StepResult::AwaitMessageAck => vm.dismiss_message(),
            StepResult::AwaitChoice(_) => vm.select_choice(Some(0)),
            StepResult::Waiting => vm.tick(TICK_SECONDS),
            StepResult::Done => {
                assert!(crossed);
                assert_eq!(vm.work_zone(1), PURCHASE_OPTION);
                return;
            }
            other => panic!("{other:?}"),
        }
        for action in vm.take_scene_actions() {
            if let SceneAction::PositionUpdate { position, .. } = action {
                assert!(position.z < 39_000);
                crossed = true;
                vm.acknowledge_position(position);
                vm.acknowledge_event();
            }
        }
    }
    panic!("Mhaura purchase did not finish");
}
