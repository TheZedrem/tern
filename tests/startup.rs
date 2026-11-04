//! Startup behavior of the built binary (spec 0001).

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn zed_term() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zed-term"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs `command` to its exit. A hang fails the test instead of stalling it:
/// the child is killed by its own handle, never by name.
fn output_within(mut command: Command, timeout: Duration) -> Output {
    let mut child = command.spawn().expect("start zed-term");
    let deadline = Instant::now() + timeout;
    while child.try_wait().expect("poll zed-term").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("zed-term still running after {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().expect("read zed-term output")
}

/// AC7: no Wayland session means a clear message and exit code 1, not a hang.
#[cfg(target_os = "linux")]
#[test]
fn without_wayland_it_says_so_and_exits_with_1() {
    let mut command = zed_term();
    command.env_remove("WAYLAND_DISPLAY");
    let output = output_within(command, Duration::from_secs(10));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("zed-term needs a Wayland session (X11 is not supported)"),
        "stderr: {stderr}"
    );
}

/// AC2: one JSON line with all six marks, exit code 0, no interaction.
#[test]
#[ignore = "needs a Wayland session"]
fn bench_startup_prints_one_json_line_with_all_six_marks() {
    let mut command = zed_term();
    command.arg("--bench-startup");
    let output = output_within(command, Duration::from_secs(30));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");

    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout}");
    let report: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(lines[0]).expect("a JSON object");
    let mark = |name: &str| {
        report
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("no {name} mark in {stdout}"))
    };
    let names = [
        "config",
        "pty",
        "platform",
        "window",
        "first_byte",
        "rendered",
    ];
    let [config, pty, platform, window, first_byte, rendered] = names.map(mark);

    // The boot path's order (spec 0001, Flows); the shell's first output
    // may land anywhere before the frame that draws it.
    assert!(
        config <= pty && pty <= platform && platform <= window && window <= rendered,
        "{stdout}"
    );
    assert!(first_byte <= rendered, "{stdout}");
}
