//! Trainer-versus-power-meter accuracy analysis over a ride's per-device
//! readings (`source_samples`).
//!
//! Neither device is treated as truth: everything is "A versus B", where A is
//! the trainer and B the power meter, and the difference is always `A − B`
//! (positive means the trainer reads higher). The steps, in order:
//!
//! 1. find the window both devices covered; too little and the report says so;
//! 2. estimate the lag between the streams by cross-correlation on a 250 ms
//!    grid and shift the meter's timestamps by it, so latency and the
//!    trainer's internal smoothing are not mistaken for an offset;
//! 3. resample both onto one 1 Hz grid;
//! 4. exclude coasting (either source under 30 W) and power steps (either
//!    source moving more than 50 W in a second), each with a 3 s guard on
//!    both sides: a flywheel's decay and a trainer's slower response to a step
//!    are smoothing, not inaccuracy, and would otherwise land as spurious
//!    differences in whichever power bin the transition passes through;
//! 5. compare at 1 s, 3 s and 30 s smoothing, bin the 3 s pairs by elapsed
//!    minute, by power level and by cadence, and keep thinned traces and
//!    Bland-Altman points for drawing.
//!
//! Every constant the analysis uses is a field of the report, so the shared
//! image can print the rules it followed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    devices::{DeviceRole, RideDevice, SourceSample},
    domain::SessionSummary,
};

/// Under this much overlap between the two streams nothing is reported.
pub const MIN_OVERLAP_SECONDS: u32 = 60;
/// The lag search covers ±this many seconds.
pub const LAG_SEARCH_SECONDS: f32 = 10.0;
const LAG_GRID_MS: i64 = 250;
/// A 1 Hz meter fills the 250 ms grid by holding each reading for at most 1 s.
const LAG_HOLD_CELLS: usize = 4;
/// Below this much variation there is nothing to correlate (steady ERG).
const LAG_MIN_SD_WATTS: f32 = 5.0;
const LAG_MIN_CORRELATION: f32 = 0.5;
/// A lag search needs at least this many overlapping cells (10 s).
const LAG_MIN_CELLS: usize = 40;
/// An empty grid second is interpolated between the readings around it when
/// they are no further apart than this (a 1 Hz device with arrival jitter).
const INTERPOLATE_MAX_GAP_MS: i64 = 1_500;
/// Either source under this is coasting and excluded.
pub const COAST_WATTS: f32 = 30.0;
/// Either source moving more than this between consecutive grid seconds is a
/// power step; the devices' different response times make the seconds around
/// it meaningless for accuracy.
pub const TRANSIENT_WATTS_PER_SECOND: f32 = 50.0;
/// Seconds excluded on each side of a coasting or step second.
pub const COAST_GUARD_SECONDS: u32 = 3;
pub const WINDOWS_SECONDS: [u32; 3] = [1, 3, 30];
/// The window the headline numbers, bins, traces and scatter use.
pub const HEADLINE_WINDOW_SECONDS: u32 = 3;
pub const MINUTE_BIN_MIN_PAIRS: u32 = 20;
pub const POWER_BIN_WATTS: f32 = 50.0;
pub const CADENCE_BIN_RPM: f32 = 10.0;
/// Power and cadence bins with fewer pairs than this are not shown.
pub const BIN_MIN_PAIRS: u32 = 30;
pub const WARMUP_MINUTES: u32 = 10;
const WARMUP_MIN_PAIRS: u32 = 60;
pub const MAX_TRACE_POINTS: usize = 900;
pub const MAX_SCATTER_POINTS: usize = 600;
pub const SIGN_CONVENTION: &str =
    "Difference is trainer minus power meter: positive means the trainer reads higher.";

