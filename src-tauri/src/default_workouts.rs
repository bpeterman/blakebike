//! The workout library every new install starts with.
//!
//! The set is deliberately small and recognizable: one test to anchor FTP
//! (every target here is a percentage of it), then a spread of durations and
//! intensities that covers an ordinary training week. Names lead with the
//! structure and descriptions lead with duration and zone, because those are
//! the only two things the library list can show.

use chrono::{DateTime, Duration, Utc};

use crate::domain::{PowerTarget, Workout, WorkoutStep};

/// Bumped whenever [`default_workouts`] gains entries, so existing installs
/// pick up the additions exactly once.
pub const DEFAULT_WORKOUTS_VERSION: u32 = 1;

fn steady(duration_seconds: u32, percent_ftp: u16) -> WorkoutStep {
    WorkoutStep::Steady {
        duration_seconds,
        target: PowerTarget::PercentFtp(percent_ftp),
    }
}

fn ramp(duration_seconds: u32, start_percent: u16, end_percent: u16) -> WorkoutStep {
    WorkoutStep::Ramp {
        duration_seconds,
        start: PowerTarget::PercentFtp(start_percent),
        end: PowerTarget::PercentFtp(end_percent),
    }
}

fn free_ride(duration_seconds: u32) -> WorkoutStep {
    WorkoutStep::FreeRide { duration_seconds }
}

fn repeat(repetitions: u8, steps: Vec<WorkoutStep>) -> WorkoutStep {
    WorkoutStep::Repeat { repetitions, steps }
}

fn workout(name: &str, description: &str, steps: Vec<WorkoutStep>) -> Workout {
    let mut workout = Workout::new(name, steps);
    workout.description = description.into();
    workout
}

/// The defaults, newest first: index 0 sorts to the top of the library and
/// onto the home screen, so the test and the two most-reached-for sessions
/// lead.
///
/// `newest` is the timestamp the first entry gets; each later entry is dated a
/// second earlier to hold this order through `ORDER BY updated_at DESC`.
pub fn default_workouts(newest: DateTime<Utc>) -> Vec<Workout> {
    let mut workouts = vec![
        ftp_test(),
        endurance_60(),
        sweet_spot(),
        threshold(),
        over_unders(),
        vo2_max(),
        tempo(),
        endurance_90(),
        recovery_spin(),
    ];
    for (index, workout) in workouts.iter_mut().enumerate() {
        let at = newest - Duration::seconds(index as i64);
        workout.created_at = at;
        workout.updated_at = at;
    }
    workouts
}

fn ftp_test() -> Workout {
    workout(
        "FTP Test (20 min)",
        "60 min · test. Ride the 20-minute free-ride block as hard as you can \
         hold it, then set your FTP to 95% of its average power. Everything \
         else in the library is a percentage of that number, so start here.",
        vec![
            ramp(600, 45, 65),
            repeat(3, vec![steady(60, 105), steady(60, 50)]),
            steady(300, 50),
            steady(300, 100),
            steady(600, 50),
            free_ride(1200),
            ramp(240, 55, 40),
        ],
    )
}

fn endurance_60() -> Workout {
    workout(
        "Endurance 60",
        "60 min · endurance. Steady aerobic riding you can hold a conversation \
         through. The session most weeks should be built around.",
        vec![ramp(600, 50, 65), steady(2400, 68), ramp(600, 65, 45)],
    )
}

fn sweet_spot() -> Workout {
    workout(
        "Sweet Spot 3×12",
        "60 min · sweet spot. Three twelve-minute blocks just under threshold \
         — hard enough to drive fitness, easy enough to repeat later in the week.",
        vec![
            ramp(600, 50, 70),
            repeat(3, vec![steady(720, 90), steady(240, 55)]),
            ramp(120, 55, 40),
        ],
    )
}

fn threshold() -> Workout {
    workout(
        "Threshold 2×20",
        "75 min · threshold. The classic pair of twenty-minute efforts at your \
         FTP, with a full ten minutes easy between them.",
        vec![
            ramp(720, 50, 70),
            repeat(2, vec![steady(1200, 98), steady(600, 55)]),
            ramp(180, 55, 40),
        ],
    )
}

fn over_unders() -> Workout {
    workout(
        "Over-Unders 3×9",
        "60 min · threshold. Three nine-minute blocks alternating two minutes \
         over threshold with one minute just under, so you practice clearing \
         lactate without easing off.",
        vec![
            ramp(600, 50, 70),
            repeat(
                3,
                vec![
                    repeat(3, vec![steady(120, 105), steady(60, 88)]),
                    steady(360, 55),
                ],
            ),
            ramp(300, 55, 40),
        ],
    )
}

