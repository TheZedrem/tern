//! One forward's `ssh -N` (spec 0017, Model): started with stdout to null
//! and stderr on a pipe that a thread of its own reads to the end, keeping
//! only its last KiB (TM-fwd-15). Tern holds the [`Child`] and stops it
//! with kill and wait on that handle, never by PID or name (TM-fwd-14).
//! Dropping a [`Process`] stops it.

use crate::terminal::clean_title;
use crate::terminal::pty::Command;
use futures::channel::oneshot;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The most of ssh's stderr kept while it runs: its last line is the
/// reason, cut at 1 KiB (TM-fwd-15).
const KEEP: usize = 1024;
/// How often the thread looks whether ssh has exited once its stderr
/// closed.
const EXIT_POLL: Duration = Duration::from_millis(50);

/// A running forward's ssh.
pub struct Process {
    child: Arc<Mutex<Child>>,
}

impl Process {
    /// Starts `command`. The receiver gets why ssh exited, once it has: its
    /// last stderr line as plain text by the title rules (TM-fwd-09), else
    /// its exit code. It never fires for a process stopped through
    /// [`Process::stop`] or dropped first.
    pub fn spawn(command: &Command) -> std::io::Result<(Self, oneshot::Receiver<String>)> {
        let mut ssh = std::process::Command::new(&command.program);
        ssh.args(&command.args)
            .envs(&command.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            ssh.creation_flags(crate::ssh::CREATE_NO_WINDOW);
        }
        // A quit stops every forward (spec 0017, flow 8); the OS ends ssh
        // when Tern dies any other way (TM-fwd-13).
        #[cfg(target_os = "linux")]
        dies_with_tern(&mut ssh);
        let mut child = ssh.spawn()?;
        #[cfg(windows)]
        if let Some(job) = run_job() {
            job.assign(&child);
        }
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let (exited, exit) = oneshot::channel();
        let watched = Arc::clone(&child);
        std::thread::Builder::new()
            .name("forward".into())
            .spawn(move || {
                let said = stderr.map(last_words).unwrap_or_default();
                // Polled, never a blocking wait under the lock: `stop` on
                // the UI thread takes the same lock.
                let code = loop {
                    let status = watched
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .try_wait();
                    match status {
                        Ok(Some(status)) => break status.code(),
                        Ok(None) => std::thread::sleep(EXIT_POLL),
                        Err(_) => break None,
                    }
                };
                let _ = exited.send(reason(&said, code));
            })?;
        Ok((Self { child }, exit))
    }

    /// Kills ssh and waits for it. A child already waited on isn't
    /// signalled again, so a reused PID is never hit.
    pub fn stop(&self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Has the kernel send ssh `SIGTERM` when the thread that spawns it ends.
/// Forwards are spawned on the UI thread, which ends only with Tern, so a
/// crash or `kill -9` ends them too (TM-fwd-13).
#[cfg(target_os = "linux")]
fn dies_with_tern(ssh: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: getpid is async-signal-safe, called here before the fork.
    let tern = unsafe { libc::getpid() };
    // SAFETY: prctl and getppid are async-signal-safe, as `pre_exec`
    // requires, and touch nothing of the parent's.
    unsafe {
        ssh.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Tern died before the signal was set: nothing would send it.
            if libc::getppid() != tern {
                return Err(std::io::Error::other("Tern is gone"));
            }
            Ok(())
        });
    }
}

/// The job every forward of this run joins, which Windows closes, ending
/// them, when Tern is gone (TM-fwd-13). `None` if Windows refused one: a
/// quit still stops them.
#[cfg(windows)]
fn run_job() -> Option<&'static crate::job::Job> {
    static JOB: std::sync::OnceLock<Option<crate::job::Job>> = std::sync::OnceLock::new();
    JOB.get_or_init(crate::job::Job::new).as_ref()
}

