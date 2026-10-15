//! Per-device log: a bounded ring of human-readable lines that the Devices hub
//! can show at any time, not only while a connection dialog is open.

use std::{collections::VecDeque, sync::Mutex};

use chrono::Utc;
use serde::Serialize;

/// How many lines each device keeps. Old lines fall off the front.
pub const CAPACITY: usize = 200;

/// How many lines a snapshot carries to the UI by default.
pub const TAIL: usize = 50;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLogLine {
    /// Unix epoch milliseconds.
    pub at_ms: i64,
    /// "info" while working, "ok" when a stage completed, "warn" for non-fatal
    /// skips, "error" when something failed.
    pub level: &'static str,
    pub step: String,
    pub detail: Option<String>,
    /// True for lines produced during a connection attempt; the trainer's
    /// connect dialog only shows these.
    pub connect: bool,
}

#[derive(Debug, Default)]
pub struct DeviceLog {
    lines: Mutex<VecDeque<DeviceLogLine>>,
}

impl DeviceLog {
    pub fn push(
        &self,
        level: &'static str,
        step: &str,
        detail: Option<String>,
        connect: bool,
    ) -> DeviceLogLine {
        let line = DeviceLogLine {
            at_ms: Utc::now().timestamp_millis(),
            level,
            step: step.to_string(),
            detail,
            connect,
        };
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line.clone());
        line
    }

    /// The most recent `count` lines, oldest first.
    pub fn tail(&self, count: usize) -> Vec<DeviceLogLine> {
        let lines = self
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let skip = lines.len().saturating_sub(count);
        lines.iter().skip(skip).cloned().collect()
    }

    pub fn all(&self) -> Vec<DeviceLogLine> {
        self.tail(CAPACITY)
    }

    #[cfg(test)]
    pub fn clear(&self) {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_lines_in_order() {
        let log = DeviceLog::default();
        log.push("info", "one", None, true);
        log.push("ok", "two", Some("detail".into()), false);
        let lines = log.all();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].step, "one");
        assert!(lines[0].connect);
        assert_eq!(lines[1].detail.as_deref(), Some("detail"));
        assert!(lines[0].at_ms <= lines[1].at_ms);
    }

    #[test]
    fn drops_oldest_past_capacity() {
        let log = DeviceLog::default();
        for index in 0..(CAPACITY + 25) {
            log.push("info", &format!("line {index}"), None, false);
        }
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log.all()[0].step, "line 25");
        assert_eq!(
            log.all()[CAPACITY - 1].step,
            format!("line {}", CAPACITY + 24)
        );
    }

    #[test]
    fn tail_returns_newest() {
        let log = DeviceLog::default();
        for index in 0..10 {
            log.push("info", &format!("{index}"), None, false);
        }
        let tail = log.tail(3);
        assert_eq!(
            tail.iter()
                .map(|line| line.step.as_str())
                .collect::<Vec<_>>(),
            ["7", "8", "9"]
        );
        assert_eq!(log.tail(100).len(), 10);
    }

    #[test]
    fn clear_empties() {
        let log = DeviceLog::default();
        log.push("info", "x", None, false);
        log.clear();
        assert!(log.is_empty());
    }
}
