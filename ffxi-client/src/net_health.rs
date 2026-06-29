use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const MAP_SILENCE_TIMEOUT: Duration = Duration::from_secs(60);

const RATE_REFRESH: Duration = Duration::from_secs(3);
const RECV_DELTA_WINDOW: usize = 64;
const SILENCE_GRACE: Duration = Duration::from_secs(3);
const SEND_ACK_TOLERANCE: u16 = 2;
const SEND_LAG_SPAN: u16 = 12;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetStatsSample {
    pub send_bps: u32,
    pub recv_bps: u32,
    pub send_health: u8,
    pub recv_health: u8,
}

#[derive(Debug, Default)]
pub struct NetHealth {
    last_rate_at: Option<Instant>,
    last_total_sent: u64,
    last_total_recv: u64,
    send_bps: u32,
    recv_bps: u32,

    prev_server_seq: Option<u16>,
    recv_deltas: VecDeque<u16>,
    server_ack_of_us: u16,
}

impl NetHealth {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_recv(&mut self, server_seq: u16, server_ack_of_us: u16) {
        if let Some(prev) = self.prev_server_seq {
            let delta = server_seq.wrapping_sub(prev).clamp(1, 64);
            self.recv_deltas.push_back(delta);
            while self.recv_deltas.len() > RECV_DELTA_WINDOW {
                self.recv_deltas.pop_front();
            }
        }
        self.prev_server_seq = Some(server_seq);
        self.server_ack_of_us = server_ack_of_us;
    }

    pub fn sample_rates(&mut self, now: Instant, total_sent: u64, total_recv: u64) {
        match self.last_rate_at {
            None => {
                self.last_rate_at = Some(now);
                self.last_total_sent = total_sent;
                self.last_total_recv = total_recv;
            }
            Some(prev) => {
                let elapsed = now.duration_since(prev);
                if elapsed < RATE_REFRESH {
                    return;
                }
                let ms = (elapsed.as_millis() as u64).max(1);
                let d_sent = total_sent.saturating_sub(self.last_total_sent);
                let d_recv = total_recv.saturating_sub(self.last_total_recv);
                self.send_bps = (d_sent.saturating_mul(1000) / ms) as u32;
                self.recv_bps = (d_recv.saturating_mul(1000) / ms) as u32;
                self.last_rate_at = Some(now);
                self.last_total_sent = total_sent;
                self.last_total_recv = total_recv;
            }
        }
    }

    fn recv_ring_health(&self) -> u8 {
        if self.recv_deltas.is_empty() {
            return 100;
        }
        let received = self.recv_deltas.len() as u32;
        let expected: u32 = self.recv_deltas.iter().map(|&d| d as u32).sum();
        ((received * 100) / expected.max(1)) as u8
    }

    fn send_ack_health(&self, last_sent_seq: u16) -> u8 {
        let lag = last_sent_seq.wrapping_sub(self.server_ack_of_us);
        let lag = if lag > 0x8000 { 0 } else { lag };
        if lag <= SEND_ACK_TOLERANCE {
            return 100;
        }
        let over = lag - SEND_ACK_TOLERANCE;
        if over >= SEND_LAG_SPAN {
            0
        } else {
            (100 - (over as u32 * 100 / SEND_LAG_SPAN as u32)) as u8
        }
    }

    fn silence_health(since_last_recv: Duration) -> u8 {
        if since_last_recv <= SILENCE_GRACE {
            return 100;
        }
        if since_last_recv >= MAP_SILENCE_TIMEOUT {
            return 0;
        }
        let span = (MAP_SILENCE_TIMEOUT - SILENCE_GRACE).as_millis() as u64;
        let into = (since_last_recv - SILENCE_GRACE).as_millis() as u64;
        (100 - (into * 100 / span.max(1))) as u8
    }

