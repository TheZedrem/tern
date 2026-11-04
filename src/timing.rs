//! Time-to-first-prompt marks (spec 0001). Every mark is a duration since
//! `main()` entry, recorded once; later calls for the same mark are ignored,
//! so nothing after boot can overwrite the boot numbers.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Mark {
    /// Config (and later session) read.
    Config,
    /// Active tab's PTYs spawned.
    Pty,
    /// GPUI platform up: Wayland connection and system fonts.
    Platform,
    /// Window and GPU device created.
    Window,
    /// First PTY output parsed.
    FirstByte,
    /// First frame with output drawn.
    Rendered,
}

const MARKS: usize = 6;

pub struct Timing {
    start: Instant,
    marks: [OnceLock<Duration>; MARKS],
}

/// Milliseconds since `main()` per mark; `None` if the mark was never reached.
/// Printed as one JSON line, which the bench runner parses back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TtfpReport {
    pub config: Option<f64>,
    pub pty: Option<f64>,
    pub platform: Option<f64>,
    pub window: Option<f64>,
    pub first_byte: Option<f64>,
    pub rendered: Option<f64>,
}

impl TtfpReport {
    /// Mark names and values in boot order, for printing tables.
    pub fn marks(&self) -> [(&'static str, Option<f64>); MARKS] {
        [
            ("config", self.config),
            ("pty", self.pty),
            ("platform", self.platform),
            ("window", self.window),
            ("first_byte", self.first_byte),
            ("rendered", self.rendered),
        ]
    }
}

impl Timing {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            marks: Default::default(),
        }
    }

    /// Records `mark` now. Returns `true` only the first time it is recorded.
    pub fn mark(&self, mark: Mark) -> bool {
        self.marks[mark as usize].set(self.start.elapsed()).is_ok()
    }

    pub fn report(&self) -> TtfpReport {
        let ms = |m: Mark| {
            self.marks[m as usize]
                .get()
                .map(|d| d.as_secs_f64() * 1000.0)
        };
        TtfpReport {
            config: ms(Mark::Config),
            pty: ms(Mark::Pty),
            platform: ms(Mark::Platform),
            window: ms(Mark::Window),
            first_byte: ms(Mark::FirstByte),
            rendered: ms(Mark::Rendered),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_is_recorded_once_and_reports_only_reached_marks() {
        let timing = Timing::new(Instant::now());
        assert!(timing.mark(Mark::Pty));
        let first = timing.report().pty.expect("pty mark reached");

        // A second call is ignored and keeps the first value.
        assert!(!timing.mark(Mark::Pty));
        assert_eq!(timing.report().pty, Some(first));

        let report = timing.report();
        assert_eq!(report.config, None);
        assert_eq!(report.window, None);
        assert_eq!(report.first_byte, None);
        assert_eq!(report.rendered, None);
    }
}
