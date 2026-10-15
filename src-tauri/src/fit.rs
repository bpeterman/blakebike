use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, TimeZone, Utc};
use fit::{Decoder, Encoder, Field, FieldKind, Message, Value, profile::MesgNum};

use crate::{
    distance::{DistancePoint, estimate_distance},
    domain::{LapMark, SessionDetail, SessionSummary, Telemetry},
    storage::Storage,
};

const FIT_EXTENSION: &str = "fit";

pub fn ride_file_path(directory: &Path, summary: &SessionSummary) -> PathBuf {
    let date = summary.started_at.format("%Y-%m-%d_%H-%M-%S");
    let name = safe_name(&summary.workout_name);
    directory.join(format!("{date}_{name}_{}.fit", summary.id))
}

pub fn ensure_ride_file(directory: &Path, detail: &SessionDetail) -> Result<PathBuf, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create Ride Files directory: {error}"))?;
    let path = ride_file_path(directory, &detail.summary);
    if fit_file_is_compatible(&path) {
        return Ok(path);
    }

    let bytes = encode_activity(detail)?;
    let temporary = path.with_extension(format!("{FIT_EXTENSION}.tmp"));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("Could not write temporary FIT file: {error}"))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not finish FIT file: {error}"));
    }
    Ok(path)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub generated: usize,
    pub existing: usize,
    pub failures: Vec<(uuid::Uuid, String)>,
}

pub fn reconcile_ride_files(directory: &Path, storage: &Storage) -> ReconcileReport {
    let mut report = ReconcileReport::default();
    let sessions = match storage.sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            report.failures.push((uuid::Uuid::nil(), error));
            return report;
        }
    };
    for summary in sessions
        .into_iter()
        .filter(|summary| summary.ended_at.is_some())
    {
        let path = ride_file_path(directory, &summary);
        let existed = fit_file_is_compatible(&path);
        let result = storage
            .session(summary.id)
            .and_then(|detail| detail.ok_or_else(|| "Ride disappeared during backfill".to_string()))
            .and_then(|mut detail| {
                let estimate =
                    estimate_distance(&detail.samples, detail.summary.distance_weight_kg);
                detail.summary.estimated_distance_meters = estimate.total_meters;
                detail.summary.distance_source = estimate.source;
                storage.update_session_distance(
                    detail.summary.id,
                    estimate.total_meters,
                    estimate.source,
                )?;
                ensure_ride_file(directory, &detail).map(|_| ())
            });
        match result {
            Ok(()) if existed => report.existing += 1,
            Ok(()) => report.generated += 1,
            Err(error) => report.failures.push((summary.id, error)),
        }
    }
    report
}

pub fn encode_activity(detail: &SessionDetail) -> Result<Vec<u8>, String> {
    let summary = &detail.summary;
    let started_at = summary.started_at;
    let ended_at = summary.ended_at.unwrap_or_else(|| {
        started_at + chrono::Duration::seconds(i64::from(summary.elapsed_seconds))
    });
    let distance = estimate_distance(&detail.samples, summary.distance_weight_kg);
    let session = Totals::session(summary, ended_at, &detail.samples, &distance.points);
    let laps = laps(detail, &distance.points, &session);
    let mut messages = vec![
        message(
            MesgNum::FileId,
            vec![
                enum_field(0, "type", "activity"),
                enum_field(1, "manufacturer", "development"),
                uint_field(2, "product", 0),
                uint_field(3, "serial_number", serial_number(summary)),
                datetime_field(4, "time_created", started_at),
            ],
        ),
        event_message(started_at, "start"),
    ];

    for (sample, point) in detail.samples.iter().zip(&distance.points) {
        messages.push(record_message(sample, point)?);
    }

    messages.push(event_message(ended_at, "stop_all"));
    for (index, (lap, trigger)) in laps.iter().enumerate() {
        messages.push(lap_message(index, lap, trigger));
    }
    messages.push(session_message(summary, &session, laps.len()));
    messages.push(message(
        MesgNum::Activity,
        vec![
            datetime_field(253, "timestamp", ended_at),
            float_field(0, "total_timer_time", f64::from(session.timer_seconds)),
            uint_field(1, "num_sessions", 1),
            enum_field(2, "type", "manual"),
            enum_field(3, "event", "activity"),
            enum_field(4, "event_type", "stop"),
        ],
    ));

    let mut bytes = Encoder::new()
        .encode(&messages)
        .map_err(|error| format!("Could not encode FIT activity: {error}"))?;
    normalize_definition_base_types(&mut bytes)?;
    refresh_crcs(&mut bytes)?;
    fit::check_integrity(&bytes)
        .map_err(|error| format!("Generated FIT activity failed validation: {error}"))?;
    Ok(bytes)
}

