//! The Home Point crystal's activation flash is authored in the event script,
//! not sent by the server: Bastok Markets event 8700's set-home-point branch
//! plays action `bind` on the event entity through 0x2C SCHEDULOR
//! (research/XiEvents/OpCodes/0x002C.md). LandSandBoat's homepoint.lua only
//! calls setHomePoint and messageSpecial on that choice, so this cue is the
//! whole trigger and the renderer has to honour it.

use ffxi_dat::dmsg::StringDat;
use ffxi_dat::event_dat::EventDat;
use ffxi_event::{DialogRunner, DialogStep, EventCue};

const BASTOK_MARKETS: u16 = 235;
const HOME_POINT_EVENT: u16 = 8700;
/// "Set this as your home point." on the "What will you do?" menu; the row index is also the
/// option word's low byte, vendor/server/scripts/globals/homepoint.lua `selection.SET_HOMEPOINT`.
const SET_HOMEPOINT_ROW: u32 = 1;
const SET_HOMEPOINT_LABEL: &str = "Set this as your home point.";
const CONFIRM_YES: u32 = 0;
const CONFIRM_NO: u32 = 1;
const MENU_ROWS: u32 = 4;
const MAX_STEPS: usize = 40;

fn load() -> Option<(EventDat, StringDat)> {
    let root = ffxi_dat::archive::open_test_install()?;
    let loc = root
        .resolve(ffxi_dat::event_locate::event_dat_file_id(BASTOK_MARKETS))
        .ok()?;
    let dat = EventDat::parse(&std::fs::read(loc.path_under(&root)).ok()?).ok()?;
    let file_id = ffxi_dat::zone_dat::string_dat_file_id(BASTOK_MARKETS);
    let sloc = root.resolve(file_id).ok()?;
    let strings = StringDat::parse(&std::fs::read(sloc.path_under(&root)).ok()?).ok()?;
    Some((dat, strings))
}

/// Drives the event choosing `first` on the "What will you do?" menu and `later` on
/// every menu after it, returning the cues in order. The event's own SENDTAG
/// and WAIT parks are answered the way a live host would, so they do not stop
/// the drive.
fn drive(
    dat: &EventDat,
    strings: &StringDat,
    first: u32,
    later: u32,
) -> (Vec<EventCue>, Vec<Vec<String>>) {
    let block = dat
        .blocks
        .iter()
        .find(|b| b.event_entry_exact(HOME_POINT_EVENT).is_some())
        .expect("a block owns the home point event");
    let mut runner = DialogRunner::start(block, HOME_POINT_EVENT, 0, vec![0; 8]).unwrap();
    let mut cues = Vec::new();
    let mut menus = Vec::new();
    let mut choice = None;
    for _ in 0..MAX_STEPS {
        let step = advance_past_parks(&mut runner, choice.take(), strings);
        cues.extend(runner.take_cues());
        match step {
            DialogStep::Frame(f) if !f.choices.is_empty() => {
                choice = Some(if menus.is_empty() { first } else { later });
                menus.push(f.choices);
            }
            DialogStep::Frame(_) => {}
            _ => break,
        }
    }
    (cues, menus)
}

/// Runs to the next dialog frame, answering the parks the event's own
/// choreography sets: the SENDTAG is acked the way the server would (a unit
/// test has no c2s/s2c round-trip) and a timed wait is run to expiry, the
/// pattern the runner's own tests use.
fn advance_past_parks(
    runner: &mut DialogRunner,
    choice: Option<u32>,
    strings: &StringDat,
) -> DialogStep {
    const WAIT_SKIP_SECS: f32 = 3600.0;
    let mut step = runner.advance(choice, strings);
    while matches!(step, DialogStep::Waiting | DialogStep::AwaitServerAck(_)) {
        step = if matches!(step, DialogStep::Waiting) {
            runner.tick(WAIT_SKIP_SECS, strings)
        } else {
            runner.ack_server(strings)
        };
    }
    step
}

fn bind_on_event_entity(cues: &[EventCue]) -> bool {
    cues.iter().any(|c| {
        matches!(
            c,
            EventCue::ActorMotion { actor1, actor2, key }
                if *key == *b"bind" && actor1.is_event_entity() && actor2.is_local_player()
        )
    })
}

#[test]
fn set_home_point_plays_bind_on_the_crystal_and_nothing_else_does() {
    let Some((dat, strings)) = load() else {
        return;
    };
    let mut firing = Vec::new();
    for first in 0..MENU_ROWS {
        for later in [CONFIRM_YES, CONFIRM_NO] {
            let (cues, menus) = drive(&dat, &strings, first, later);
            if bind_on_event_entity(&cues) {
                firing.push((first, later));
                assert_eq!(menus[0][SET_HOMEPOINT_ROW as usize], SET_HOMEPOINT_LABEL);
                assert_eq!(menus[1], ["Yes.", "No."]);
            }
        }
    }
    assert_eq!(
        firing,
        [(SET_HOMEPOINT_ROW, CONFIRM_YES)],
        "bind plays once, on a confirmed set-home-point"
    );
}
