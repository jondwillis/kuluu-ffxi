//! THROWAWAY PROBE (adversarial verification of the "evt-time" host mapping
//! claim). Delete after reading.
//!
//! Part 1 scans the retail install's event DATs and reports the real
//! distribution of 0x1C wait operands (in 1/60 s units) and 0x6F sleep counts.
//! Part 2 simulates retail's frame-delta model (XIClient GameManager::CheckTick)
//! against the proposed "elapsed_seconds * 60, clamp 20" host mapping.

use std::collections::BTreeMap;

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::opcode_meta::OPCODE_META;

const OP_END: u8 = 0x00;
const OP_EXECEND: u8 = 0x21;
const OP_WAIT: u8 = 0x1C;
const OP_SLEEP: u8 = 0x6F;
const OP_TRANSPAR: u8 = 0x6C;
const OP_DELIVERY: u8 = 0xB2;
const REFERENCE_FLAG: u32 = 0x8000;
const REFERENCE_INDEX_MASK: u32 = 0x7FFF;

fn install() -> Option<DatRoot> {
    DatRoot::from_env_or_default().ok()
}

#[test]
fn probe_wait_operand_distribution() {
    let Some(root) = install() else {
        eprintln!("skipping: no FFXI install");
        return;
    };

    let mut wait_literals: BTreeMap<i64, usize> = BTreeMap::new();
    let mut wait_nonliteral = 0usize;
    let mut sleeps = 0usize;
    let mut transpar = 0usize;
    let mut delivery = 0usize;
    let mut zones = 0usize;

    for zone in 1u16..=300 {
        let Some(eloc) = ffxi_dat::event_locate::zone_id_to_event_location(zone) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(eloc.path_under(&root)) else {
            continue;
        };
        let Ok(edat) = EventDat::parse(&bytes) else {
            continue;
        };
        zones += 1;

        for block in &edat.blocks {
            let data = &block.event_data;
            for &entry in &block.event_offsets {
                // Linear size-walk from the entry; stop at END/EXECEND, an
                // out-of-table opcode, or a branch we cannot follow statically.
                let mut pc = entry as usize;
                let mut budget = 4096;
                while pc < data.len() && budget > 0 {
                    budget -= 1;
                    let op = data[pc];
                    if op == OP_END || op == OP_EXECEND {
                        break;
                    }
                    let Some(meta) = OPCODE_META.get(op as usize).filter(|m| m.valid) else {
                        break;
                    };
                    match op {
                        OP_WAIT => {
                            let lo = *data.get(pc + 1).unwrap_or(&0) as u32;
                            let hi = *data.get(pc + 2).unwrap_or(&0) as u32;
                            let code = lo | (hi << 8);
                            if code & REFERENCE_FLAG != 0 {
                                let v = block
                                    .references
                                    .get((code & REFERENCE_INDEX_MASK) as usize)
                                    .copied()
                                    .unwrap_or(0) as i32;
                                *wait_literals.entry(v as i64).or_default() += 1;
                            } else {
                                wait_nonliteral += 1;
                            }
                        }
                        OP_SLEEP => sleeps += 1,
                        OP_TRANSPAR => transpar += 1,
                        OP_DELIVERY => delivery += 1,
                        _ => {}
                    }
                    if meta.jumps {
                        break;
                    }
                    pc += meta.size as usize;
                }
            }
        }
    }

    let total: usize = wait_literals.values().sum();
    eprintln!("zones scanned: {zones}");
    eprintln!(
        "0x1C waits: {total} literal, {wait_nonliteral} work-var; \
         0x6F sleeps: {sleeps}; 0x6C fades: {transpar}; 0xB2: {delivery}"
    );
    eprintln!("--- 0x1C literal operand histogram (units of 1/60 s) ---");
    let mut rows: Vec<_> = wait_literals.iter().collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (v, n) in rows.iter().take(30) {
        eprintln!("  {v:>8} units ({:>8.3} s nominal) x{n}", **v as f64 / 60.0);
    }
    let small = wait_literals
        .iter()
        .filter(|(v, _)| **v > 0 && **v <= 20)
        .map(|(_, n)| *n)
        .sum::<usize>();
    eprintln!(
        "waits of 1..=20 units (i.e. <= one 333 ms clamp step): {small}/{total} = {:.1}%",
        100.0 * small as f64 / total.max(1) as f64
    );
    let sub_tick = wait_literals
        .iter()
        .filter(|(v, _)| **v > 0 && **v <= 12)
        .map(|(_, n)| *n)
        .sum::<usize>();
    eprintln!(
        "waits of 1..=12 units (finish inside ONE 200 ms reactor drive): {sub_tick}/{total} = {:.1}%",
        100.0 * sub_tick as f64 / total.max(1) as f64
    );
}