fn fit_file_is_compatible(path: &Path) -> bool {
    let Ok(mut bytes) = fs::read(path) else {
        return false;
    };
    if fit::check_integrity(&bytes).is_err() {
        return false;
    }
    matches!(normalize_definition_base_types(&mut bytes), Ok(false))
        && fit_file_contains_distance(&bytes)
}

fn fit_file_contains_distance(bytes: &[u8]) -> bool {
    let (messages, errors) = Decoder::new(bytes).read_all();
    if !errors.is_empty() {
        return false;
    }
    let records: Vec<_> = messages
        .iter()
        .filter(|message| message.global_mesg_num == MesgNum::Record as u16)
        .collect();
    let records_have_distance =
        records.is_empty() || records.iter().all(|message| message.field(5).is_some());
    records_have_distance
        && messages.iter().any(|message| {
            message.global_mesg_num == MesgNum::Lap as u16 && message.field(9).is_some()
        })
        && messages.iter().any(|message| {
            message.global_mesg_num == MesgNum::Session as u16 && message.field(9).is_some()
        })
}

/// fit-sdk-rust 0.2.1 emits only the low five bits of FIT base type IDs.
/// External FIT readers require bit 7 for endian-aware multi-byte types; without
/// it they read little-endian timestamps as big-endian values.
fn normalize_definition_base_types(bytes: &mut [u8]) -> Result<bool, String> {
    if bytes.len() < 16 {
        return Err("FIT file is too short".to_string());
    }
    let header_size = usize::from(bytes[0]);
    let data_size = u32::from_le_bytes(
        bytes[4..8]
            .try_into()
            .map_err(|_| "FIT data size is missing".to_string())?,
    ) as usize;
    let data_end = header_size
        .checked_add(data_size)
        .ok_or_else(|| "FIT data size overflowed".to_string())?;
    if header_size > bytes.len() || data_end + 2 > bytes.len() {
        return Err("FIT data extends beyond the file".to_string());
    }

    let mut cursor = header_size;
    let mut local_sizes: [Option<usize>; 16] = [None; 16];
    let mut changed = false;
    while cursor < data_end {
        let record_header = bytes[cursor];
        cursor += 1;
        if record_header & 0x80 != 0 {
            let local = usize::from((record_header >> 5) & 0x03);
            let size = local_sizes[local]
                .ok_or_else(|| "Compressed FIT record has no definition".to_string())?;
            cursor = cursor
                .checked_add(size.saturating_sub(4))
                .ok_or_else(|| "FIT record size overflowed".to_string())?;
            continue;
        }

        let local = usize::from(record_header & 0x0f);
        if record_header & 0x40 == 0 {
            let size = local_sizes[local]
                .ok_or_else(|| "FIT data record has no definition".to_string())?;
            cursor = cursor
                .checked_add(size)
                .ok_or_else(|| "FIT record size overflowed".to_string())?;
            if cursor > data_end {
                return Err("FIT data record extends beyond the data section".to_string());
            }
            continue;
        }

        if cursor + 5 > data_end {
            return Err("FIT definition is truncated".to_string());
        }
        cursor += 4; // reserved, architecture, global message number
        let field_count = usize::from(bytes[cursor]);
        cursor += 1;
        let mut record_size = 0usize;
        for _ in 0..field_count {
            if cursor + 3 > data_end {
                return Err("FIT field definition is truncated".to_string());
            }
            record_size += usize::from(bytes[cursor + 1]);
            let base_type = bytes[cursor + 2] & 0x1f;
            if is_endian_aware_base_type(base_type) && bytes[cursor + 2] & 0x80 == 0 {
                bytes[cursor + 2] |= 0x80;
                changed = true;
            }
            cursor += 3;
        }
        if record_header & 0x20 != 0 {
            if cursor >= data_end {
                return Err("FIT developer definition is truncated".to_string());
            }
            let developer_count = usize::from(bytes[cursor]);
            cursor += 1;
            for _ in 0..developer_count {
                if cursor + 3 > data_end {
                    return Err("FIT developer field definition is truncated".to_string());
                }
                record_size += usize::from(bytes[cursor + 1]);
                cursor += 3;
            }
        }
        local_sizes[local] = Some(record_size);
    }
    if cursor != data_end {
        return Err("FIT records do not align with the data section".to_string());
    }
    Ok(changed)
}

