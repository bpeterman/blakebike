//! Cadence from cumulative crank revolution data, as sent by power meters
//! (Cycling Power Measurement) and cadence sensors (CSC Measurement).
//!
//! Each packet carries a cumulative crank revolution count (u16, wraps) and
//! the time of the last crank event in 1/1024 s (u16, wraps every 64 s).
//! Cadence is the delta of revolutions over the delta of event time between
//! two packets whose event time differs. When the event time stops changing
//! the rider has stopped pedalling; after a short grace period we report 0.

const TICKS_PER_SECOND: f32 = 1_024.0;
/// How long the event time may stay unchanged before cadence reads 0.
pub const STALL_MS: i64 = 2_000;
/// Anything above this is a glitch (double-counted revolution, bad delta).
const MAX_PLAUSIBLE_RPM: f32 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    revolutions: u16,
    event_time: u16,
}

#[derive(Debug, Default, Clone)]
pub struct CrankCadence {
    last: Option<Sample>,
    last_rpm: f32,
    /// Wall-clock time of the last packet whose event time advanced.
    last_advance_ms: i64,
}

impl CrankCadence {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed one packet. Returns the cadence to report, or `None` until two
    /// packets have been seen.
    pub fn update(&mut self, revolutions: u16, event_time: u16, now_ms: i64) -> Option<f32> {
        let sample = Sample {
            revolutions,
            event_time,
        };
        let Some(last) = self.last else {
            self.last = Some(sample);
            self.last_advance_ms = now_ms;
            return None;
        };
        let delta_time = event_time.wrapping_sub(last.event_time);
        if delta_time == 0 {
            // No new crank event. Hold the last value briefly, then coast to 0.
            if now_ms - self.last_advance_ms >= STALL_MS {
                self.last_rpm = 0.0;
            }
            return Some(self.last_rpm);
        }
        let delta_revolutions = revolutions.wrapping_sub(last.revolutions);
        let rpm = f32::from(delta_revolutions) * 60.0 * TICKS_PER_SECOND / f32::from(delta_time);
        self.last = Some(sample);
        self.last_advance_ms = now_ms;
        if rpm > MAX_PLAUSIBLE_RPM {
            // Keep the sample as the new baseline but do not trust the value.
            return Some(self.last_rpm);
        }
        self.last_rpm = rpm;
        Some(rpm)
    }

    #[cfg(test)]
    pub fn last_rpm(&self) -> f32 {
        self.last_rpm
    }

    /// Last seen cumulative revolutions (0 before any packet). Used by the
    /// simulators to hold the counter still while "coasting".
    pub fn last_revolutions(&self) -> u16 {
        self.last.map(|sample| sample.revolutions).unwrap_or(0)
    }

    pub fn last_event_time(&self) -> u16 {
        self.last.map(|sample| sample.event_time).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn needs_two_packets() {
        let mut crank = CrankCadence::default();
        assert_eq!(crank.update(10, 1_000, 0), None);
        // One revolution in exactly one second (1024 ticks) = 60 rpm.
        assert!(close(crank.update(11, 2_024, 1_000).unwrap(), 60.0));
    }

    #[test]
    fn computes_ninety_rpm() {
        let mut crank = CrankCadence::default();
        crank.update(0, 0, 0);
        // 3 revolutions over 2 s: 90 rpm.
        let rpm = crank.update(3, 2_048, 2_000).unwrap();
        assert!(close(rpm, 90.0), "{rpm}");
    }

    #[test]
    fn handles_revolution_and_time_rollover() {
        let mut crank = CrankCadence::default();
        crank.update(65_534, 65_000, 0);
        // +2 revolutions, event time wraps: 65_000 -> 1_000 is 1_536 ticks (1.5 s).
        let rpm = crank.update(0, 1_000, 1_500).unwrap();
        assert!(close(rpm, 80.0), "{rpm}");
    }

    #[test]
    fn holds_then_coasts_to_zero() {
        let mut crank = CrankCadence::default();
        crank.update(0, 0, 0);
        crank.update(1, 1_024, 1_000);
        // Same event time: nothing new yet, hold 60.
        assert!(close(crank.update(1, 1_024, 2_000).unwrap(), 60.0));
        // Still stalled past the grace period: 0.
        assert!(close(crank.update(1, 1_024, 3_100).unwrap(), 0.0));
        // Pedalling resumes: measured from the last advancing sample.
        let rpm = crank.update(2, 1_024 + 1_024, 4_000).unwrap();
        assert!(close(rpm, 60.0), "{rpm}");
    }

    #[test]
    fn ignores_implausible_spikes() {
        let mut crank = CrankCadence::default();
        crank.update(0, 0, 0);
        crank.update(1, 1_024, 1_000); // 60 rpm
        // 10 revolutions in 100 ms would be 6000 rpm: keep 60.
        assert!(close(crank.update(11, 1_024 + 102, 1_100).unwrap(), 60.0));
        // But the baseline moved on, so the next honest sample is fine.
        assert!(close(
            crank.update(12, 1_024 + 102 + 1_024, 2_100).unwrap(),
            60.0
        ));
    }

    #[test]
    fn time_advancing_without_revolutions_is_zero() {
        let mut crank = CrankCadence::default();
        crank.update(5, 0, 0);
        assert!(close(crank.update(5, 2_048, 2_000).unwrap(), 0.0));
    }

    #[test]
    fn reset_forgets_history() {
        let mut crank = CrankCadence::default();
        crank.update(0, 0, 0);
        crank.update(1, 1_024, 1_000);
        crank.reset();
        assert_eq!(crank.update(50, 5_000, 5_000), None);
        assert_eq!(crank.last_rpm(), 0.0);
    }
}