/// Reads `stderr` until it closes, keeping its last [`KEEP`] bytes, so ssh
/// never blocks on a full pipe and a flood holds no memory.
fn last_words(mut stderr: impl Read) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0; 1024];
    while let Ok(read @ 1..) = stderr.read(&mut buffer) {
        kept.extend_from_slice(&buffer[..read]);
        if kept.len() > 2 * KEEP {
            kept.drain(..kept.len() - KEEP);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// How many of ssh's last lines a reason shows: a failed bind ends on
/// "Could not request local forwarding.", two lines after the one that
/// names the port.
const REASON_LINES: usize = 3;

/// Why a forward failed: the last [`REASON_LINES`] lines ssh wrote, joined
/// with ` · `, cleaned and cut to 200 characters by the title rules (spec
/// 0003), else its exit code.
pub fn reason(said: &str, code: Option<i32>) -> String {
    let mut lines: Vec<&str> = said
        .lines()
        .rev()
        .filter(|line| !clean_title(line).is_empty())
        .take(REASON_LINES)
        .collect();
    lines.reverse();
    let said = clean_title(&lines.join(" · "));
    match code {
        _ if !said.is_empty() => said,
        Some(code) => format!("ssh exited with code {code}"),
        None => "ssh was stopped".to_owned(),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// What OpenSSH writes when a local forward's port is taken (flow 5).
    const BIND_FAILED: &str = "bind [127.0.0.1]:15432: Address already in use\n\
        channel_setup_fwd_listener_tcpip: cannot listen to port: 15432\n\
        Could not request local forwarding.\n";

    /// Flow 5: a failed bind ends on "Could not request local
    /// forwarding.", two lines after the one naming the port, so the
    /// reason's three lines reach back to it.
    #[test]
    fn a_failed_binds_reason_names_the_port_that_is_taken() {
        let said = format!("debug1: Connecting to bastion\nWarning: earlier noise\n{BIND_FAILED}");
        assert_eq!(
            reason(&said, Some(255)),
            "bind [127.0.0.1]:15432: Address already in use · \
             channel_setup_fwd_listener_tcpip: cannot listen to port: 15432 · \
             Could not request local forwarding."
        );
    }

    /// Flow 5: the last three lines that say something, skipping blank
    /// ones and ones that are nothing but control characters.
    #[test]
    fn the_reason_is_the_last_three_non_empty_lines() {
        let said = "one\r\ntwo\r\n\r\nthree\n \n\x1b\x07\nfour\n\n";
        assert_eq!(reason(said, Some(255)), "two · three · four");
        assert_eq!(reason("only line", Some(255)), "only line");
    }

    /// TM-fwd-09: stderr is shown as plain text, so control, direction and
    /// invisible characters can't reorder or hide what it says.
    #[test]
    fn the_reason_holds_no_control_or_direction_characters() {
        let said = "Run \u{202E}hs.lieve\u{202C} now\x1b[2J\u{200B}\u{2066}x\u{2069}\u{FEFF}\x00\n";
        let shown = reason(said, Some(255));
        assert!(!shown.is_empty());
        let hidden = |c: char| {
            c.is_control()
                || matches!(
                    c,
                    '\u{061C}'
                        | '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        };
        assert!(!shown.chars().any(hidden), "{shown:?}");
    }

    /// TM-fwd-09 and flow 5: at most 200 characters, however long the
    /// lines.
    #[test]
    fn the_reason_is_at_most_200_characters() {
        let line = "é".repeat(1000);
        let said = format!("{line}\n{line}\n{line}\n");
        assert_eq!(reason(&said, Some(255)).chars().count(), 200);
    }

    /// Flow 5: ssh that said nothing is named by its exit code.
    #[test]
    fn without_stderr_the_reason_is_the_exit_code() {
        assert!(reason("", Some(255)).contains("255"));
        assert!(reason("\n \n\x1b\n", Some(1)).contains('1'));
        assert!(!reason("", None).is_empty());
    }

    #[cfg(unix)]
    pub(crate) mod spawned {
        use super::super::*;
        use std::collections::BTreeMap;
        use std::time::Instant;

        /// A fake ssh: `sh -c script`.
        pub(crate) fn sh(script: &str) -> Command {
            Command {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                env: BTreeMap::new(),
            }
        }

        /// The exit reason, or `None` if none came within `limit`.
        fn wait_for(exit: &mut oneshot::Receiver<String>, limit: Duration) -> Option<String> {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline {
                if let Ok(Some(reason)) = exit.try_recv() {
                    return Some(reason);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }

        /// Flow 5: ssh exiting tells the forward why, from its stderr.
        #[test]
        fn an_exit_reports_the_last_stderr_lines() {
            let script = format!(
                "printf '{}' >&2; exit 255",
                super::BIND_FAILED.replace('\n', "\\n")
            );
            let (_process, mut exit) = Process::spawn(&sh(&script)).expect("sh runs");
            let reason = wait_for(&mut exit, Duration::from_secs(5)).expect("an exit reason");
            assert!(
                reason.starts_with("bind [127.0.0.1]:15432: Address already in use"),
                "{reason}"
            );
        }

        /// Flow 5: ssh that wrote nothing is named by its exit code.
        #[test]
        fn a_silent_exit_reports_its_code() {
            let (_process, mut exit) = Process::spawn(&sh("exit 3")).expect("sh runs");
            let reason = wait_for(&mut exit, Duration::from_secs(5)).expect("an exit reason");
            assert!(reason.contains('3'), "{reason}");
        }

        /// TM-fwd-15: an endless stderr can't block ssh on a full pipe or
        /// grow the reason: megabytes of it still end in a short reason
        /// with the last line.
        #[test]
        fn a_stderr_flood_neither_blocks_ssh_nor_grows_the_reason() {
            let script = "yes flood | head -n 800000 >&2; echo done >&2; exit 1";
            let (_process, mut exit) = Process::spawn(&sh(script)).expect("sh runs");
            let reason = wait_for(&mut exit, Duration::from_secs(10)).expect("an exit reason");
            assert!(
                reason.chars().count() <= 200,
                "{} characters",
                reason.chars().count()
            );
            assert_eq!(reason, "flood · flood · done");
        }

        /// Flow 6 and TM-fwd-14: stopping kills ssh through the handle kept
        /// from spawn; the PID it had is gone afterwards.
        #[test]
        fn stopping_or_dropping_ends_ssh() {
            let dir = crate::config::tests::TempDir::new("forward-process-stop");
            for drop_it in [false, true] {
                let pidfile = dir.path().join(format!("pid-{drop_it}"));
                let script = format!("echo $$ > '{}'; exec sleep 30", pidfile.display());
                let (process, _exit) = Process::spawn(&sh(&script)).expect("sh runs");
                let pid = read_pid(&pidfile);
                assert!(alive(&pid), "ssh {pid} runs before it's stopped");
                if drop_it {
                    drop(process);
                } else {
                    process.stop();
                }
                assert!(!alive(&pid), "ssh {pid} still runs");
            }
        }

        /// TM-fwd-13: the kernel ends ssh when the thread that started it
        /// ends, which in Tern is the UI thread, so a crash or `kill -9`
        /// takes every forward with it. ssh runs as long as that thread
        /// does.
        #[cfg(target_os = "linux")]
        #[test]
        fn ssh_ends_with_the_thread_that_started_it() {
            let dir = crate::config::tests::TempDir::new("forward-process-pdeathsig");
            let pidfile = dir.path().join("pid");
            let script = format!("echo $$ > '{}'; exec sleep 60", pidfile.display());
            let (release, released) = std::sync::mpsc::channel::<()>();
            let starter = std::thread::spawn(move || {
                let started = Process::spawn(&sh(&script)).expect("sh runs");
                let _ = released.recv();
                started
            });
            // Held until the end so a failure still stops ssh, through the
            // handle and never by PID.
            let (_process, _exit) = {
                let pid = read_pid(&pidfile);
                std::thread::sleep(Duration::from_millis(200));
                assert!(alive(&pid), "ssh {pid} ended while its thread runs");
                release.send(()).expect("the starter waits");
                let started = starter.join().expect("the starter returns ssh");
                let deadline = Instant::now() + Duration::from_secs(5);
                while alive(&pid) {
                    assert!(
                        Instant::now() < deadline,
                        "ssh {pid} outlived the thread that started it"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                started
            };
        }

        /// The PID the fake ssh wrote to `pidfile`, once it has.
        pub(crate) fn read_pid(pidfile: &std::path::Path) -> String {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(pid) = std::fs::read_to_string(pidfile)
                    && pid.ends_with('\n')
                {
                    return pid.trim().to_owned();
                }
                assert!(Instant::now() < deadline, "no PID in {}", pidfile.display());
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Whether `pid` is a process: `kill -0` only asks, it signals
        /// nothing.
        pub(crate) fn alive(pid: &str) -> bool {
            std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }
    }
}