fn is_endian_aware_base_type(base_type: u8) -> bool {
    matches!(
        base_type,
        0x03 | 0x04 | 0x05 | 0x06 | 0x08 | 0x09 | 0x0b | 0x0c | 0x0e | 0x0f | 0x10
    )
}

fn refresh_crcs(bytes: &mut [u8]) -> Result<(), String> {
    let header_size = usize::from(bytes[0]);
    let data_size = u32::from_le_bytes(
        bytes[4..8]
            .try_into()
            .map_err(|_| "FIT data size is missing".to_string())?,
    ) as usize;
    let data_end = header_size
        .checked_add(data_size)
        .ok_or_else(|| "FIT data size overflowed".to_string())?;
    if data_end + 2 > bytes.len() {
        return Err("FIT data extends beyond the file".to_string());
    }
    if header_size == 14 {
        let crc = fit::crc16(&bytes[..12]).to_le_bytes();
        bytes[12..14].copy_from_slice(&crc);
    }
    let crc = fit::crc16(&bytes[..data_end]).to_le_bytes();
    bytes[data_end..data_end + 2].copy_from_slice(&crc);
    Ok(())
}

fn record_message(sample: &Telemetry, distance: &DistancePoint) -> Result<Message, String> {
    let timestamp = Utc
        .timestamp_millis_opt(sample.timestamp_ms)
        .single()
        .ok_or_else(|| format!("Invalid telemetry timestamp: {}", sample.timestamp_ms))?;
    let mut fields = vec![
        datetime_field(253, "timestamp", timestamp),
        uint_field(7, "power", u64::from(sample.power_watts)),
        float_field(5, "distance", distance.distance_meters),
        float_field(6, "speed", distance.speed_mps),
    ];
    if let Some(heart_rate) = sample.heart_rate_bpm {
        fields.push(uint_field(3, "heart_rate", u64::from(heart_rate)));
    }
    if let Some(cadence) = sample.cadence_rpm {
        fields.push(uint_field(
            4,
            "cadence",
            cadence.round().clamp(0.0, 254.0) as u64,
        ));
    }
    Ok(message(MesgNum::Record, fields))
}

fn event_message(timestamp: DateTime<Utc>, event_type: &'static str) -> Message {
    message(
        MesgNum::Event,
        vec![
            datetime_field(253, "timestamp", timestamp),
            enum_field(0, "event", "timer"),
            enum_field(1, "event_type", event_type),
        ],
    )
}

/// The FIT laps with what ended each: one lap per manual lap the rider marked
/// plus the last one the ride's end closed, or the whole ride as a single lap
/// when they marked none. Marks are matched to the samples by wall time, so a
/// lap's records, averages and distance are exactly the samples ridden in it.
fn laps(
    detail: &SessionDetail,
    points: &[DistancePoint],
    session: &Totals,
) -> Vec<(Totals, &'static str)> {
    let marks = usable_marks(&detail.laps, session);
    if marks.is_empty() {
        return vec![(session.clone(), "session_end")];
    }
    let mut laps = Vec::with_capacity(marks.len() + 1);
    let mut start = session.start;
    let mut start_elapsed = 0;
    let mut first = 0;
    for (mark, end) in marks {
        let last = first
            + detail.samples[first..]
                .partition_point(|sample| sample.timestamp_ms < mark.timestamp_ms);
        laps.push((
            Totals::lap(
                start,
                end,
                mark.elapsed_seconds - start_elapsed,
                &detail.samples[first..last],
                &points[..last],
                first,
            ),
            "manual",
        ));
        start = end;
        start_elapsed = mark.elapsed_seconds;
        first = last;
    }
    laps.push((
        Totals::lap(
            start,
            session.end,
            session.timer_seconds.saturating_sub(start_elapsed),
            &detail.samples[first..],
            points,
            first,
        ),
        "session_end",
    ));
    laps
}