/// Retail per-frame delta: 4-frame moving average of 60/fps, truncated to an
/// int, clamped to [FPSDivisor, 20] (XIClient GameManager::UpdateTimers +
/// CheckTick). Steady-state fps, so the moving average is a no-op.
fn retail_frame_delta(fps: f64, fps_divisor: f64) -> f64 {
    let avg = 60.0 / fps;
    let trunc = avg.trunc();
    trunc.clamp(fps_divisor, 20.0)
}

/// Wall-clock seconds a `units`-unit WaitTime takes at a steady `fps`, under
/// retail's model (one decrement per frame, advance when < 0).
fn retail_wait_seconds(units: f64, fps: f64, fps_divisor: f64) -> f64 {
    let d = retail_frame_delta(fps, fps_divisor);
    let mut remaining = units;
    let mut frames = 0.0;
    while remaining >= 0.0 {
        remaining -= d;
        frames += 1.0;
    }
    frames / fps
}

/// The claimed host mapping: subtract elapsed*60 per drive, clamped to 20.
fn claimed_wait_seconds(units: f64, drive_hz: f64) -> f64 {
    let dt = 1.0 / drive_hz;
    let d = (dt * 60.0).min(20.0);
    let mut remaining = units;
    let mut drives = 0.0;
    while remaining >= 0.0 {
        remaining -= d;
        drives += 1.0;
    }
    drives * dt
}

#[test]
fn probe_model_comparison() {
    eprintln!("=== retail model: 16-unit 0x6F sleep (nominal 0.2667 s) ===");
    for fps in [
        60.0, 30.0, 29.0, 25.0, 24.0, 20.5, 20.0, 15.0, 12.0, 10.0, 5.0, 2.0,
    ] {
        let d = retail_frame_delta(fps, 2.0);
        let s = retail_wait_seconds(16.0, fps, 2.0);
        eprintln!(
            "  fps {fps:>5.1}: delta {d:>4.1} units/frame -> {s:.4} s  ({:+.1}% vs nominal)",
            100.0 * (s / (16.0 / 60.0) - 1.0)
        );
    }

    eprintln!("=== retail model at FPSDivisor=1 (60 fps config) ===");
    for fps in [60.0, 45.0, 40.0, 30.0] {
        let d = retail_frame_delta(fps, 1.0);
        let s = retail_wait_seconds(16.0, fps, 1.0);
        eprintln!(
            "  fps {fps:>5.1}: delta {d:>4.1} units/frame -> {s:.4} s  ({:+.1}% vs nominal)",
            100.0 * (s / (16.0 / 60.0) - 1.0)
        );
    }

    eprintln!("=== claimed mapping: same 16-unit wait, varying host drive rate ===");
    for hz in [144.0, 60.0, 30.0, 10.0, 5.0, 3.0, 2.0, 1.0] {
        let s = claimed_wait_seconds(16.0, hz);
        eprintln!(
            "  drive {hz:>6.1} Hz: {s:.4} s  ({:+.1}% vs nominal)",
            100.0 * (s / (16.0 / 60.0) - 1.0)
        );
    }

    eprintln!("=== claimed mapping: 1-unit wait (16.7 ms nominal) ===");
    for hz in [144.0, 60.0, 30.0, 5.0] {
        let s = claimed_wait_seconds(1.0, hz);
        eprintln!(
            "  drive {hz:>6.1} Hz: {s:.4} s  ({:+.0}% vs nominal)",
            100.0 * (s / (1.0 / 60.0) - 1.0)
        );
    }

    eprintln!("=== claimed mapping: 600-unit wait (10 s nominal), slow drives ===");
    for hz in [5.0, 3.0, 2.0, 1.0, 0.5] {
        let s = claimed_wait_seconds(600.0, hz);
        eprintln!(
            "  drive {hz:>6.1} Hz: {s:.4} s  ({:+.1}% vs nominal)",
            100.0 * (s / 10.0 - 1.0)
        );
    }

    eprintln!("=== 0x6C AlphaTime is uint16: fractional delta truncates ===");
    for hz in [30.0, 60.0, 120.0, 144.0] {
        let dt: f64 = 1.0 / hz;
        let d: f64 = (dt * 60.0).min(20.0);
        // retail: AlphaTime (uint16) -= delta; terminate when bit 0x8000 set.
        let mut alpha_time: i32 = 30; // 0.5 s nominal fade
        let mut now_alpha = 0.0f64;
        let ofs = (255.0 - 0.0) / 30.0;
        let mut drives = 0;
        loop {
            let next = (alpha_time as f64) - d;
            alpha_time = next as i32; // C truncation into the uint16 field
            drives += 1;
            if alpha_time <= 0 {
                break;
            }
            now_alpha += d * ofs;
            if drives > 10_000 {
                break;
            }
        }
        eprintln!(
            "  drive {hz:>6.1} Hz (delta {d:.3}): countdown ended after {drives} drives = \
             {:.4} s wall, alpha reached {now_alpha:.0}/255",
            drives as f64 * dt
        );
    }
}