    pub fn snapshot(&self, since_last_recv: Duration, last_sent_seq: u16) -> NetStatsSample {
        let silence = Self::silence_health(since_last_recv);
        NetStatsSample {
            send_bps: self.send_bps,
            recv_bps: self.recv_bps,
            send_health: self.send_ack_health(last_sent_seq).min(silence),
            recv_health: self.recv_ring_health().min(silence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_recv_is_full_recv_health() {
        let mut h = NetHealth::new();
        for s in 100u16..120 {
            h.on_recv(s, 0);
        }
        assert_eq!(h.snapshot(Duration::ZERO, 0).recv_health, 100);
    }

    #[test]
    fn dropped_recv_bundles_lower_recv_health() {
        let mut h = NetHealth::new();
        h.on_recv(10, 0);
        h.on_recv(12, 0);
        h.on_recv(14, 0);
        assert_eq!(h.snapshot(Duration::ZERO, 0).recv_health, 50);
    }

    #[test]
    fn recv_seq_wraps_without_panic_or_false_loss() {
        let mut h = NetHealth::new();
        h.on_recv(u16::MAX - 1, 0);
        h.on_recv(u16::MAX, 0);
        h.on_recv(0, 0);
        h.on_recv(1, 0);
        assert_eq!(h.snapshot(Duration::ZERO, 0).recv_health, 100);
    }

    #[test]
    fn send_health_full_when_server_acks_keep_up() {
        let mut h = NetHealth::new();
        h.on_recv(1, 50);
        assert_eq!(h.snapshot(Duration::ZERO, 51).send_health, 100);
    }

    #[test]
    fn send_health_drops_to_zero_when_acks_stall() {
        let mut h = NetHealth::new();
        h.on_recv(1, 50);
        assert_eq!(
            h.snapshot(Duration::ZERO, 50 + SEND_ACK_TOLERANCE + SEND_LAG_SPAN)
                .send_health,
            0
        );
    }

    #[test]
    fn ack_ahead_of_sent_is_not_treated_as_loss() {
        let mut h = NetHealth::new();
        h.on_recv(1, 5);
        assert_eq!(h.snapshot(Duration::ZERO, 4).send_health, 100);
    }

    #[test]
    fn silence_caps_both_arrows() {
        let mut h = NetHealth::new();
        for s in 0u16..10 {
            h.on_recv(s, 0);
        }
        let healthy = h.snapshot(Duration::ZERO, 0);
        assert_eq!(healthy.recv_health, 100);
        assert_eq!(healthy.send_health, 100);

        let gone = h.snapshot(MAP_SILENCE_TIMEOUT, 0);
        assert_eq!(gone.recv_health, 0);
        assert_eq!(gone.send_health, 0);
    }

    #[test]
    fn silence_decays_between_grace_and_timeout() {
        let h = NetHealth::new();
        assert_eq!(h.snapshot(SILENCE_GRACE, 0).recv_health, 100);
        let mid = SILENCE_GRACE + (MAP_SILENCE_TIMEOUT - SILENCE_GRACE) / 2;
        let pct = h.snapshot(mid, 0).recv_health;
        assert!((48..=52).contains(&pct), "expected ~50%, got {pct}");
    }

    #[test]
    fn rate_first_sample_seeds_then_computes_bps() {
        let mut h = NetHealth::new();
        let t0 = Instant::now();
        h.sample_rates(t0, 0, 0);
        assert_eq!(h.snapshot(Duration::ZERO, 0).send_bps, 0);

        let t1 = t0 + RATE_REFRESH;
        h.sample_rates(t1, 600, 1800);
        let s = h.snapshot(Duration::ZERO, 0);
        assert_eq!(s.send_bps, 200);
        assert_eq!(s.recv_bps, 600);
    }

    #[test]
    fn rate_holds_until_refresh_interval_elapses() {
        let mut h = NetHealth::new();
        let t0 = Instant::now();
        h.sample_rates(t0, 0, 0);
        h.sample_rates(t0 + Duration::from_millis(500), 9000, 9000);
        assert_eq!(h.snapshot(Duration::ZERO, 0).recv_bps, 0);
        h.sample_rates(t0 + RATE_REFRESH, 9000, 9000);
        assert_eq!(h.snapshot(Duration::ZERO, 0).recv_bps, 3000);
    }
}