/// The outcome for a ride that recorded both power devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PowerComparison {
    /// Both devices were recorded, but not for long enough at the same time.
    Insufficient(Box<InsufficientComparison>),
    Ready(Box<ReadyComparison>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsufficientComparison {
    pub session_id: Uuid,
    pub reason: String,
    pub min_overlap_seconds: u32,
    pub coverage: Coverage,
    pub a: ComparedSource,
    pub b: ComparedSource,
    pub sign_convention: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyComparison {
    pub session_id: Uuid,
    pub workout_name: String,
    pub started_at: DateTime<Utc>,
    pub elapsed_seconds: u32,
    /// The trainer.
    pub a: ComparedSource,
    /// The power meter.
    pub b: ComparedSource,
    pub sign_convention: String,
    pub coverage: Coverage,
    pub exclusion: Exclusion,
    pub lag: Lag,
    /// One entry per smoothing window, in `WINDOWS_SECONDS` order.
    pub windows: Vec<WindowStats>,
    pub headline_window_seconds: u32,
    /// Difference as a line over mean power: additive versus proportional.
    pub trend: Trend,
    pub warmup: Option<Warmup>,
    /// Per elapsed minute from ride start (`lower`/`upper` in minutes).
    pub by_minute: Vec<Bin>,
    pub minute_bin_min_pairs: u32,
    /// Per `POWER_BIN_WATTS` of mean power (`lower`/`upper` in watts).
    pub by_power: Vec<Bin>,
    pub power_bin_watts: f32,
    pub by_cadence: Option<CadenceBins>,
    pub cadence_bin_rpm: f32,
    pub bin_min_pairs: u32,
    /// Both headline-smoothed traces over elapsed time, thinned for drawing.
    pub traces: Vec<TracePoint>,
    /// `(mean, difference)` of the headline-smoothed pairs, thinned.
    pub bland_altman: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparedSource {
    pub role: DeviceRole,
    pub device: Option<RideDevice>,
    /// Readings with a power value.
    pub readings: u32,
    pub rate_hz: f32,
    /// Grid seconds in the overlap this source had a value for.
    pub seconds: u32,
    /// Over the compared seconds only, so both sources are averaged over the
    /// same moments.
    pub mean_watts: Option<f32>,
    pub max_watts: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub ride_seconds: u32,
    pub overlap_seconds: u32,
    /// Elapsed seconds from ride start where the overlap begins and ends.
    pub overlap_start_seconds: u32,
    pub overlap_end_seconds: u32,
    /// Seconds that made it into the 1 s comparison.
    pub compared_seconds: u32,
    /// Seconds dropped by the exclusion rules (guards included).
    pub excluded_seconds: u32,
    /// Of those, seconds dropped as coasting.
    pub coasting_seconds: u32,
    /// Of those, seconds dropped around a power step (and not also coasting).
    pub transient_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exclusion {
    pub min_watts: f32,
    pub transient_watts_per_second: f32,
    pub guard_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lag {
    /// Positive: the trainer trails the meter. Zero when not confident.
    pub seconds: f32,
    pub correlation: f32,
    pub confident: bool,
    pub search_seconds: f32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStats {
    pub window_seconds: u32,
    pub count: u32,
    pub mean_watts: f32,
    /// Ratio of sums, `Σa / Σb − 1`, so low-power pairs do not dominate.
    pub mean_percent: f32,
    pub sd_watts: f32,
    /// Bland-Altman limits of agreement: mean ± 1.96 sd.
    pub loa_low_watts: f32,
    pub loa_high_watts: f32,
    pub p95_abs_watts: f32,
    pub p95_abs_percent: f32,
    pub max_abs_watts: f32,
    pub max_abs_percent: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trend {
    pub intercept_watts: f32,
    /// Slope of the difference over mean power, as percent of power.
    pub slope_percent: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Warmup {
    pub minutes: u32,
    pub first_mean_percent: f32,
    pub rest_mean_percent: f32,
    pub first_count: u32,
    pub rest_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bin {
    pub lower: f32,
    pub upper: f32,
    pub count: u32,
    pub mean_watts: f32,
    pub mean_percent: f32,
    pub sd_watts: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadenceBins {
    /// Whose cadence the bins are on.
    pub source: DeviceRole,
    pub bins: Vec<Bin>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracePoint {
    pub elapsed_seconds: f32,
    pub a: Option<f32>,
    pub b: Option<f32>,
}

/// A device's power readings, sorted by time, with cadence where it had one.
struct Readings {
    role: DeviceRole,
    at_ms: Vec<i64>,
    watts: Vec<f32>,
    cadence: Vec<Option<f32>>,
}

impl Readings {
    fn collect(samples: &[SourceSample], role: DeviceRole) -> Self {
        let mut rows: Vec<(i64, f32, Option<f32>)> = samples
            .iter()
            .filter(|sample| sample.role == role)
            .filter_map(|sample| {
                sample
                    .power_watts
                    .map(|watts| (sample.timestamp_ms, f32::from(watts), sample.cadence_rpm))
            })
            .collect();
        rows.sort_by_key(|row| row.0);
        Self {
            role,
            at_ms: rows.iter().map(|row| row.0).collect(),
            watts: rows.iter().map(|row| row.1).collect(),
            cadence: rows.iter().map(|row| row.2).collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.at_ms.is_empty()
    }

    fn first_ms(&self) -> i64 {
        self.at_ms[0]
    }

    fn last_ms(&self) -> i64 {
        self.at_ms[self.at_ms.len() - 1]
    }

    fn rate_hz(&self) -> f32 {
        let span = self.last_ms() - self.first_ms();
        if span <= 0 || self.at_ms.len() < 2 {
            0.0
        } else {
            (self.at_ms.len() - 1) as f32 / (span as f32 / 1_000.0)
        }
    }

    fn shifted(&self, by_ms: i64) -> Self {
        Self {
            role: self.role,
            at_ms: self.at_ms.iter().map(|at| at + by_ms).collect(),
            watts: self.watts.clone(),
            cadence: self.cadence.clone(),
        }
    }

    /// Mean of the readings in each cell `(start + j·cell, start + (j+1)·cell]`.
    fn cell_means(&self, start_ms: i64, cell_ms: i64, cells: usize) -> Vec<Option<f32>> {
        let mut sums = vec![0.0_f32; cells];
        let mut counts = vec![0_u32; cells];
        for (at, watts) in self.at_ms.iter().zip(&self.watts) {
            let offset = at - start_ms;
            if offset < 0 {
                continue;
            }
            // `(start, start + cell]` is cell 0; a reading exactly at `start`
            // belongs to it too.
            let index = ((offset - 1).max(0) / cell_ms) as usize;
            if index >= cells {
                continue;
            }
            sums[index] += watts;
            counts[index] += 1;
        }
        sums.into_iter()
            .zip(counts)
            .map(|(sum, count)| (count > 0).then(|| sum / count as f32))
            .collect()
    }

    /// Power (and cadence, when both neighbours have it) linearly interpolated
    /// at `at_ms` between the readings either side of it, if they are no more
    /// than `INTERPOLATE_MAX_GAP_MS` apart.
    fn interpolate(&self, at_ms: i64) -> Option<(f32, Option<f32>)> {
        let next = self.at_ms.partition_point(|at| *at < at_ms);
        let previous = next.checked_sub(1)?;
        if next >= self.at_ms.len() {
            return None;
        }
        let span = self.at_ms[next] - self.at_ms[previous];
        if span > INTERPOLATE_MAX_GAP_MS {
            return None;
        }
        let weight = if span == 0 {
            1.0
        } else {
            (at_ms - self.at_ms[previous]) as f32 / span as f32
        };
        let between = |from: f32, to: f32| from + (to - from) * weight;
        let power = between(self.watts[previous], self.watts[next]);
        let cadence = match (self.cadence[previous], self.cadence[next]) {
            (Some(from), Some(to)) => Some(between(from, to)),
            _ => None,
        };
        Some((power, cadence))
    }

    /// Power and cadence on a 1 Hz grid: the mean of each second's readings,
    /// or, for an empty second, the value interpolated at its centre between
    /// the readings around it (arrival jitter on a 1 Hz device), otherwise a
    /// gap. No multi-second hold: a held value is a duplicate that would
    /// flatter the 1 s spread.
    fn resample_1hz(&self, start_ms: i64, seconds: usize) -> (Vec<Option<f32>>, Vec<Option<f32>>) {
        let mut power = self.cell_means(start_ms, 1_000, seconds);
        let mut cadence = vec![None; seconds];
        let mut cadence_sums = vec![(0.0_f32, 0_u32); seconds];
        for ((at, cadence_value), _) in self.at_ms.iter().zip(&self.cadence).zip(&self.watts) {
            let Some(value) = cadence_value else {
                continue;
            };
            let offset = at - start_ms;
            if offset < 0 {
                continue;
            }
            let index = ((offset - 1).max(0) / 1_000) as usize;
            if index < seconds {
                cadence_sums[index].0 += value;
                cadence_sums[index].1 += 1;
            }
        }
        for (index, (sum, count)) in cadence_sums.into_iter().enumerate() {
            if count > 0 {
                cadence[index] = Some(sum / count as f32);
            }
        }
        for index in 0..seconds {
            if power[index].is_some() {
                continue;
            }
            let centre = start_ms + index as i64 * 1_000 + 500;
            if let Some((watts, interpolated_cadence)) = self.interpolate(centre) {
                power[index] = Some(watts);
                if cadence[index].is_none() {
                    cadence[index] = interpolated_cadence;
                }
            }
        }
        (power, cadence)
    }
}

/// Analyse a ride's per-device readings. `None` when fewer than two power
/// devices were recorded, so single-source rides carry no report at all.
pub fn compare_session(
    summary: &SessionSummary,
    samples: &[SourceSample],
    devices: &[RideDevice],
) -> Option<PowerComparison> {
    let a = Readings::collect(samples, DeviceRole::Trainer);
    let b = Readings::collect(samples, DeviceRole::Power);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let device_for = |role| devices.iter().find(|device| device.role == role).cloned();
    Some(compare(
        summary,
        &a,
        &b,
        device_for(DeviceRole::Trainer),
        device_for(DeviceRole::Power),
    ))
}

fn compare(
    summary: &SessionSummary,
    a: &Readings,
    b: &Readings,
    device_a: Option<RideDevice>,
    device_b: Option<RideDevice>,
) -> PowerComparison {
    let ride_start_ms = summary.started_at.timestamp_millis();
    let elapsed_at = |at_ms: i64| ((at_ms - ride_start_ms).max(0) / 1_000) as u32;
    let overlap_start = a.first_ms().max(b.first_ms());
    let overlap_end = a.last_ms().min(b.last_ms());
    let overlap_seconds = ((overlap_end - overlap_start).max(0) / 1_000) as u32;
    let mut source_a = ComparedSource {
        role: a.role,
        device: device_a,
        readings: a.at_ms.len() as u32,
        rate_hz: a.rate_hz(),
        seconds: 0,
        mean_watts: None,
        max_watts: None,
    };
    let mut source_b = ComparedSource {
        role: b.role,
        device: device_b,
        readings: b.at_ms.len() as u32,
        rate_hz: b.rate_hz(),
        seconds: 0,
        mean_watts: None,
        max_watts: None,
    };
    let mut coverage = Coverage {
        ride_seconds: summary.elapsed_seconds,
        overlap_seconds,
        overlap_start_seconds: elapsed_at(overlap_start),
        overlap_end_seconds: elapsed_at(overlap_end),
        compared_seconds: 0,
        excluded_seconds: 0,
        coasting_seconds: 0,
        transient_seconds: 0,
    };
    if overlap_seconds < MIN_OVERLAP_SECONDS {
        return PowerComparison::Insufficient(Box::new(InsufficientComparison {
            session_id: summary.id,
            reason: format!(
                "Both devices reported at the same time for only {overlap_seconds} s; at least {MIN_OVERLAP_SECONDS} s are needed."
            ),
            min_overlap_seconds: MIN_OVERLAP_SECONDS,
            coverage,
            a: source_a,
            b: source_b,
            sign_convention: SIGN_CONVENTION.into(),
        }));
    }

    let lag = estimate_lag(a, b, overlap_start, overlap_end);
    let shift_ms = if lag.confident {
        (lag.seconds * 1_000.0).round() as i64
    } else {
        0
    };
    let b = b.shifted(shift_ms);
    let start = a.first_ms().max(b.first_ms());
    let end = a.last_ms().min(b.last_ms());
    let seconds = ((end - start).max(0) / 1_000) as usize;
    let (power_a, cadence_a) = a.resample_1hz(start, seconds);
    let (power_b, cadence_b) = b.resample_1hz(start, seconds);
    let elapsed: Vec<f32> = (0..seconds)
        .map(|index| ((start + index as i64 * 1_000 + 500) - ride_start_ms) as f32 / 1_000.0)
        .collect();

    // Coasting on either side and power steps on either side, each with the
    // guard around it; a second is compared when it is neither.
    let coasting: Vec<bool> = power_a
        .iter()
        .zip(&power_b)
        .map(|(a, b)| matches!((a, b), (Some(a), Some(b)) if *a < COAST_WATTS || *b < COAST_WATTS))
        .collect();
    let stepping: Vec<bool> = (0..seconds)
        .map(|index| index > 0 && (steps(&power_a, index) || steps(&power_b, index)))
        .collect();
    let guarded_coasting = with_guard(&coasting, COAST_GUARD_SECONDS as usize);
    let guarded_stepping = with_guard(&stepping, COAST_GUARD_SECONDS as usize);
    let present: Vec<bool> = power_a
        .iter()
        .zip(&power_b)
        .map(|(a, b)| a.is_some() && b.is_some())
        .collect();
    let compared: Vec<bool> = (0..seconds)
        .map(|index| present[index] && !guarded_coasting[index] && !guarded_stepping[index])
        .collect();
    coverage.compared_seconds = compared.iter().filter(|c| **c).count() as u32;
    coverage.coasting_seconds = (0..seconds)
        .filter(|index| present[*index] && guarded_coasting[*index])
        .count() as u32;
    coverage.transient_seconds = (0..seconds)
        .filter(|index| present[*index] && guarded_stepping[*index] && !guarded_coasting[*index])
        .count() as u32;
    coverage.excluded_seconds = coverage.coasting_seconds + coverage.transient_seconds;
    source_a.seconds = power_a.iter().flatten().count() as u32;
    source_b.seconds = power_b.iter().flatten().count() as u32;
    let (mean_a, max_a) = mean_and_max(&power_a, &compared);
    let (mean_b, max_b) = mean_and_max(&power_b, &compared);
    source_a.mean_watts = mean_a;
    source_a.max_watts = max_a;
    source_b.mean_watts = mean_b;
    source_b.max_watts = max_b;

    // Excluded seconds leave the series before smoothing, so a 30 s window
    // straddling a coast or a step averages only the compared seconds in it.
    let masked = |series: &[Option<f32>]| -> Vec<Option<f32>> {
        series
            .iter()
            .zip(&compared)
            .map(|(value, keep)| if *keep { *value } else { None })
            .collect()
    };
    let masked_a = masked(&power_a);
    let masked_b = masked(&power_b);
    let mut windows = Vec::new();
    let mut headline_pairs: Vec<Pair> = Vec::new();
    for window in WINDOWS_SECONDS {
        let smoothed_a = smooth(&masked_a, window as usize);
        let smoothed_b = smooth(&masked_b, window as usize);
        let pairs: Vec<Pair> = (0..seconds)
            .filter(|index| compared[*index])
            .filter_map(|index| {
                Some(Pair {
                    index,
                    a: smoothed_a[index]?,
                    b: smoothed_b[index]?,
                })
            })
            .collect();
        if window == HEADLINE_WINDOW_SECONDS {
            headline_pairs = pairs.clone();
        }
        windows.push(window_stats(window, &pairs));
    }
    // The traces keep every second, excluded ones included, so the overlay
    // shows the whole ride as ridden.
    let smoothed_a = smooth(&power_a, HEADLINE_WINDOW_SECONDS as usize);
    let smoothed_b = smooth(&power_b, HEADLINE_WINDOW_SECONDS as usize);

    let trend = trend(&headline_pairs);
    let by_minute = bins(
        &headline_pairs,
        |pair| Some(elapsed[pair.index] / 60.0),
        1.0,
        MINUTE_BIN_MIN_PAIRS,
    );
    let warmup = warmup(&headline_pairs, &elapsed);
    let by_power = bins(
        &headline_pairs,
        |pair| Some(pair.mean()),
        POWER_BIN_WATTS,
        BIN_MIN_PAIRS,
    );
    let by_cadence = cadence_bins(&headline_pairs, &cadence_a, &cadence_b, a.role, b.role);

    let traces = thin(
        (0..seconds)
            .map(|index| TracePoint {
                elapsed_seconds: elapsed[index],
                a: smoothed_a[index],
                b: smoothed_b[index],
            })
            .collect(),
        MAX_TRACE_POINTS,
    );
    let bland_altman = thin(
        headline_pairs
            .iter()
            .map(|pair| [pair.mean(), pair.diff()])
            .collect(),
        MAX_SCATTER_POINTS,
    );

    PowerComparison::Ready(Box::new(ReadyComparison {
        session_id: summary.id,
        workout_name: summary.workout_name.clone(),
        started_at: summary.started_at,
        elapsed_seconds: summary.elapsed_seconds,
        a: source_a,
        b: source_b,
        sign_convention: SIGN_CONVENTION.into(),
        coverage,
        exclusion: Exclusion {
            min_watts: COAST_WATTS,
            transient_watts_per_second: TRANSIENT_WATTS_PER_SECOND,
            guard_seconds: COAST_GUARD_SECONDS,
        },
        lag,
        windows,
        headline_window_seconds: HEADLINE_WINDOW_SECONDS,
        trend,
        warmup,
        by_minute,
        minute_bin_min_pairs: MINUTE_BIN_MIN_PAIRS,
        by_power,
        power_bin_watts: POWER_BIN_WATTS,
        by_cadence,
        cadence_bin_rpm: CADENCE_BIN_RPM,
        bin_min_pairs: BIN_MIN_PAIRS,
        traces,
        bland_altman,
    }))
}

#[derive(Debug, Clone, Copy)]
struct Pair {
    index: usize,
    a: f32,
    b: f32,
}

impl Pair {
    fn diff(&self) -> f32 {
        self.a - self.b
    }

    fn mean(&self) -> f32 {
        (self.a + self.b) / 2.0
    }

    fn percent(&self) -> f32 {
        let mean = self.mean();
        if mean.abs() < f32::EPSILON {
            0.0
        } else {
            self.diff() / mean * 100.0
        }
    }
}

/// Cross-correlate the two streams on a 250 ms grid over ±`LAG_SEARCH_SECONDS`
/// and refine the peak to a fraction of a cell. Positive means the trainer
/// trails the meter (`A(t) ≈ B(t − lag)`).
fn estimate_lag(a: &Readings, b: &Readings, start_ms: i64, end_ms: i64) -> Lag {
    let cells = ((end_ms - start_ms) / LAG_GRID_MS).max(0) as usize;
    let grid_a = hold(a.cell_means(start_ms, LAG_GRID_MS, cells), LAG_HOLD_CELLS);
    let grid_b = hold(b.cell_means(start_ms, LAG_GRID_MS, cells), LAG_HOLD_CELLS);
    let search = (LAG_SEARCH_SECONDS * 1_000.0 / LAG_GRID_MS as f32).round() as i64;
    let unconfident = |note: &str| Lag {
        seconds: 0.0,
        correlation: 0.0,
        confident: false,
        search_seconds: LAG_SEARCH_SECONDS,
        note: note.into(),
    };
    let sd_a = standard_deviation(grid_a.iter().flatten().copied());
    let sd_b = standard_deviation(grid_b.iter().flatten().copied());
    if sd_a < LAG_MIN_SD_WATTS || sd_b < LAG_MIN_SD_WATTS {
        return unconfident(
            "Power barely varied, so the lag between the devices could not be estimated; assumed 0 s.",
        );
    }
    let correlations: Vec<Option<f32>> = (-search..=search)
        .map(|lag| correlation_at(&grid_a, &grid_b, lag))
        .collect();
    let Some((peak_index, peak)) = correlations
        .iter()
        .enumerate()
        .filter_map(|(index, r)| r.map(|r| (index, r)))
        .max_by(|(_, x), (_, y)| x.total_cmp(y))
    else {
        return unconfident("Too little overlapping data to estimate the lag; assumed 0 s.");
    };
    if peak < LAG_MIN_CORRELATION {
        return Lag {
            correlation: peak,
            ..unconfident(
                "The two streams did not correlate well enough to estimate the lag; assumed 0 s.",
            )
        };
    }
    if peak_index == 0 || peak_index == correlations.len() - 1 {
        return Lag {
            correlation: peak,
            ..unconfident(
                "The best alignment sat at the edge of the search range, so the lag was not trusted; assumed 0 s.",
            )
        };
    }
    // Parabolic refinement through the three cells around the peak.
    let below = correlations[peak_index - 1].unwrap_or(peak);
    let above = correlations[peak_index + 1].unwrap_or(peak);
    let denominator = below - 2.0 * peak + above;
    let fraction = if denominator < 0.0 {
        (0.5 * (below - above) / denominator).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let lag_cells = (peak_index as i64 - search) as f32 + fraction;
    let seconds = lag_cells * LAG_GRID_MS as f32 / 1_000.0;
    let note = if seconds.abs() < 0.125 {
        "The two streams were already aligned.".to_string()
    } else if seconds > 0.0 {
        format!(
            "The trainer trails the power meter by {seconds:.2} s; the meter was shifted to match before comparing."
        )
    } else {
        format!(
            "The power meter trails the trainer by {:.2} s; the meter was shifted to match before comparing.",
            -seconds
        )
    };
    Lag {
        seconds,
        correlation: peak,
        confident: true,
        search_seconds: LAG_SEARCH_SECONDS,
        note,
    }
}

/// Pearson correlation of `a[j]` with `b[j − lag]` over the cells both have.
fn correlation_at(a: &[Option<f32>], b: &[Option<f32>], lag: i64) -> Option<f32> {
    let pairs: Vec<(f32, f32)> = (0..a.len())
        .filter_map(|j| {
            let k = j as i64 - lag;
            if k < 0 || k as usize >= b.len() {
                return None;
            }
            Some((a[j]?, b[k as usize]?))
        })
        .collect();
    if pairs.len() < LAG_MIN_CELLS {
        return None;
    }
    let n = pairs.len() as f32;
    let mean_a = pairs.iter().map(|(a, _)| a).sum::<f32>() / n;
    let mean_b = pairs.iter().map(|(_, b)| b).sum::<f32>() / n;
    let (mut cov, mut var_a, mut var_b) = (0.0_f32, 0.0_f32, 0.0_f32);
    for (a, b) in &pairs {
        let da = a - mean_a;
        let db = b - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    if var_a <= f32::EPSILON || var_b <= f32::EPSILON {
        return None;
    }
    Some(cov / (var_a.sqrt() * var_b.sqrt()))
}

/// Whether a series moved by more than `TRANSIENT_WATTS_PER_SECOND` into
/// grid second `index`.
fn steps(series: &[Option<f32>], index: usize) -> bool {
    matches!(
        (series[index - 1], series[index]),
        (Some(previous), Some(current)) if (current - previous).abs() > TRANSIENT_WATTS_PER_SECOND
    )
}

/// Mark `guard` seconds on each side of every marked second.
fn with_guard(marked: &[bool], guard: usize) -> Vec<bool> {
    (0..marked.len())
        .map(|index| {
            let from = index.saturating_sub(guard);
            let to = (index + guard).min(marked.len().saturating_sub(1));
            marked[from..=to].iter().any(|flag| *flag)
        })
        .collect()
}

/// Fill empty cells from the previous value for at most `max_cells`.
fn hold(mut cells: Vec<Option<f32>>, max_cells: usize) -> Vec<Option<f32>> {
    let mut last: Option<(f32, usize)> = None;
    for (index, cell) in cells.iter_mut().enumerate() {
        match *cell {
            Some(value) => last = Some((value, index)),
            None => {
                if let Some((value, at)) = last
                    && index - at <= max_cells
                {
                    *cell = Some(value);
                }
            }
        }
    }
    cells
}

/// Centered moving average needing at least half of the window (as much of
/// it as fits at the edges) present. A window of 1 is the series itself.
fn smooth(series: &[Option<f32>], window: usize) -> Vec<Option<f32>> {
    if window <= 1 {
        return series.to_vec();
    }
    let half = window / 2;
    (0..series.len())
        .map(|index| {
            let from = index.saturating_sub(half);
            let to = (index + half + 1 - window.is_multiple_of(2) as usize).min(series.len());
            let needed = (to - from).div_ceil(2);
            let values: Vec<f32> = series[from..to].iter().flatten().copied().collect();
            (values.len() >= needed).then(|| values.iter().sum::<f32>() / values.len() as f32)
        })
        .collect()
}

fn window_stats(window: u32, pairs: &[Pair]) -> WindowStats {
    let count = pairs.len() as u32;
    if pairs.is_empty() {
        return WindowStats {
            window_seconds: window,
            count,
            mean_watts: 0.0,
            mean_percent: 0.0,
            sd_watts: 0.0,
            loa_low_watts: 0.0,
            loa_high_watts: 0.0,
            p95_abs_watts: 0.0,
            p95_abs_percent: 0.0,
            max_abs_watts: 0.0,
            max_abs_percent: 0.0,
        };
    }
    let diffs: Vec<f32> = pairs.iter().map(Pair::diff).collect();
    let mean_watts = diffs.iter().sum::<f32>() / count as f32;
    let sd_watts = standard_deviation(diffs.iter().copied());
    let mut abs_watts: Vec<f32> = diffs.iter().map(|d| d.abs()).collect();
    let mut abs_percent: Vec<f32> = pairs.iter().map(|pair| pair.percent().abs()).collect();
    abs_watts.sort_by(f32::total_cmp);
    abs_percent.sort_by(f32::total_cmp);
    WindowStats {
        window_seconds: window,
        count,
        mean_watts,
        mean_percent: ratio_of_sums_percent(pairs),
        sd_watts,
        loa_low_watts: mean_watts - 1.96 * sd_watts,
        loa_high_watts: mean_watts + 1.96 * sd_watts,
        p95_abs_watts: percentile(&abs_watts, 0.95),
        p95_abs_percent: percentile(&abs_percent, 0.95),
        max_abs_watts: abs_watts[abs_watts.len() - 1],
        max_abs_percent: abs_percent[abs_percent.len() - 1],
    }
}

fn ratio_of_sums_percent(pairs: &[Pair]) -> f32 {
    let sum_a: f32 = pairs.iter().map(|pair| pair.a).sum();
    let sum_b: f32 = pairs.iter().map(|pair| pair.b).sum();
    if sum_b.abs() < f32::EPSILON {
        0.0
    } else {
        (sum_a / sum_b - 1.0) * 100.0
    }
}

/// Least squares of the difference over the mean: a flat offset shows as the
/// intercept, a scale error as the slope.
fn trend(pairs: &[Pair]) -> Trend {
    if pairs.is_empty() {
        return Trend {
            intercept_watts: 0.0,
            slope_percent: 0.0,
        };
    }
    let n = pairs.len() as f32;
    let mean_x = pairs.iter().map(Pair::mean).sum::<f32>() / n;
    let mean_y = pairs.iter().map(Pair::diff).sum::<f32>() / n;
    let (mut cov, mut var) = (0.0_f32, 0.0_f32);
    for pair in pairs {
        let dx = pair.mean() - mean_x;
        cov += dx * (pair.diff() - mean_y);
        var += dx * dx;
    }
    let slope = if var <= f32::EPSILON { 0.0 } else { cov / var };
    Trend {
        intercept_watts: mean_y - slope * mean_x,
        slope_percent: slope * 100.0,
    }
}

fn warmup(pairs: &[Pair], elapsed: &[f32]) -> Option<Warmup> {
    let boundary = WARMUP_MINUTES as f32 * 60.0;
    let (first, rest): (Vec<Pair>, Vec<Pair>) = pairs
        .iter()
        .partition(|pair| elapsed[pair.index] < boundary);
    if (first.len() as u32) < WARMUP_MIN_PAIRS || (rest.len() as u32) < WARMUP_MIN_PAIRS {
        return None;
    }
    Some(Warmup {
        minutes: WARMUP_MINUTES,
        first_mean_percent: ratio_of_sums_percent(&first),
        rest_mean_percent: ratio_of_sums_percent(&rest),
        first_count: first.len() as u32,
        rest_count: rest.len() as u32,
    })
}

/// Group pairs by `key / width` and summarize each group with at least
/// `min_pairs` members, in ascending order.
fn bins(
    pairs: &[Pair],
    key: impl Fn(&Pair) -> Option<f32>,
    width: f32,
    min_pairs: u32,
) -> Vec<Bin> {
    let mut groups: std::collections::BTreeMap<i64, Vec<Pair>> = Default::default();
    for pair in pairs {
        let Some(value) = key(pair) else {
            continue;
        };
        if !value.is_finite() || value < 0.0 {
            continue;
        }
        groups
            .entry((value / width).floor() as i64)
            .or_default()
            .push(*pair);
    }
    groups
        .into_iter()
        .filter(|(_, members)| members.len() as u32 >= min_pairs)
        .map(|(slot, members)| {
            let diffs: Vec<f32> = members.iter().map(Pair::diff).collect();
            Bin {
                lower: slot as f32 * width,
                upper: (slot + 1) as f32 * width,
                count: members.len() as u32,
                mean_watts: diffs.iter().sum::<f32>() / members.len() as f32,
                mean_percent: ratio_of_sums_percent(&members),
                sd_watts: standard_deviation(diffs.iter().copied()),
            }
        })
        .collect()
}

/// Cadence bins on the meter's cadence when it covers at least half the
/// pairs, else the trainer's, else none.
fn cadence_bins(
    pairs: &[Pair],
    cadence_a: &[Option<f32>],
    cadence_b: &[Option<f32>],
    role_a: DeviceRole,
    role_b: DeviceRole,
) -> Option<CadenceBins> {
    if pairs.is_empty() {
        return None;
    }
    let coverage = |cadence: &[Option<f32>]| {
        pairs
            .iter()
            .filter(|pair| cadence[pair.index].is_some_and(|c| c > 0.0))
            .count()
            * 2
            >= pairs.len()
    };
    let (source, cadence) = if coverage(cadence_b) {
        (role_b, cadence_b)
    } else if coverage(cadence_a) {
        (role_a, cadence_a)
    } else {
        return None;
    };
    let bins = bins(
        pairs,
        |pair| cadence[pair.index].filter(|c| *c > 0.0),
        CADENCE_BIN_RPM,
        BIN_MIN_PAIRS,
    );
    (!bins.is_empty()).then_some(CadenceBins { source, bins })
}

fn mean_and_max(series: &[Option<f32>], mask: &[bool]) -> (Option<f32>, Option<f32>) {
    let values: Vec<f32> = series
        .iter()
        .zip(mask)
        .filter_map(|(value, keep)| if *keep { *value } else { None })
        .collect();
    if values.is_empty() {
        return (None, None);
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    (Some(mean), Some(max))
}

/// Sample standard deviation; zero for fewer than two values.
fn standard_deviation(values: impl Iterator<Item = f32>) -> f32 {
    let values: Vec<f32> = values.collect();
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let variance =
        values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / (values.len() - 1) as f32;
    variance.sqrt()
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((fraction * sorted.len() as f32).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Keep at most `maximum` evenly spaced points, always the first and last.
fn thin<T: Clone>(points: Vec<T>, maximum: usize) -> Vec<T> {
    if points.len() <= maximum || maximum < 2 {
        return points;
    }
    let step = (points.len() - 1) as f32 / (maximum - 1) as f32;
    (0..maximum)
        .map(|index| points[((index as f32 * step).round() as usize).min(points.len() - 1)].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIDE_START_MS: i64 = 1_700_000_000_000;

    /// Deterministic noise without a dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
        }
    }

    /// A 25-minute ride with steady blocks, a ramp, intervals, sprints and
    /// coasting, so every bin and the lag search have something to work on.
    fn profile(t: f32) -> f32 {
        let wiggle = 8.0 * (t / 23.0 * std::f32::consts::TAU).sin();
        match t {
            t if t < 120.0 => 150.0 + wiggle,
            t if t < 300.0 => 240.0 + wiggle,
            t if t < 420.0 => 150.0 + (t - 300.0) / 120.0 * 180.0,
            t if t < 428.0 => 0.0,
            t if t < 720.0 => {
                if (((t - 428.0) / 30.0) as u32).is_multiple_of(2) {
                    320.0 + wiggle
                } else {
                    160.0 + wiggle
                }
            }
            t if t < 730.0 => 450.0, // a 10 s spike: too sparse for a bin
            t => {
                let coasting = ((t - 730.0) % 200.0) < 5.0;
                if coasting { 0.0 } else { 210.0 + wiggle }
            }
        }
    }

    struct Synthetic {
        duration_s: f32,
        lag_s: f32,
        offset_w: f32,
        scale: fn(f32) -> f32,
        noise_w: f32,
        meter_start_s: f32,
        meter_jitter_s: f32,
        meter_cadence: bool,
        profile: fn(f32) -> f32,
    }

    impl Default for Synthetic {
        fn default() -> Self {
            Self {
                duration_s: 1_500.0,
                lag_s: 0.0,
                offset_w: 0.0,
                scale: |_| 1.0,
                noise_w: 2.0,
                meter_start_s: 0.0,
                meter_jitter_s: 0.05,
                meter_cadence: true,
                profile,
            }
        }
    }

    impl Synthetic {
        /// Meter at 1 Hz; trainer at 4 Hz reading `profile(t − lag) × scale(t)
        /// + offset`, lightly smoothed like a flywheel-based estimate.
        fn build(&self) -> (Vec<SourceSample>, Vec<SourceSample>) {
            let mut noise = Lcg(7);
            let mut meter = Vec::new();
            let mut t = self.meter_start_s;
            while t < self.duration_s {
                let jitter = noise.next() * self.meter_jitter_s;
                let watts = (self.profile)(t).max(0.0);
                meter.push(SourceSample {
                    role: DeviceRole::Power,
                    timestamp_ms: RIDE_START_MS + ((t + jitter) * 1_000.0).round() as i64,
                    power_watts: Some(watts.round() as u16),
                    cadence_rpm: self
                        .meter_cadence
                        .then_some(if watts > 0.0 { 90.0 } else { 0.0 }),
                    balance_left_percent: Some(50.0),
                });
                t += 1.0;
            }
            let mut trainer = Vec::new();
            let mut smoothed: Option<f32> = None;
            let mut t = 0.0_f32;
            while t < self.duration_s {
                let raw = (self.profile)(t - self.lag_s).max(0.0);
                let value = if raw > 0.0 {
                    raw * (self.scale)(t) + self.offset_w
                } else {
                    0.0
                };
                let level = match smoothed {
                    Some(previous) => previous + 0.7 * (value - previous),
                    None => value,
                };
                smoothed = Some(level);
                let watts = (level + noise.next() * self.noise_w).max(0.0);
                trainer.push(SourceSample {
                    role: DeviceRole::Trainer,
                    timestamp_ms: RIDE_START_MS + (t * 1_000.0).round() as i64,
                    power_watts: Some(watts.round() as u16),
                    cadence_rpm: Some(if watts > 0.0 { 88.0 } else { 0.0 }),
                    balance_left_percent: None,
                });
                t += 0.25;
            }
            (trainer, meter)
        }

        fn compare(&self) -> PowerComparison {
            let (trainer, meter) = self.build();
            let samples: Vec<SourceSample> = trainer.into_iter().chain(meter).collect();
            compare_session(&summary(self.duration_s as u32), &samples, &devices()).unwrap()
        }

        fn ready(&self) -> ReadyComparison {
            match self.compare() {
                PowerComparison::Ready(ready) => *ready,
                other => panic!("expected a ready comparison, got {other:?}"),
            }
        }
    }

    fn summary(elapsed_seconds: u32) -> SessionSummary {
        SessionSummary {
            id: Uuid::new_v4(),
            workout_id: None,
            workout_name: "Dual".into(),
            started_at: DateTime::<Utc>::from_timestamp_millis(RIDE_START_MS).unwrap(),
            ended_at: None,
            elapsed_seconds,
            average_power_watts: 0,
            max_power_watts: 0,
            average_cadence_rpm: None,
            estimated_distance_meters: 0.0,
            distance_source: None,
            distance_weight_kg: 84.0,
            completed: true,
            recording_warning: None,
        }
    }

    fn devices() -> Vec<RideDevice> {
        vec![
            RideDevice {
                role: DeviceRole::Trainer,
                id: "kickr".into(),
                name: "KICKR CORE".into(),
                transport: Default::default(),
                simulated: false,
                manufacturer: Some("Wahoo".into()),
                model: None,
                firmware: Some("1.2".into()),
                last_calibration: None,
            },
            RideDevice {
                role: DeviceRole::Power,
                id: "pm".into(),
                name: "Assioma DUO".into(),
                transport: Default::default(),
                simulated: false,
                manufacturer: Some("Favero".into()),
                model: None,
                firmware: Some("5.14".into()),
                last_calibration: None,
            },
        ]
    }

    fn headline(ready: &ReadyComparison) -> &WindowStats {
        ready
            .windows
            .iter()
            .find(|window| window.window_seconds == HEADLINE_WINDOW_SECONDS)
            .unwrap()
    }

    #[test]
    fn a_ride_without_two_power_sources_has_no_comparison() {
        let (trainer, _) = Synthetic::default().build();
        assert!(compare_session(&summary(100), &trainer, &devices()).is_none());
        assert!(compare_session(&summary(100), &[], &devices()).is_none());
        // A meter whose frames never carried power does not count either.
        let mut samples = trainer;
        samples.push(SourceSample {
            role: DeviceRole::Power,
            timestamp_ms: RIDE_START_MS,
            power_watts: None,
            cadence_rpm: Some(90.0),
            balance_left_percent: None,
        });
        assert!(compare_session(&summary(100), &samples, &devices()).is_none());
    }

    #[test]
    fn recovers_a_known_lag_and_flat_offset() {
        let ready = Synthetic {
            lag_s: 2.0,
            offset_w: 5.0,
            ..Synthetic::default()
        }
        .ready();
        assert!(ready.lag.confident, "{:?}", ready.lag);
        assert!(
            (ready.lag.seconds - 2.0).abs() < 0.4,
            "lag {}",
            ready.lag.seconds
        );
        assert!(ready.lag.note.contains("trainer trails"));
        let stats = headline(&ready);
        assert!(
            (stats.mean_watts - 5.0).abs() < 1.5,
            "mean {}",
            stats.mean_watts
        );
        assert!(stats.count > 1_000, "count {}", stats.count);
        assert!(
            ready.trend.slope_percent.abs() < 1.0,
            "slope {}",
            ready.trend.slope_percent
        );
        assert!(
            (ready.trend.intercept_watts - 5.0).abs() < 3.0,
            "intercept {}",
            ready.trend.intercept_watts
        );
        // The 1 s window is noisier than the 30 s window, never the other way.
        assert!(ready.windows[0].sd_watts >= ready.windows[2].sd_watts);
        assert_eq!(ready.a.device.as_ref().unwrap().name, "KICKR CORE");
        assert_eq!(
            ready.b.device.as_ref().unwrap().firmware.as_deref(),
            Some("5.14")
        );
        assert_eq!(ready.sign_convention, SIGN_CONVENTION);
        assert!(ready.a.rate_hz > 3.5 && ready.b.rate_hz < 1.2);
    }

    #[test]
    fn recovers_a_sub_second_lag() {
        let ready = Synthetic {
            lag_s: 0.75,
            ..Synthetic::default()
        }
        .ready();
        assert!(ready.lag.confident);
        assert!(
            (ready.lag.seconds - 0.75).abs() < 0.35,
            "lag {}",
            ready.lag.seconds
        );
        // Aligned, the flat streams agree to within the noise.
        assert!(headline(&ready).mean_watts.abs() < 1.5);
    }

    #[test]
    fn a_meter_that_trails_the_trainer_gets_a_negative_lag() {
        let ready = Synthetic {
            lag_s: -1.5,
            ..Synthetic::default()
        }
        .ready();
        assert!(ready.lag.confident);
        assert!(
            (ready.lag.seconds + 1.5).abs() < 0.4,
            "lag {}",
            ready.lag.seconds
        );
        assert!(ready.lag.note.contains("power meter trails"));
    }

    #[test]
    fn a_scale_error_shows_as_percent_and_slope_not_offset() {
        let ready = Synthetic {
            scale: |_| 1.03,
            ..Synthetic::default()
        }
        .ready();
        let stats = headline(&ready);
        assert!(
            (stats.mean_percent - 3.0).abs() < 0.7,
            "percent {}",
            stats.mean_percent
        );
        assert!(
            (ready.trend.slope_percent - 3.0).abs() < 1.0,
            "slope {}",
            ready.trend.slope_percent
        );
        assert!(
            ready.trend.intercept_watts.abs() < 4.0,
            "intercept {}",
            ready.trend.intercept_watts
        );
        // Percent bins follow the same 3% everywhere power was ridden.
        for bin in &ready.by_power {
            assert!((bin.mean_percent - 3.0).abs() < 1.5, "{bin:?}");
        }
    }

    #[test]
    fn warm_up_drift_is_visible_per_minute_and_summarized() {
        let ready = Synthetic {
            scale: |t| if t < 600.0 { 1.03 } else { 1.0 },
            ..Synthetic::default()
        }
        .ready();
        let warmup = ready.warmup.as_ref().expect("enough pairs on both sides");
        assert_eq!(warmup.minutes, WARMUP_MINUTES);
        assert!((warmup.first_mean_percent - 3.0).abs() < 0.7, "{warmup:?}");
        assert!(warmup.rest_mean_percent.abs() < 0.7, "{warmup:?}");
        let early: Vec<&Bin> = ready
            .by_minute
            .iter()
            .filter(|bin| bin.upper <= 10.0)
            .collect();
        let late: Vec<&Bin> = ready
            .by_minute
            .iter()
            .filter(|bin| bin.lower >= 11.0)
            .collect();
        assert!(
            early.len() >= 8 && late.len() >= 10,
            "{:?}",
            ready.by_minute
        );
        assert!(early.iter().all(|bin| bin.mean_percent > 1.5), "{early:?}");
        assert!(
            late.iter().all(|bin| bin.mean_percent.abs() < 1.2),
            "{late:?}"
        );
        assert!(
            ready
                .by_minute
                .iter()
                .all(|bin| bin.count >= MINUTE_BIN_MIN_PAIRS)
        );
    }

    #[test]
    fn coasting_is_excluded_with_a_guard_on_each_side() {
        // Both devices flat at 200 W for five minutes, one second of coasting.
        let flat: fn(f32) -> f32 = |t| {
            if (150.0..151.0).contains(&t) {
                0.0
            } else {
                200.0
            }
        };
        let ready = Synthetic {
            duration_s: 300.0,
            profile: flat,
            noise_w: 0.0,
            ..Synthetic::default()
        }
        .ready();
        // The one dip is enough to align on, and it sits at 0 s.
        assert!(ready.lag.seconds.abs() < 0.3, "{:?}", ready.lag);
        let expected_excluded = 1 + 2 * COAST_GUARD_SECONDS;
        assert_eq!(ready.coverage.coasting_seconds, expected_excluded);
        // The trainer's decay into the dip is a step of its own a second
        // before the coast; its guard reaches at most one second past the
        // coasting guard on each side.
        assert!(
            ready.coverage.transient_seconds <= 2,
            "{:?}",
            ready.coverage
        );
        assert_eq!(
            ready.coverage.excluded_seconds,
            expected_excluded + ready.coverage.transient_seconds
        );
        assert_eq!(
            ready.coverage.compared_seconds + ready.coverage.excluded_seconds,
            ready.coverage.overlap_seconds
        );
        assert_eq!(ready.exclusion.min_watts, COAST_WATTS);
        assert_eq!(ready.exclusion.guard_seconds, COAST_GUARD_SECONDS);
        assert_eq!(headline(&ready).mean_watts, 0.0);
    }

    #[test]
    fn power_steps_are_excluded_with_a_guard_on_each_side() {
        // One 200 → 300 W step at 150 s, nothing else.
        let step: fn(f32) -> f32 = |t| if t < 150.0 { 200.0 } else { 300.0 };
        let ready = Synthetic {
            duration_s: 300.0,
            profile: step,
            noise_w: 0.0,
            ..Synthetic::default()
        }
        .ready();
        assert_eq!(ready.coverage.coasting_seconds, 0);
        assert_eq!(
            ready.exclusion.transient_watts_per_second,
            TRANSIENT_WATTS_PER_SECOND
        );
        // The step second itself plus the guard; the trainer's own slower
        // rise may mark one more.
        let transient = ready.coverage.transient_seconds;
        assert!((7..=9).contains(&transient), "transient {transient}");
        assert_eq!(ready.coverage.excluded_seconds, transient);
        // With the step gone, two devices that agree everywhere else agree.
        assert!(
            headline(&ready).max_abs_watts < 2.0,
            "{:?}",
            headline(&ready)
        );
    }

    #[test]
    fn sparse_bins_are_suppressed_and_counts_are_reported() {
        let ready = Synthetic::default().ready();
        assert!(!ready.by_power.is_empty());
        assert!(ready.by_power.iter().all(|bin| bin.count >= BIN_MIN_PAIRS));
        // The 10 s spike at 450 W never earns a bin.
        assert!(
            ready.by_power.iter().all(|bin| bin.lower < 400.0),
            "{:?}",
            ready.by_power
        );
        // The long blocks do.
        assert!(ready.by_power.iter().any(|bin| bin.lower == 150.0));
        assert!(ready.by_power.iter().any(|bin| bin.lower == 300.0));
        assert_eq!(ready.power_bin_watts, POWER_BIN_WATTS);
        assert_eq!(ready.bin_min_pairs, BIN_MIN_PAIRS);
    }

    #[test]
    fn partial_overlap_is_analysed_and_reported() {
        let ready = Synthetic {
            meter_start_s: 300.0,
            ..Synthetic::default()
        }
        .ready();
        assert!(
            (ready.coverage.overlap_start_seconds as i64 - 300).abs() <= 2,
            "{:?}",
            ready.coverage
        );
        assert!(ready.coverage.overlap_end_seconds >= 1_495);
        assert_eq!(ready.coverage.ride_seconds, 1_500);
        assert!(ready.a.readings > ready.b.readings);
        assert!(ready.coverage.compared_seconds > 1_000);
        // Nothing before the meter arrived is in the drift chart.
        assert!(ready.by_minute.iter().all(|bin| bin.lower >= 5.0));
        assert!(
            ready
                .traces
                .iter()
                .all(|point| point.elapsed_seconds >= 299.0)
        );
    }

    #[test]
    fn too_little_overlap_is_insufficient_not_an_error() {
        let comparison = Synthetic {
            duration_s: 330.0,
            meter_start_s: 300.0,
            ..Synthetic::default()
        }
        .compare();
        let PowerComparison::Insufficient(report) = comparison else {
            panic!("expected insufficient, got {comparison:?}");
        };
        let report = *report;
        assert!((report.coverage.overlap_seconds as i64 - 30).abs() <= 1);
        assert_eq!(report.min_overlap_seconds, MIN_OVERLAP_SECONDS);
        assert!(report.reason.contains("60 s"));
        assert_eq!(report.b.device.as_ref().unwrap().name, "Assioma DUO");
        let json = serde_json::to_value(PowerComparison::Insufficient(Box::new(report))).unwrap();
        assert_eq!(json["status"], "insufficient");
        assert!(json["reason"].is_string());
    }

    #[test]
    fn steady_power_leaves_the_lag_unestimated() {
        let ready = Synthetic {
            duration_s: 300.0,
            profile: |_| 200.0,
            lag_s: 2.0,
            ..Synthetic::default()
        }
        .ready();
        assert!(!ready.lag.confident);
        assert_eq!(ready.lag.seconds, 0.0);
        assert!(ready.lag.note.contains("could not be estimated"));
        assert_eq!(ready.lag.search_seconds, LAG_SEARCH_SECONDS);
    }

    #[test]
    fn cadence_bins_prefer_the_meter_and_fall_back_to_the_trainer() {
        let ready = Synthetic::default().ready();
        let cadence = ready
            .by_cadence
            .as_ref()
            .expect("meter cadence covers the ride");
        assert_eq!(cadence.source, DeviceRole::Power);
        assert!(cadence.bins.iter().any(|bin| bin.lower == 90.0));
        assert!(cadence.bins.iter().all(|bin| bin.count >= BIN_MIN_PAIRS));

        let ready = Synthetic {
            meter_cadence: false,
            ..Synthetic::default()
        }
        .ready();
        let cadence = ready.by_cadence.as_ref().expect("trainer cadence steps in");
        assert_eq!(cadence.source, DeviceRole::Trainer);
        assert!(cadence.bins.iter().any(|bin| bin.lower == 80.0));
    }

    #[test]
    fn traces_and_scatter_are_thinned_for_drawing() {
        let ready = Synthetic::default().ready();
        assert!(ready.traces.len() <= MAX_TRACE_POINTS);
        assert!(ready.traces.len() > MAX_TRACE_POINTS / 2);
        assert!(ready.bland_altman.len() <= MAX_SCATTER_POINTS);
        assert!(
            ready.traces.first().unwrap().elapsed_seconds
                < ready.traces.last().unwrap().elapsed_seconds
        );
        // Each trace carries both devices' smoothed power where present.
        assert!(
            ready
                .traces
                .iter()
                .filter(|point| point.a.is_some() && point.b.is_some())
                .count()
                > 400
        );
        let json = serde_json::to_value(PowerComparison::Ready(Box::new(ready))).unwrap();
        assert_eq!(json["status"], "ready");
        assert_eq!(json["a"]["role"], "trainer");
        assert!(json["windows"][1]["meanPercent"].is_number());
        assert!(json["lag"]["confident"].is_boolean());
    }

    #[test]
    fn arrival_jitter_on_a_one_hertz_meter_leaves_no_gaps() {
        let ready = Synthetic {
            meter_jitter_s: 0.15,
            duration_s: 300.0,
            ..Synthetic::default()
        }
        .ready();
        assert!(
            ready.b.seconds + 2 >= ready.coverage.overlap_seconds,
            "meter seconds {} of {}",
            ready.b.seconds,
            ready.coverage.overlap_seconds
        );
        assert_eq!(ready.a.seconds, ready.coverage.overlap_seconds);
        assert!(ready.a.mean_watts.is_some() && ready.b.max_watts.is_some());
    }

    #[test]
    fn helpers_behave_at_the_edges() {
        assert_eq!(
            smooth(&[Some(1.0), None, Some(3.0)], 1),
            vec![Some(1.0), None, Some(3.0)]
        );
        // A 3 s window needs two of three present; one of two at the edges.
        assert_eq!(
            smooth(&[Some(1.0), None, Some(3.0)], 3),
            vec![Some(1.0), Some(2.0), Some(3.0)]
        );
        assert_eq!(smooth(&[Some(1.0), None, None, Some(4.0)], 3)[2], None);
        assert_eq!(
            with_guard(&[false, false, true, false, false, false], 1),
            vec![false, true, true, true, false, false]
        );
        assert!(steps(&[Some(100.0), Some(151.0)], 1));
        assert!(!steps(&[Some(100.0), Some(149.0)], 1));
        assert!(!steps(&[None, Some(300.0)], 1));
        assert_eq!(
            hold(vec![Some(1.0), None, None, None, None, None], 4),
            vec![Some(1.0); 5]
                .into_iter()
                .chain([None])
                .collect::<Vec<_>>()
        );
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.95), 4.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.0);
        assert_eq!(thin((0..10).collect(), 4), vec![0, 3, 6, 9]);
        assert_eq!(thin((0..3).collect(), 4), vec![0, 1, 2]);
        assert!(
            (standard_deviation([2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0].into_iter()) - 2.138)
                .abs()
                < 0.01
        );
    }
}