fn vo2_max() -> Workout {
    workout(
        "VO2 5×3",
        "50 min · VO2 max. Five three-minute efforts well above threshold with \
         equal recovery. Short, and the hardest session here.",
        vec![
            ramp(600, 50, 70),
            repeat(5, vec![steady(180, 115), steady(180, 50)]),
            ramp(600, 50, 40),
        ],
    )
}

fn tempo() -> Workout {
    workout(
        "Tempo 2×20",
        "60 min · tempo. Two twenty-minute blocks at a firm but sustainable \
         pace — a step up from endurance without the cost of threshold work.",
        vec![
            ramp(480, 50, 65),
            repeat(2, vec![steady(1200, 80), steady(300, 55)]),
            ramp(120, 55, 40),
        ],
    )
}

fn endurance_90() -> Workout {
    workout(
        "Endurance 90",
        "90 min · endurance. A longer aerobic ride, with a three-minute tempo \
         lift at the end of each block to keep it from dragging.",
        vec![
            ramp(600, 50, 65),
            repeat(3, vec![steady(1200, 68), steady(180, 80)]),
            ramp(660, 65, 45),
        ],
    )
}

fn recovery_spin() -> Workout {
    workout(
        "Recovery Spin",
        "30 min · recovery. Easy spinning for the day after something hard. If \
         it feels like work, turn it down.",
        vec![ramp(300, 40, 50), steady(1200, 50), ramp(300, 50, 40)],
    )
}

/// The single "FTP Builder" sample that shipped before this library existed.
/// Installs that still hold it untouched have it replaced by the set above;
/// a copy the rider has edited is theirs and stays.
pub fn legacy_sample_steps() -> Vec<WorkoutStep> {
    vec![
        ramp(300, 45, 70),
        repeat(3, vec![steady(180, 100), steady(120, 55)]),
        ramp(300, 65, 40),
    ]
}

pub fn is_legacy_sample(workout: &Workout) -> bool {
    workout.name == "FTP Builder" && workout.steps == legacy_sample_steps()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_is_valid() {
        for workout in default_workouts(Utc::now()) {
            workout
                .validate()
                .unwrap_or_else(|error| panic!("{}: {error}", workout.name));
        }
    }

    #[test]
    fn names_and_descriptions_are_distinct_and_present() {
        let workouts = default_workouts(Utc::now());
        let mut names: Vec<&str> = workouts.iter().map(|it| it.name.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "default workout names must be unique");
        for workout in &workouts {
            assert!(
                !workout.description.trim().is_empty(),
                "{} needs a description",
                workout.name
            );
        }
    }

    /// The descriptions lead with a duration, so it has to be the real one.
    #[test]
    fn described_duration_matches_the_steps() {
        for workout in default_workouts(Utc::now()) {
            let minutes = workout.duration_seconds() / 60;
            assert_eq!(
                workout.duration_seconds() % 60,
                0,
                "{} should be a whole number of minutes",
                workout.name
            );
            assert!(
                workout
                    .description
                    .starts_with(&format!("{minutes} min · ")),
                "{} is {minutes} min but says {:?}",
                workout.name,
                workout.description
            );
        }
    }

    #[test]
    fn ordering_puts_the_test_first_and_holds_through_a_sort() {
        let mut workouts = default_workouts(Utc::now());
        assert_eq!(workouts[0].name, "FTP Test (20 min)");
        let expected: Vec<String> = workouts.iter().map(|it| it.name.clone()).collect();
        workouts.sort_by_key(|it| std::cmp::Reverse(it.updated_at));
        let sorted: Vec<String> = workouts.iter().map(|it| it.name.clone()).collect();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn recognizes_only_an_untouched_legacy_sample() {
        let mut sample = Workout::new("FTP Builder", legacy_sample_steps());
        assert!(is_legacy_sample(&sample));
        sample.steps.push(steady(60, 50));
        assert!(!is_legacy_sample(&sample));
        let renamed = Workout::new("My FTP Builder", legacy_sample_steps());
        assert!(!is_legacy_sample(&renamed));
    }

    #[test]
    fn none_of_the_defaults_look_like_the_legacy_sample() {
        for workout in default_workouts(Utc::now()) {
            assert!(!is_legacy_sample(&workout));
        }
    }
}