/// The marks that split the ride into laps, with their wall time: inside the
/// ride, and each later than the one before on both clocks. Anything else (a
/// clock jump, a corrupt row) would make a lap of no or negative length.
fn usable_marks(marks: &[LapMark], session: &Totals) -> Vec<(LapMark, DateTime<Utc>)> {
    let mut usable: Vec<(LapMark, DateTime<Utc>)> = Vec::new();
    for mark in marks {
        let Some(at) = Utc.timestamp_millis_opt(mark.timestamp_ms).single() else {
            continue;
        };
        let after_previous = usable.last().is_none_or(|(previous, previous_at)| {
            mark.elapsed_seconds > previous.elapsed_seconds && at > *previous_at
        });
        if at > session.start && at < session.end && mark.elapsed_seconds > 0 && after_previous {
            usable.push((*mark, at));
        }
    }
    usable
}

fn lap_message(index: usize, lap: &Totals, trigger: &'static str) -> Message {
    let mut fields = common_summary_fields(lap, SummaryKind::Lap);
    fields.extend([
        uint_field(254, "message_index", index as u64),
        enum_field(23, "intensity", "active"),
        enum_field(24, "lap_trigger", trigger),
        enum_field(25, "sport", "cycling"),
        uint_field(26, "event_group", 0),
    ]);
    message(MesgNum::Lap, fields)
}

fn session_message(summary: &SessionSummary, session: &Totals, lap_count: usize) -> Message {
    let mut fields = common_summary_fields(session, SummaryKind::Session);
    fields.extend([
        enum_field(5, "sport", "cycling"),
        enum_field(6, "sub_sport", "indoor_cycling"),
        uint_field(25, "first_lap_index", 0),
        uint_field(26, "num_laps", lap_count as u64),
        enum_field(28, "trigger", "activity_end"),
        string_field(110, "sport_profile_name", &summary.workout_name),
    ]);
    message(MesgNum::Session, fields)
}

#[derive(Clone, Copy)]
enum SummaryKind {
    Lap,
    Session,
}

fn common_summary_fields(totals: &Totals, kind: SummaryKind) -> Vec<Field> {
    let (avg_speed, max_speed, avg_hr, max_hr, avg_cadence, max_cadence, avg_power, max_power) =
        match kind {
            SummaryKind::Lap => (13, 14, 15, 16, 17, 18, 19, 20),
            SummaryKind::Session => (14, 15, 16, 17, 18, 19, 20, 21),
        };
    let event = match kind {
        SummaryKind::Lap => "lap",
        SummaryKind::Session => "session",
    };
    let mut fields = vec![
        datetime_field(253, "timestamp", totals.end),
        enum_field(0, "event", event),
        enum_field(1, "event_type", "stop"),
        datetime_field(2, "start_time", totals.start),
        float_field(7, "total_elapsed_time", totals.elapsed_seconds),
        float_field(8, "total_timer_time", f64::from(totals.timer_seconds)),
        float_field(9, "total_distance", totals.distance_meters),
        uint_field(
            avg_power,
            "avg_power",
            u64::from(totals.average_power_watts),
        ),
        uint_field(max_power, "max_power", u64::from(totals.max_power_watts)),
    ];
    if let Some(value) = totals.average_speed_mps {
        fields.push(float_field(avg_speed, "avg_speed", value));
    }
    if let Some(value) = totals.max_speed_mps {
        fields.push(float_field(max_speed, "max_speed", value));
    }
    if let Some(value) = totals.average_heart_rate {
        fields.push(uint_field(avg_hr, "avg_heart_rate", u64::from(value)));
    }
    if let Some(value) = totals.max_heart_rate {
        fields.push(uint_field(max_hr, "max_heart_rate", u64::from(value)));
    }
    if let Some(value) = totals.average_cadence_rpm {
        fields.push(uint_field(
            avg_cadence,
            "avg_cadence",
            value.round().clamp(0.0, 254.0) as u64,
        ));
    }
    if let Some(value) = totals.max_cadence {
        fields.push(uint_field(max_cadence, "max_cadence", u64::from(value)));
    }
    fields
}

/// What a lap or session message says about its stretch of the ride.
#[derive(Clone)]
struct Totals {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    /// Wall time, pauses included.
    elapsed_seconds: f64,
    /// Riding time on the ride clock.
    timer_seconds: u32,
    distance_meters: f64,
    average_power_watts: u16,
    max_power_watts: u16,
    average_cadence_rpm: Option<f32>,
    average_speed_mps: Option<f64>,
    max_speed_mps: Option<f64>,
    average_heart_rate: Option<u8>,
    max_heart_rate: Option<u8>,
    max_cadence: Option<u8>,
}

