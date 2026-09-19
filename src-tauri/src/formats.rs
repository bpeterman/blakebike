use chrono::Utc;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

use crate::domain::{PowerTarget, Workout, WorkoutStep};

pub fn import_zwo(xml: &str) -> Result<Workout, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut name = "Imported workout".to_string();
    let mut description = String::new();
    let mut steps = Vec::new();
    let mut current_text = None::<String>;
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) => {
                let tag = String::from_utf8_lossy(element.name().as_ref()).to_string();
                match tag.as_str() {
                    "name" | "description" => current_text = Some(tag),
                    "SteadyState" => steps.push(steady(&element)?),
                    "Warmup" | "Cooldown" | "Ramp" => steps.push(ramp(&element)?),
                    "FreeRide" => steps.push(WorkoutStep::FreeRide {
                        duration_seconds: required_u32(&element, b"Duration")?,
                    }),
                    "IntervalsT" => steps.push(intervals(&element)?),
                    _ => {}
                }
            }
            Event::Empty(element) => {
                let tag = element.name();
                match tag.as_ref() {
                    b"SteadyState" => steps.push(steady(&element)?),
                    b"Warmup" | b"Cooldown" | b"Ramp" => steps.push(ramp(&element)?),
                    b"FreeRide" => steps.push(WorkoutStep::FreeRide {
                        duration_seconds: required_u32(&element, b"Duration")?,
                    }),
                    b"IntervalsT" => steps.push(intervals(&element)?),
                    _ => {}
                }
            }
            Event::Text(text) => {
                if let Some(tag) = current_text.take() {
                    let value = text
                        .decode()
                        .map_err(|error| error.to_string())?
                        .into_owned();
                    if tag == "name" {
                        name = value;
                    } else {
                        description = value;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut workout = Workout::new(name, steps);
    workout.description = description;
    workout.source = "zwo".into();
    workout.created_at = Utc::now();
    workout.updated_at = workout.created_at;
    workout.validate()?;
    Ok(workout)
}

fn steady(element: &BytesStart<'_>) -> Result<WorkoutStep, String> {
    Ok(WorkoutStep::Steady {
        duration_seconds: required_u32(element, b"Duration")?,
        target: percent_target(required_f32(element, b"Power")?),
    })
}

fn ramp(element: &BytesStart<'_>) -> Result<WorkoutStep, String> {
    Ok(WorkoutStep::Ramp {
        duration_seconds: required_u32(element, b"Duration")?,
        start: percent_target(required_f32(element, b"PowerLow")?),
        end: percent_target(required_f32(element, b"PowerHigh")?),
    })
}

fn intervals(element: &BytesStart<'_>) -> Result<WorkoutStep, String> {
    Ok(WorkoutStep::Repeat {
        repetitions: required_u32(element, b"Repeat")?
            .try_into()
            .map_err(|_| "Repeat count is too large".to_string())?,
        steps: vec![
            WorkoutStep::Steady {
                duration_seconds: required_u32(element, b"OnDuration")?,
                target: percent_target(required_f32(element, b"OnPower")?),
            },
            WorkoutStep::Steady {
                duration_seconds: required_u32(element, b"OffDuration")?,
                target: percent_target(required_f32(element, b"OffPower")?),
            },
        ],
    })
}

fn required_u32(element: &BytesStart<'_>, key: &[u8]) -> Result<u32, String> {
    required_attribute(element, key)?
        .parse()
        .map_err(|_| format!("Invalid numeric attribute {}", String::from_utf8_lossy(key)))
}

fn required_f32(element: &BytesStart<'_>, key: &[u8]) -> Result<f32, String> {
    required_attribute(element, key)?
        .parse()
        .map_err(|_| format!("Invalid numeric attribute {}", String::from_utf8_lossy(key)))
}

fn required_attribute(element: &BytesStart<'_>, key: &[u8]) -> Result<String, String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
        .ok_or_else(|| format!("Missing attribute {}", String::from_utf8_lossy(key)))
}

fn percent_target(fraction: f32) -> PowerTarget {
    PowerTarget::PercentFtp((fraction * 100.0).round().max(1.0) as u16)
}

pub fn export_zwo(workout: &Workout, ftp: u16) -> String {
    let mut output = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<workout_file>\n  <name>{}</name>\n  <description>{}</description>\n  <workout>\n",
        escape_xml(&workout.name),
        escape_xml(&workout.description)
    );
    for step in &workout.steps {
        write_step(&mut output, step, ftp, "    ");
    }
    output.push_str("  </workout>\n</workout_file>\n");
    output
}

fn write_step(output: &mut String, step: &WorkoutStep, ftp: u16, indent: &str) {
    match step {
        WorkoutStep::Steady {
            duration_seconds,
            target,
        } => output.push_str(&format!(
            "{indent}<SteadyState Duration=\"{duration_seconds}\" Power=\"{:.3}\" />\n",
            target.watts(ftp) as f32 / ftp as f32
        )),
        WorkoutStep::Ramp {
            duration_seconds,
            start,
            end,
        } => output.push_str(&format!(
            "{indent}<Ramp Duration=\"{duration_seconds}\" PowerLow=\"{:.3}\" PowerHigh=\"{:.3}\" />\n",
            start.watts(ftp) as f32 / ftp as f32,
            end.watts(ftp) as f32 / ftp as f32
        )),
        WorkoutStep::FreeRide { duration_seconds } => output.push_str(&format!(
            "{indent}<FreeRide Duration=\"{duration_seconds}\" />\n"
        )),
        WorkoutStep::Repeat { repetitions, steps } => {
            for _ in 0..*repetitions {
                for nested in steps {
                    write_step(output, nested, ftp, indent);
                }
            }
        }
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_common_zwo_steps() {
        let xml = r#"<workout_file><name>Intervals</name><workout>
          <Warmup Duration="60" PowerLow="0.4" PowerHigh="0.7"/>
          <IntervalsT Repeat="3" OnDuration="30" OffDuration="30" OnPower="1.2" OffPower="0.5"/>
        </workout></workout_file>"#;
        let workout = import_zwo(xml).unwrap();
        assert_eq!(workout.name, "Intervals");
        assert_eq!(workout.duration_seconds(), 240);
    }

    #[test]
    fn escapes_names_when_exporting() {
        let workout = Workout::new(
            "Threshold & chill",
            vec![WorkoutStep::FreeRide {
                duration_seconds: 60,
            }],
        );
        assert!(export_zwo(&workout, 200).contains("Threshold &amp; chill"));
    }
}
