use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, TimeZone, Utc};
use fit::{Decoder, Encoder, Field, FieldKind, Message, Value, profile::MesgNum};

use crate::{
    distance::{DistanceEstimate, DistancePoint, estimate_distance},
    domain::{SessionDetail, SessionSummary, Telemetry},
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
    let stats = RideStats::from_samples(&detail.samples, &distance, summary.elapsed_seconds);
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
    messages.push(lap_message(
        summary,
        ended_at,
        &stats,
        distance.total_meters,
    ));
    messages.push(session_message(
        summary,
        ended_at,
        &stats,
        distance.total_meters,
    ));
    messages.push(message(
        MesgNum::Activity,
        vec![
            datetime_field(253, "timestamp", ended_at),
            float_field(0, "total_timer_time", f64::from(summary.elapsed_seconds)),
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

fn lap_message(
    summary: &SessionSummary,
    ended_at: DateTime<Utc>,
    stats: &RideStats,
    total_distance_meters: f64,
) -> Message {
    let mut fields = common_summary_fields(summary, ended_at, stats, SummaryKind::Lap);
    fields.push(float_field(9, "total_distance", total_distance_meters));
    fields.extend([
        enum_field(23, "intensity", "active"),
        enum_field(24, "lap_trigger", "session_end"),
        enum_field(25, "sport", "cycling"),
        uint_field(26, "event_group", 0),
    ]);
    message(MesgNum::Lap, fields)
}

fn session_message(
    summary: &SessionSummary,
    ended_at: DateTime<Utc>,
    stats: &RideStats,
    total_distance_meters: f64,
) -> Message {
    let mut fields = common_summary_fields(summary, ended_at, stats, SummaryKind::Session);
    fields.push(float_field(9, "total_distance", total_distance_meters));
    fields.extend([
        enum_field(5, "sport", "cycling"),
        enum_field(6, "sub_sport", "indoor_cycling"),
        uint_field(25, "first_lap_index", 0),
        uint_field(26, "num_laps", 1),
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

fn common_summary_fields(
    summary: &SessionSummary,
    ended_at: DateTime<Utc>,
    stats: &RideStats,
    kind: SummaryKind,
) -> Vec<Field> {
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
        datetime_field(253, "timestamp", ended_at),
        enum_field(0, "event", event),
        enum_field(1, "event_type", "stop"),
        datetime_field(2, "start_time", summary.started_at),
        float_field(7, "total_elapsed_time", f64::from(summary.elapsed_seconds)),
        float_field(8, "total_timer_time", f64::from(summary.elapsed_seconds)),
        uint_field(
            avg_power,
            "avg_power",
            u64::from(summary.average_power_watts),
        ),
        uint_field(max_power, "max_power", u64::from(summary.max_power_watts)),
    ];
    if let Some(value) = stats.average_speed_mps {
        fields.push(float_field(avg_speed, "avg_speed", value));
    }
    if let Some(value) = stats.max_speed_mps {
        fields.push(float_field(max_speed, "max_speed", value));
    }
    if let Some(value) = stats.average_heart_rate {
        fields.push(uint_field(avg_hr, "avg_heart_rate", u64::from(value)));
    }
    if let Some(value) = stats.max_heart_rate {
        fields.push(uint_field(max_hr, "max_heart_rate", u64::from(value)));
    }
    if let Some(value) = summary.average_cadence_rpm {
        fields.push(uint_field(
            avg_cadence,
            "avg_cadence",
            value.round().clamp(0.0, 254.0) as u64,
        ));
    }
    if let Some(value) = stats.max_cadence {
        fields.push(uint_field(max_cadence, "max_cadence", u64::from(value)));
    }
    fields
}

#[derive(Default)]
struct RideStats {
    average_speed_mps: Option<f64>,
    max_speed_mps: Option<f64>,
    average_heart_rate: Option<u8>,
    max_heart_rate: Option<u8>,
    max_cadence: Option<u8>,
}

impl RideStats {
    fn from_samples(
        samples: &[Telemetry],
        distance: &DistanceEstimate,
        elapsed_seconds: u32,
    ) -> Self {
        let heart_rates: Vec<u8> = samples
            .iter()
            .filter_map(|sample| sample.heart_rate_bpm)
            .collect();
        let cadences: Vec<u8> = samples
            .iter()
            .filter_map(|sample| {
                sample
                    .cadence_rpm
                    .map(|value| value.round().clamp(0.0, 254.0) as u8)
            })
            .collect();
        Self {
            average_speed_mps: (elapsed_seconds > 0)
                .then(|| distance.total_meters / f64::from(elapsed_seconds)),
            max_speed_mps: distance.max_speed_mps,
            average_heart_rate: (!heart_rates.is_empty()).then(|| {
                (heart_rates
                    .iter()
                    .map(|value| u32::from(*value))
                    .sum::<u32>()
                    / heart_rates.len() as u32) as u8
            }),
            max_heart_rate: heart_rates.iter().copied().max(),
            max_cadence: cadences.iter().copied().max(),
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