impl Totals {
    /// The whole ride. Power and cadence come from the summary the runner
    /// kept, so the session agrees with History.
    fn session(
        summary: &SessionSummary,
        end: DateTime<Utc>,
        samples: &[Telemetry],
        points: &[DistancePoint],
    ) -> Self {
        let mut totals = Self::from_samples(
            summary.started_at,
            end,
            summary.elapsed_seconds,
            samples,
            points,
            0.0,
        );
        // FIT keeps both: timer time leaves pauses out, elapsed time is the
        // wall clock from start to end.
        totals.elapsed_seconds = f64::from(
            summary
                .wall_clock_seconds()
                .unwrap_or(summary.elapsed_seconds),
        );
        totals.average_power_watts = summary.average_power_watts;
        totals.max_power_watts = summary.max_power_watts;
        totals.average_cadence_rpm = summary.average_cadence_rpm;
        totals
    }

    /// One manual lap: `samples` are the lap's own, `points` the ride's
    /// distance up to the lap's end, and `first` where the lap's samples
    /// start in them.
    fn lap(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        timer_seconds: u32,
        samples: &[Telemetry],
        points: &[DistancePoint],
        first: usize,
    ) -> Self {
        let start_meters = first
            .checked_sub(1)
            .and_then(|index| points.get(index))
            .map_or(0.0, |point| point.distance_meters);
        Self::from_samples(
            start,
            end,
            timer_seconds,
            samples,
            &points[first.min(points.len())..],
            start_meters,
        )
    }

    fn from_samples(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        timer_seconds: u32,
        samples: &[Telemetry],
        points: &[DistancePoint],
        start_meters: f64,
    ) -> Self {
        let heart_rates: Vec<u8> = samples
            .iter()
            .filter_map(|sample| sample.heart_rate_bpm)
            .collect();
        let cadences: Vec<f32> = samples
            .iter()
            .filter_map(|sample| sample.cadence_rpm)
            .collect();
        let distance_meters = points
            .last()
            .map_or(0.0, |point| point.distance_meters - start_meters)
            .max(0.0);
        Self {
            start,
            end,
            elapsed_seconds: (end - start).num_milliseconds().max(0) as f64 / 1000.0,
            timer_seconds,
            distance_meters,
            average_power_watts: if samples.is_empty() {
                0
            } else {
                (samples
                    .iter()
                    .map(|sample| u64::from(sample.power_watts))
                    .sum::<u64>()
                    / samples.len() as u64) as u16
            },
            max_power_watts: samples
                .iter()
                .map(|sample| sample.power_watts)
                .max()
                .unwrap_or(0),
            average_cadence_rpm: (!cadences.is_empty())
                .then(|| cadences.iter().sum::<f32>() / cadences.len() as f32),
            average_speed_mps: (timer_seconds > 0)
                .then(|| distance_meters / f64::from(timer_seconds)),
            max_speed_mps: points.iter().map(|point| point.speed_mps).reduce(f64::max),
            average_heart_rate: (!heart_rates.is_empty()).then(|| {
                (heart_rates
                    .iter()
                    .map(|value| u32::from(*value))
                    .sum::<u32>()
                    / heart_rates.len() as u32) as u8
            }),
            max_heart_rate: heart_rates.iter().copied().max(),
            max_cadence: cadences
                .iter()
                .map(|value| value.round().clamp(0.0, 254.0) as u8)
                .max(),
        }
    }
}

fn message(number: MesgNum, fields: Vec<Field>) -> Message {
    Message {
        global_mesg_num: number as u16,
        name: number.as_str(),
        fields,
    }
}

fn field(field_def_num: u8, name: &str, value: Value) -> Field {
    Field {
        name: name.to_string(),
        kind: FieldKind::Standard { field_def_num },
        value,
        units: None,
    }
}

fn enum_field(field_def_num: u8, name: &str, value: &'static str) -> Field {
    field(field_def_num, name, Value::Enum(value.into()))
}

fn uint_field(field_def_num: u8, name: &str, value: u64) -> Field {
    field(field_def_num, name, Value::UInt(value))
}

fn float_field(field_def_num: u8, name: &str, value: f64) -> Field {
    field(field_def_num, name, Value::Float(value))
}

