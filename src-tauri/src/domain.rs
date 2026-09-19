use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub ftp_watts: u16,
    pub max_power_watts: u16,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Rider".into(),
            ftp_watts: 200,
            max_power_watts: 800,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workout {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub source: String,
    pub version: u32,
    pub steps: Vec<WorkoutStep>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Workout {
    pub fn new(name: impl Into<String>, steps: Vec<WorkoutStep>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: String::new(),
            source: "local".into(),
            version: 1,
            steps,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn duration_seconds(&self) -> u32 {
        self.steps.iter().map(WorkoutStep::duration_seconds).sum()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Workout name is required".into());
        }
        if self.steps.is_empty() {
            return Err("Add at least one workout step".into());
        }
        if self.duration_seconds() > 6 * 60 * 60 {
            return Err("Workout cannot exceed six hours".into());
        }
        for step in &self.steps {
            step.validate()?;
        }
        Ok(())
    }

    pub fn compile(&self, ftp: u16) -> Vec<Interval> {
        let mut intervals = Vec::new();
        for step in &self.steps {
            step.compile(ftp, &mut intervals);
        }
        intervals
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkoutStep {
    Steady {
        duration_seconds: u32,
        target: PowerTarget,
    },
    Ramp {
        duration_seconds: u32,
        start: PowerTarget,
        end: PowerTarget,
    },
    FreeRide {
        duration_seconds: u32,
    },
    Repeat {
        repetitions: u8,
        steps: Vec<WorkoutStep>,
    },
}

impl WorkoutStep {
    pub fn duration_seconds(&self) -> u32 {
        match self {
            Self::Steady { duration_seconds, .. }
            | Self::Ramp { duration_seconds, .. }
            | Self::FreeRide { duration_seconds } => *duration_seconds,
            Self::Repeat { repetitions, steps } => {
                u32::from(*repetitions)
                    * steps.iter().map(WorkoutStep::duration_seconds).sum::<u32>()
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Steady {
                duration_seconds,
                target,
            } => {
                validate_duration(*duration_seconds)?;
                target.validate()
            }
            Self::Ramp {
                duration_seconds,
                start,
                end,
            } => {
                validate_duration(*duration_seconds)?;
                start.validate()?;
                end.validate()
            }
            Self::FreeRide { duration_seconds } => validate_duration(*duration_seconds),
            Self::Repeat { repetitions, steps } => {
                if *repetitions < 2 || *repetitions > 20 {
                    return Err("Repeats must be between 2 and 20".into());
                }
                if steps.is_empty() {
                    return Err("A repeat group cannot be empty".into());
                }
                for step in steps {
                    step.validate()?;
                }
                Ok(())
            }
        }
    }

    fn compile(&self, ftp: u16, output: &mut Vec<Interval>) {
        match self {
            Self::Steady {
                duration_seconds,
                target,
            } => output.push(Interval {
                duration_seconds: *duration_seconds,
                start_watts: Some(target.watts(ftp)),
                end_watts: Some(target.watts(ftp)),
                free_ride: false,
            }),
            Self::Ramp {
                duration_seconds,
                start,
                end,
            } => output.push(Interval {
                duration_seconds: *duration_seconds,
                start_watts: Some(start.watts(ftp)),
                end_watts: Some(end.watts(ftp)),
                free_ride: false,
            }),
            Self::FreeRide { duration_seconds } => output.push(Interval {
                duration_seconds: *duration_seconds,
                start_watts: None,
                end_watts: None,
                free_ride: true,
            }),
            Self::Repeat { repetitions, steps } => {
                for _ in 0..*repetitions {
                    for step in steps {
                        step.compile(ftp, output);
                    }
                }
            }
        }
    }
}

fn validate_duration(duration: u32) -> Result<(), String> {
    if duration == 0 {
        Err("Step duration must be greater than zero".into())
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "unit", content = "value", rename_all = "camelCase")]
pub enum PowerTarget {
    Watts(u16),
    PercentFtp(u16),
}

impl PowerTarget {
    pub fn watts(self, ftp: u16) -> u16 {
        match self {
            Self::Watts(watts) => watts,
            Self::PercentFtp(percent) => {
                ((u32::from(ftp) * u32::from(percent)) / 100).min(u32::from(u16::MAX)) as u16
            }
        }
    }

    fn validate(self) -> Result<(), String> {
        match self {
            Self::Watts(0) => Err("Power must be greater than zero".into()),
            Self::PercentFtp(0 | 301..) => Err("FTP target must be between 1% and 300%".into()),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub duration_seconds: u32,
    pub start_watts: Option<u16>,
    pub end_watts: Option<u16>,
    pub free_ride: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Telemetry {
    pub timestamp_ms: i64,
    pub power_watts: u16,
    pub cadence_rpm: Option<f32>,
    pub speed_kph: Option<f32>,
    pub heart_rate_bpm: Option<u8>,
    pub target_power_watts: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: Uuid,
    pub workout_id: Option<Uuid>,
    pub workout_name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub elapsed_seconds: u32,
    pub average_power_watts: u16,
    pub max_power_watts: u16,
    pub average_cadence_rpm: Option<f32>,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub samples: Vec<Telemetry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_repeats_and_percent_targets() {
        let workout = Workout::new(
            "Test",
            vec![WorkoutStep::Repeat {
                repetitions: 2,
                steps: vec![WorkoutStep::Steady {
                    duration_seconds: 30,
                    target: PowerTarget::PercentFtp(120),
                }],
            }],
        );
        let intervals = workout.compile(200);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0].start_watts, Some(240));
        assert_eq!(workout.duration_seconds(), 60);
    }

    #[test]
    fn rejects_empty_workout() {
        assert!(Workout::new("", vec![]).validate().is_err());
    }
}
