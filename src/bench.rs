//! Startup benchmark (spec 0001). `zed-term --bench-startup` is one
//! measured launch that prints its marks as JSON and quits after the first
//! frame. `zed-term --bench-startup N` re-runs this binary that way once
//! cold, then N times warm, and prints the median and p90 of every mark.

use crate::timing::TtfpReport;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The startup budget, warm median (architecture §Cross-cutting rules).
pub const BUDGET_MS: f64 = 500.0;

/// A hung launch is killed after this long, so the bench never hangs.
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Normal interactive start.
    Normal,
    /// One measured launch: `--bench-startup`.
    Once,
    /// The runner: `--bench-startup N`.
    Runner { runs: usize },
    /// `--import-hosts <file> [--dry-run]`: imports and exits, no window
    /// (spec 0010).
    Import { file: PathBuf, dry_run: bool },
}

/// How `--import-hosts` is called.
const IMPORT_USAGE: &str = "usage: zed-term --import-hosts <file> [--dry-run]";

impl Mode {
    /// Parses the arguments after the program name.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut args = args.into_iter().peekable();
        let mode = match args.next().as_deref() {
            None => Mode::Normal,
            Some("--bench-startup") => match args.next() {
                None => Mode::Once,
                Some(runs) => match runs.parse::<usize>() {
                    Ok(runs) if runs > 0 => Mode::Runner { runs },
                    _ => {
                        return Err(format!(
                            "--bench-startup takes a run count of 1 or more, not {runs:?}"
                        ));
                    }
                },
            },
            Some("--import-hosts") => match args.next() {
                Some(file) if !file.starts_with("--") => Mode::Import {
                    file: file.into(),
                    dry_run: args.next_if(|arg| arg == "--dry-run").is_some(),
                },
                _ => return Err(IMPORT_USAGE.to_owned()),
            },
            Some(other) => return Err(format!("unknown argument {other:?}")),
        };
        match args.next() {
            None => Ok(mode),
            Some(extra) => Err(format!("unexpected argument {extra:?}")),
        }
    }
}

/// Runs the bench and prints the table. Returns the process exit code.
pub fn run(runs: usize) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("zed-term: cannot find own executable: {error}");
            return 1;
        }
    };
    let mut reports = Vec::with_capacity(runs + 1);
    for run in 0..=runs {
        match launch(&exe) {
            Ok(report) => reports.push(report),
            Err(error) => {
                eprintln!("zed-term: bench run {run} failed: {error}");
                return 1;
            }
        }
    }
    let (cold, warm) = reports.split_first().expect("at least one run");

    println!("mark          cold   median      p90   (warm, n={runs})");
    let fmt = |ms: Option<f64>| ms.map_or_else(|| "n/a".to_owned(), |ms| format!("{ms:.1}"));
    for (index, (name, cold_ms)) in cold.marks().into_iter().enumerate() {
        let values: Vec<f64> = warm.iter().filter_map(|r| r.marks()[index].1).collect();
        println!(
            "{name:<11} {:>7}  {:>7}  {:>7}",
            fmt(cold_ms),
            fmt(percentile(&values, 0.5)),
            fmt(percentile(&values, 0.9)),
        );
    }
    let rendered: Vec<f64> = warm.iter().filter_map(|r| r.rendered).collect();
    if let Some(median) = percentile(&rendered, 0.5) {
        let verdict = if median < BUDGET_MS {
            "OK"
        } else {
            "OVER BUDGET"
        };
        println!("\nTTFP warm median {median:.1} ms (budget < {BUDGET_MS} ms): {verdict}");
    }
    0
}

/// One `--bench-startup` launch of `exe`, killed if it hangs.
fn launch(exe: &Path) -> Result<TtfpReport, String> {
    let mut child = Command::new(exe)
        .arg("--bench-startup")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            // Our own child, by handle: never a process we didn't start.
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("no report within {} s", RUN_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(|e| e.to_string())?;
    }
    if !status.success() {
        return Err(format!("exited with {status}"));
    }
    parse_report(&stdout)
}

/// The last JSON line of a launch's stdout.
pub fn parse_report(stdout: &str) -> Result<TtfpReport, String> {
    let line = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with('{'))
        .ok_or("no report on stdout")?;
    serde_json::from_str(line).map_err(|e| format!("bad report {line:?}: {e}"))
}

/// The `p`-quantile (0 to 1) by the nearest-rank method; `None` when empty.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (p * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Mode, String> {
        Mode::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn arguments_select_the_mode() {
        assert_eq!(parse(&[]), Ok(Mode::Normal));
        assert_eq!(parse(&["--bench-startup"]), Ok(Mode::Once));
        assert_eq!(
            parse(&["--bench-startup", "10"]),
            Ok(Mode::Runner { runs: 10 })
        );
    }

    #[test]
    fn bad_arguments_are_rejected() {
        let cases: [&[&str]; 6] = [
            &["--bench-startup", "0"],
            &["--bench-startup", "ten"],
            &["--bench-startup", "-1"],
            &["--bench-startup", "10", "extra"],
            &["--verbose"],
            &["ready"],
        ];
        for args in cases {
            assert!(parse(args).is_err(), "accepted {args:?}");
        }
    }

    /// Spec 0010, flow 1.
    #[test]
    fn import_hosts_takes_a_file_and_an_optional_dry_run() {
        let import = |file: &str, dry_run| {
            Ok(Mode::Import {
                file: file.into(),
                dry_run,
            })
        };
        assert_eq!(parse(&["--import-hosts", "a.ini"]), import("a.ini", false));
        assert_eq!(
            parse(&["--import-hosts", "a.ini", "--dry-run"]),
            import("a.ini", true)
        );
        let bad: [&[&str]; 4] = [
            &["--import-hosts"],
            &["--import-hosts", "--dry-run"],
            &["--import-hosts", "a.ini", "--dry-run", "extra"],
            &["--import-hosts", "a.ini", "b.ini"],
        ];
        for args in bad {
            assert!(parse(args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn percentile_is_the_nearest_rank() {
        let ten: Vec<f64> = (1..=10).rev().map(f64::from).collect();
        assert_eq!(percentile(&ten, 0.5), Some(5.0));
        assert_eq!(percentile(&ten, 0.9), Some(9.0));
        let four = [40.0, 10.0, 30.0, 20.0];
        assert_eq!(percentile(&four, 0.5), Some(20.0));
        assert_eq!(percentile(&four, 0.9), Some(40.0));
        assert_eq!(percentile(&[7.0], 0.9), Some(7.0));
    }

    #[test]
    fn percentile_of_no_runs_is_none() {
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn the_report_is_the_last_json_line_with_the_spec_mark_names() {
        let stdout = "\
            {\"config\":9,\"pty\":9,\"platform\":9,\"window\":9,\"first_byte\":9,\"rendered\":9}\n\
            some output\n\
            {\"config\":1,\"pty\":2,\"platform\":3,\"window\":4,\"first_byte\":5,\"rendered\":6}\n";
        let report = parse_report(stdout).expect("report");
        let marks = report.marks().map(|(_, ms)| ms);
        let expected = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0].map(Some);
        assert_eq!(marks, expected);
    }

    #[test]
    fn a_missing_or_garbled_report_is_an_error() {
        for stdout in ["", "ready\n", "{\"config\": 1.0, \"pty\":\n"] {
            assert!(parse_report(stdout).is_err(), "accepted {stdout:?}");
        }
    }
}
