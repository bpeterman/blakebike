use crate::domain::{DistanceSource, Telemetry};

const AIR_DENSITY_KG_M3: f64 = 1.225;
const DRAG_AREA_M2: f64 = 0.32;
const ROLLING_RESISTANCE: f64 = 0.004;
const DRIVETRAIN_EFFICIENCY: f64 = 0.97;
const GRAVITY_M_S2: f64 = 9.80665;
const MAX_SAMPLE_GAP_SECONDS: f64 = 3.0;

#[derive(Debug, Clone, Copy)]
pub struct DistancePoint {
    pub speed_mps: f64,
    pub distance_meters: f64,
}

#[derive(Debug, Clone, Default)]
pub struct DistanceEstimate {
    pub points: Vec<DistancePoint>,
    pub total_meters: f64,
    pub source: Option<DistanceSource>,
    pub max_speed_mps: Option<f64>,
}

/// Folds samples into a running distance total. The live ride and the saved
/// session both go through this, so the number on screen while riding and the
/// number stored with the session come from one implementation.
#[derive(Debug, Clone)]
pub struct DistanceAccumulator {
    mass_kg: f64,
    previous: Option<(i64, f64)>,
    meters: f64,
    used_trainer: bool,
    used_power: bool,
    max_speed_mps: Option<f64>,
}

impl DistanceAccumulator {
    pub fn new(total_weight_kg: f32) -> Self {
        Self {
            mass_kg: f64::from(total_weight_kg).clamp(33.0, 290.0),
            previous: None,
            meters: 0.0,
            used_trainer: false,
            used_power: false,
            max_speed_mps: None,
        }
    }

    /// Adds one sample and returns the speed it was credited with. Gaps longer
    /// than [`MAX_SAMPLE_GAP_SECONDS`] add no distance: the rider was away.
    pub fn push(&mut self, sample: &Telemetry) -> f64 {
        let (speed_mps, trainer_speed) = match sample
            .speed_kph
            .filter(|speed| speed.is_finite() && *speed >= 0.0)
        {
            Some(speed) => (f64::from(speed) / 3.6, true),
            None => (speed_from_power(sample.power_watts, self.mass_kg), false),
        };
        self.used_trainer |= trainer_speed;
        self.used_power |= !trainer_speed;
        self.max_speed_mps = Some(
            self.max_speed_mps
                .map_or(speed_mps, |current| current.max(speed_mps)),
        );
        if let Some((previous_timestamp, previous_speed)) = self.previous {
            let delta_seconds = (sample.timestamp_ms - previous_timestamp) as f64 / 1000.0;
            if delta_seconds > 0.0 && delta_seconds <= MAX_SAMPLE_GAP_SECONDS {
                self.meters += (previous_speed + speed_mps) * 0.5 * delta_seconds;
            }
        }
        self.previous = Some((sample.timestamp_ms, speed_mps));
        speed_mps
    }

    pub fn meters(&self) -> f64 {
        self.meters
    }

    pub fn source(&self) -> Option<DistanceSource> {
        match (self.used_trainer, self.used_power) {
            (true, true) => Some(DistanceSource::Mixed),
            (true, false) => Some(DistanceSource::Trainer),
            (false, true) => Some(DistanceSource::Power),
            (false, false) => None,
        }
    }

    pub fn max_speed_mps(&self) -> Option<f64> {
        self.max_speed_mps
    }
}

pub fn estimate_distance(samples: &[Telemetry], total_weight_kg: f32) -> DistanceEstimate {
    let mut accumulator = DistanceAccumulator::new(total_weight_kg);
    let points = samples
        .iter()
        .map(|sample| DistancePoint {
            speed_mps: accumulator.push(sample),
            distance_meters: accumulator.meters(),
        })
        .collect();
    DistanceEstimate {
        points,
        total_meters: accumulator.meters(),
        source: accumulator.source(),
        max_speed_mps: accumulator.max_speed_mps(),
    }
}

pub fn speed_from_power(power_watts: u16, total_weight_kg: f64) -> f64 {
    if power_watts == 0 {
        return 0.0;
    }
    let wheel_power = f64::from(power_watts) * DRIVETRAIN_EFFICIENCY;
    let rolling_force = ROLLING_RESISTANCE * total_weight_kg * GRAVITY_M_S2;
    let aerodynamic_factor = 0.5 * AIR_DENSITY_KG_M3 * DRAG_AREA_M2;
    let mut low: f64 = 0.0;
    let mut high: f64 = 40.0;
    for _ in 0..48 {
        let speed = (low + high) * 0.5;
        let required_power = aerodynamic_factor * speed.powi(3) + rolling_force * speed;
        if required_power < wheel_power {
            low = speed;
        } else {
            high = speed;
        }
    }
    (low + high) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(timestamp_ms: i64, power_watts: u16, speed_kph: Option<f32>) -> Telemetry {
        Telemetry {
            timestamp_ms,
            power_watts,
            cadence_rpm: None,
            speed_kph,
            heart_rate_bpm: None,
            target_power_watts: None,
        }
    }

    #[test]
    fn integrates_trainer_speed_with_the_trapezoidal_rule() {
        let estimate = estimate_distance(
            &[
                sample(0, 200, Some(36.0)),
                sample(1_000, 200, Some(36.0)),
                sample(2_000, 200, Some(36.0)),
            ],
            84.0,
        );
        assert!((estimate.total_meters - 20.0).abs() < 0.001);
        assert_eq!(estimate.source, Some(DistanceSource::Trainer));
    }

    #[test]
    fn power_fallback_produces_a_reasonable_flat_road_speed() {
        let speed_kph = speed_from_power(200, 84.0) * 3.6;
        assert!((28.0..=34.0).contains(&speed_kph), "{speed_kph}");
        let estimate = estimate_distance(&[sample(0, 200, None), sample(1_000, 200, None)], 84.0);
        assert!(estimate.total_meters > 7.0);
        assert_eq!(estimate.source, Some(DistanceSource::Power));
    }

    #[test]
    fn pause_sized_gaps_do_not_add_distance() {
        let estimate = estimate_distance(
            &[
                sample(0, 200, Some(36.0)),
                sample(1_000, 200, Some(36.0)),
                sample(61_000, 200, Some(36.0)),
                sample(62_000, 200, Some(36.0)),
            ],
            84.0,
        );
        assert!((estimate.total_meters - 20.0).abs() < 0.001);
    }

    #[test]
    fn reports_mixed_when_trainer_speed_appears_mid_ride() {
        let estimate = estimate_distance(
            &[sample(0, 200, None), sample(1_000, 200, Some(30.0))],
            84.0,
        );
        assert_eq!(estimate.source, Some(DistanceSource::Mixed));
    }
}
