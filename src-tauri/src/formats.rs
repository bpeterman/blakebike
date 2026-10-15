use chrono::Utc;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

use crate::domain::{PowerTarget, Workout, WorkoutStep};

/// Parse a Zwift workout file. Every direct child of `<workout>` must be a
/// step we understand; an unknown one fails the import rather than silently
/// shortening the workout. Children of steps (`textevent` cues) and the
/// metadata outside `<workout>` other than name and description are ignored.
pub fn import_zwo(xml: &str) -> Result<Workout, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut name = "Imported workout".to_string();
    let mut description = String::new();
    let mut steps = Vec::new();
    let mut current_text = None::<String>;
    // Nesting depth of the element being read, and the depth at which the
    // children of `<workout>` sit while inside it.
    let mut depth = 0_usize;
    let mut step_depth = None::<usize>;
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) => {
                let tag = element.name();
                let tag = tag.as_ref();
                if step_depth == Some(depth) {
                    steps.push(step(tag, &element)?);
                } else if tag == b"workout" {
                    step_depth = Some(depth + 1);
                } else if tag == b"name" || tag == b"description" {
                    current_text = Some(String::from_utf8_lossy(tag).into_owned());
                }
                depth += 1;
            }
            Event::Empty(element) => {
                if step_depth == Some(depth) {
                    steps.push(step(element.name().as_ref(), &element)?);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if step_depth == Some(depth + 1) {
                    step_depth = None;
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

/// One direct child of `<workout>`. `SolidState` is the pre-2015 spelling of
/// `SteadyState`; `MaxEffort` has no target and rides like free ride.
fn step(tag: &[u8], element: &BytesStart<'_>) -> Result<WorkoutStep, String> {
    match tag {
        b"SteadyState" | b"SolidState" => steady(element),
        b"Warmup" | b"Cooldown" | b"Ramp" => ramp(element),
        b"FreeRide" | b"MaxEffort" => Ok(WorkoutStep::FreeRide {
            duration_seconds: required_u32(element, b"Duration")?,
        }),
        b"IntervalsT" => intervals(element),
        other => Err(format!(
            "Unsupported workout element <{}>",
            String::from_utf8_lossy(other)
        )),
    }
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
        assert_eq!(workout.source, "zwo");
        assert!(!workout.is_mirrored());
    }

    #[test]
    fn maps_legacy_and_effort_steps_and_ignores_step_children() {
        let xml = r#"<workout_file>
          <author>Someone</author><name>Mixed</name><sportType>bike</sportType>
          <tags><tag name="INTERVALS"/></tags>
          <workout>
            <SolidState Duration="120" Power="0.6"/>
            <SteadyState Duration="300" Power="0.9" Cadence="90">
              <textevent timeoffset="0" message="Settle in"/>
            </SteadyState>
            <MaxEffort Duration="30"/>
            <Cooldown Duration="60" PowerLow="0.7" PowerHigh="0.4"/>
          </workout>
        </workout_file>"#;
        let workout = import_zwo(xml).unwrap();
        assert_eq!(workout.name, "Mixed");
        assert_eq!(workout.steps.len(), 4);
        assert!(matches!(
            workout.steps[0],
            WorkoutStep::Steady {
                duration_seconds: 120,
                target: PowerTarget::PercentFtp(60)
            }
        ));
        assert!(matches!(
            workout.steps[2],
            WorkoutStep::FreeRide {
                duration_seconds: 30
            }
        ));
        assert!(matches!(workout.steps[3], WorkoutStep::Ramp { .. }));
    }

    #[test]
    fn refuses_unknown_step_elements_instead_of_dropping_them() {
        let xml = r#"<workout_file><name>Odd</name><workout>
          <SteadyState Duration="300" Power="0.7"/>
          <Sprint Duration="15"/>
        </workout></workout_file>"#;
        assert_eq!(
            import_zwo(xml).unwrap_err(),
            "Unsupported workout element <Sprint>"
        );
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