fn datetime_field(field_def_num: u8, name: &str, value: DateTime<Utc>) -> Field {
    field(field_def_num, name, Value::DateTime(value))
}

fn string_field(field_def_num: u8, name: &str, value: &str) -> Field {
    field(field_def_num, name, Value::String(value.to_string()))
}

fn serial_number(summary: &SessionSummary) -> u64 {
    let bytes = summary.id.as_bytes();
    u64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).max(1))
}

fn safe_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let collapsed = cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "ride".to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;

    fn detail() -> SessionDetail {
        let started_at = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap();
        SessionDetail {
            summary: SessionSummary {
                id: Uuid::parse_str("12345678-1234-4321-9876-123456789abc").unwrap(),
                workout_id: None,
                workout_name: "Threshold / Ride".to_string(),
                started_at,
                ended_at: Some(started_at + chrono::Duration::seconds(2)),
                elapsed_seconds: 2,
                average_power_watts: 210,
                max_power_watts: 250,
                average_cadence_rpm: Some(91.0),
                estimated_distance_meters: 0.0,
                distance_source: None,
                distance_weight_kg: 84.0,
                completed: true,
                recording_warning: None,
            },
            samples: vec![
                Telemetry {
                    timestamp_ms: started_at.timestamp_millis(),
                    power_watts: 170,
                    cadence_rpm: Some(88.0),
                    speed_kph: Some(30.0),
                    heart_rate_bpm: Some(140),
                    target_power_watts: Some(200),
                },
                Telemetry {
                    timestamp_ms: (started_at + chrono::Duration::seconds(1)).timestamp_millis(),
                    power_watts: 250,
                    cadence_rpm: Some(94.0),
                    speed_kph: Some(32.0),
                    heart_rate_bpm: Some(150),
                    target_power_watts: Some(220),
                },
            ],
            laps: Vec::new(),
        }
    }

    #[test]
    fn encodes_required_activity_messages_and_metrics() {
        let bytes = encode_activity(&detail()).unwrap();
        fit::check_integrity(&bytes).unwrap();
        let mut normalized = bytes.clone();
        assert_eq!(normalize_definition_base_types(&mut normalized), Ok(false));
        let (messages, errors) = Decoder::builder(&bytes).build().read_all();
        assert!(errors.is_empty(), "{errors:?}");
        for required in ["file_id", "event", "record", "lap", "session", "activity"] {
            assert!(messages.iter().any(|message| message.name == required));
        }
        let record = messages
            .iter()
            .find(|message| message.name == "record")
            .unwrap();
        assert_eq!(record.field("power").unwrap().value.as_u64(), Some(170));
        assert_eq!(
            record.field("heart_rate").unwrap().value.as_u64(),
            Some(140)
        );
        assert_eq!(
            record.field("timestamp").unwrap().value.as_datetime(),
            Some(detail().summary.started_at)
        );
        assert_eq!(record.field("cadence").unwrap().value.as_u64(), Some(88));
        let speed = record
            .field("speed")
            .or_else(|| record.field("enhanced_speed"))
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!((speed - (30.0 / 3.6)).abs() < 0.01);
        assert_eq!(record.field("distance").unwrap().value.as_f64(), Some(0.0));
        let distances: Vec<f64> = messages
            .iter()
            .filter(|message| message.name == "record")
            .map(|message| message.field("distance").unwrap().value.as_f64().unwrap())
            .collect();
        assert!(distances.windows(2).all(|pair| pair[1] >= pair[0]));
        let session = messages
            .iter()
            .find(|message| message.name == "session")
            .unwrap();
        assert_eq!(
            session.field("event").unwrap().value.as_str(),
            Some("session")
        );
        assert_eq!(
            session.field("sport").unwrap().value.as_str(),
            Some("cycling")
        );
        assert_eq!(
            session.field("sub_sport").unwrap().value.as_str(),
            Some("indoor_cycling")
        );
        let total_distance = session
            .field("total_distance")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(total_distance > 8.0);
        assert!((total_distance - distances.last().unwrap()).abs() < 0.01);
    }

    /// Two minutes at one sample a second: 150 W for the first 60 s, 250 W
    /// after, and a manual lap at 60 s and 90 s.
    fn lapped_detail() -> SessionDetail {
        let mut detail = detail();
        let started_ms = detail.summary.started_at.timestamp_millis();
        detail.summary.ended_at = Some(detail.summary.started_at + chrono::Duration::seconds(120));
        detail.summary.elapsed_seconds = 120;
        detail.samples = (0..120_i64)
            .map(|second| Telemetry {
                timestamp_ms: started_ms + second * 1_000,
                power_watts: if second < 60 { 150 } else { 250 },
                cadence_rpm: Some(if second < 60 { 80.0 } else { 100.0 }),
                speed_kph: Some(36.0),
                heart_rate_bpm: Some(140),
                target_power_watts: None,
            })
            .collect();
        detail.laps = vec![
            LapMark {
                elapsed_seconds: 60,
                timestamp_ms: started_ms + 60_000,
            },
            LapMark {
                elapsed_seconds: 90,
                timestamp_ms: started_ms + 90_000,
            },
        ];
        detail
    }

    fn lap_messages(detail: &SessionDetail) -> (Vec<Message>, Message) {
        let bytes = encode_activity(detail).unwrap();
        fit::check_integrity(&bytes).unwrap();
        let (messages, errors) = Decoder::builder(&bytes).build().read_all();
        assert!(errors.is_empty(), "{errors:?}");
        let session = messages
            .iter()
            .find(|message| message.name == "session")
            .unwrap()
            .clone();
        let laps = messages
            .into_iter()
            .filter(|message| message.name == "lap")
            .collect();
        (laps, session)
    }

    fn uint(message: &Message, name: &str) -> u64 {
        message.field(name).unwrap().value.as_u64().unwrap()
    }

    fn float(message: &Message, name: &str) -> f64 {
        message.field(name).unwrap().value.as_f64().unwrap()
    }

    #[test]
    fn a_ride_without_manual_laps_is_one_lap() {
        let (laps, session) = lap_messages(&detail());
        assert_eq!(laps.len(), 1);
        assert_eq!(
            laps[0].field("lap_trigger").unwrap().value.as_str(),
            Some("session_end")
        );
        assert_eq!(uint(&session, "num_laps"), 1);
        assert_eq!(uint(&laps[0], "avg_power"), 210);
    }

    #[test]
    fn manual_laps_become_fit_laps_of_their_own_samples() {
        let detail = lapped_detail();
        let (laps, session) = lap_messages(&detail);
        assert_eq!(laps.len(), 3);
        assert_eq!(uint(&session, "num_laps"), 3);
        let triggers: Vec<_> = laps
            .iter()
            .map(|lap| {
                lap.field("lap_trigger")
                    .unwrap()
                    .value
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(triggers, ["manual", "manual", "session_end"]);
        let timers: Vec<f64> = laps
            .iter()
            .map(|lap| float(lap, "total_timer_time"))
            .collect();
        assert_eq!(timers, [60.0, 30.0, 30.0]);
        let powers: Vec<u64> = laps.iter().map(|lap| uint(lap, "avg_power")).collect();
        assert_eq!(powers, [150, 250, 250]);
        let cadences: Vec<u64> = laps.iter().map(|lap| uint(lap, "avg_cadence")).collect();
        assert_eq!(cadences, [80, 100, 100]);
        let starts: Vec<_> = laps
            .iter()
            .map(|lap| {
                lap.field("start_time")
                    .unwrap()
                    .value
                    .as_datetime()
                    .unwrap()
            })
            .collect();
        let started_at = detail.summary.started_at;
        assert_eq!(
            starts,
            [
                started_at,
                started_at + chrono::Duration::seconds(60),
                started_at + chrono::Duration::seconds(90),
            ]
        );
        // The laps' distances add up to the ride's.
        let lap_distance: f64 = laps.iter().map(|lap| float(lap, "total_distance")).sum();
        assert!((lap_distance - float(&session, "total_distance")).abs() < 0.05);
    }

    #[test]
    fn marks_that_cannot_split_the_ride_are_ignored() {
        let mut detail = lapped_detail();
        let started_ms = detail.summary.started_at.timestamp_millis();
        detail.laps = vec![
            // Before the ride, out of order, and after it ended.
            LapMark {
                elapsed_seconds: 10,
                timestamp_ms: started_ms - 5_000,
            },
            LapMark {
                elapsed_seconds: 60,
                timestamp_ms: started_ms + 60_000,
            },
            LapMark {
                elapsed_seconds: 50,
                timestamp_ms: started_ms + 70_000,
            },
            LapMark {
                elapsed_seconds: 200,
                timestamp_ms: started_ms + 200_000,
            },
        ];
        let (laps, _) = lap_messages(&detail);
        assert_eq!(laps.len(), 2);
    }

    #[test]
    fn timer_time_leaves_pauses_out_and_elapsed_time_keeps_them() {
        let mut detail = detail();
        // Two seconds ridden across a ride that took ninety on the wall clock.
        detail.summary.ended_at = Some(detail.summary.started_at + chrono::Duration::seconds(90));
        let bytes = encode_activity(&detail).unwrap();
        let (messages, errors) = Decoder::builder(&bytes).build().read_all();
        assert!(errors.is_empty(), "{errors:?}");
        let time = |name: &str, field: &str| {
            messages
                .iter()
                .find(|message| message.name == name)
                .and_then(|message| message.field(field))
                .and_then(|field| field.value.as_f64())
        };
        for summary in ["lap", "session"] {
            assert_eq!(time(summary, "total_timer_time"), Some(2.0), "{summary}");
            assert_eq!(time(summary, "total_elapsed_time"), Some(90.0), "{summary}");
        }
        assert_eq!(time("activity", "total_timer_time"), Some(2.0));
    }

    #[test]
    fn persists_to_a_deterministic_safe_path() {
        let directory = tempdir().unwrap();
        let detail = detail();
        let first = ensure_ride_file(directory.path(), &detail).unwrap();
        let second = ensure_ride_file(directory.path(), &detail).unwrap();
        assert_eq!(first, second);
        assert!(first.is_file());
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("threshold-ride")
        );
        assert!(!first.with_extension("fit.tmp").exists());
    }

    #[test]
    fn replaces_files_created_without_endian_aware_base_types() {
        let directory = tempdir().unwrap();
        let detail = detail();
        let path = ride_file_path(directory.path(), &detail.summary);
        let mut legacy = encode_activity(&detail).unwrap();
        let field = legacy
            .windows(3)
            .position(|window| window == [4, 4, 0x86])
            .expect("time_created definition");
        legacy[field + 2] = 0x06;
        refresh_crcs(&mut legacy).unwrap();
        fs::write(&path, legacy).unwrap();
        assert!(!fit_file_is_compatible(&path));

        ensure_ride_file(directory.path(), &detail).unwrap();
        assert!(fit_file_is_compatible(&path));
    }

    #[test]
    fn empty_names_get_a_safe_fallback() {
        let mut detail = detail();
        detail.summary.workout_name = "///".to_string();
        assert!(
            ride_file_path(Path::new("/tmp"), &detail.summary)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("_ride_")
        );
    }

    #[test]
    fn reconciliation_backfills_missing_files_without_changing_ride_data() {
        let storage = Storage::in_memory().unwrap();
        let mut summary = storage.start_session(None, "Backfill Ride", 84.0).unwrap();
        let sample = detail().samples[0].clone();
        storage.record_sample(summary.id, &sample).unwrap();
        summary.ended_at = Some(summary.started_at + chrono::Duration::seconds(1));
        summary.elapsed_seconds = 1;
        summary.average_power_watts = sample.power_watts;
        summary.max_power_watts = sample.power_watts;
        storage.finish_session(&summary).unwrap();

        let directory = tempdir().unwrap();
        let report = reconcile_ride_files(directory.path(), &storage);
        assert_eq!(report.generated, 1);
        assert!(report.failures.is_empty());
        assert!(ride_file_path(directory.path(), &summary).is_file());
        let stored = storage.session(summary.id).unwrap().unwrap();
        assert_eq!(stored.samples.len(), 1);
        assert_eq!(stored.summary.estimated_distance_meters, 0.0);
        assert_eq!(
            stored.summary.distance_source,
            Some(crate::domain::DistanceSource::Trainer)
        );
    }

    #[test]
    fn reconciliation_reports_filesystem_failures_and_preserves_the_ride() {
        let storage = Storage::in_memory().unwrap();
        let mut summary = storage.start_session(None, "Protected Ride", 84.0).unwrap();
        summary.ended_at = Some(summary.started_at);
        storage.finish_session(&summary).unwrap();

        let directory = tempdir().unwrap();
        let not_a_directory = directory.path().join("blocked");
        fs::write(&not_a_directory, b"file").unwrap();
        let report = reconcile_ride_files(&not_a_directory, &storage);
        assert_eq!(report.failures.len(), 1);
        assert!(storage.session(summary.id).unwrap().is_some());
    }
}
