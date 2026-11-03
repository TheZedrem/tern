//! PTY lifecycle (spec 0002): spawn, input, resize, output streaming, exit.
//!
//! Each pane gets three threads, so no Tauri command ever blocks on PTY I/O:
//! - reader: PTY output → `Output` (which applies backpressure),
//! - writer: an ordered input queue → PTY, so a large paste into a full
//!   buffer blocks only this thread,
//! - waiter: waits for the child and hands its exit code to the reader.
//!
//! PTY bytes are never logged (AGENTS.md: they carry secrets).

use crate::output::{Output, Sink};
use crate::timing::{Mark, Timing};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;

pub type PaneId = u32;

type Master = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

struct Pane<S> {
    master: Master,
    input: mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    output: Arc<Output<S>>,
}

pub struct PtyManager<S> {
    panes: Mutex<HashMap<PaneId, Pane<S>>>,
    next_id: AtomicU32,
    timing: Arc<Timing>,
}

/// The command a pane runs when its profile doesn't set one: the login shell
/// on Unix, else the first of `pwsh`, `powershell`, `cmd` on Windows (OQ-003).
pub fn default_command() -> CommandBuilder {
    #[cfg(unix)]
    {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg("-l");
        cmd
    }
    #[cfg(windows)]
    {
        let found = ["pwsh.exe", "powershell.exe"]
            .into_iter()
            .find(|exe| on_path(exe));
        CommandBuilder::new(found.unwrap_or("cmd.exe"))
    }
}

#[cfg(windows)]
fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(exe).is_file()))
}

/// The bench-mode command: prints one line and exits, so `--bench-startup`
/// measures zed-term rather than the user's dotfiles (spec 0001).
pub fn bench_command() -> CommandBuilder {
    #[cfg(unix)]
    let mut cmd = CommandBuilder::new("sh");
    #[cfg(unix)]
    cmd.args(["-c", "echo ready"]);
    #[cfg(windows)]
    let mut cmd = CommandBuilder::new("cmd.exe");
    #[cfg(windows)]
    cmd.args(["/c", "echo ready"]);
    cmd
}

/// Adds the environment every pane gets on top of the inherited one, and
/// starts in `cwd` or the home directory.
pub fn prepare(cmd: &mut CommandBuilder, cwd: Option<&std::path::Path>) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "zed-term");
    if let Some(dir) = cwd.map(Into::into).or_else(dirs::home_dir) {
        cmd.cwd(dir);
    }
}

/// Human-readable command line, for spawn-failure messages.
pub fn describe(cmd: &CommandBuilder) -> String {
    cmd.get_argv()
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

impl<S: Sink> PtyManager<S> {
    pub fn new(timing: Arc<Timing>) -> Self {
        Self {
            panes: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            timing,
        }
    }

    fn panes(&self) -> MutexGuard<'_, HashMap<PaneId, Pane<S>>> {
        self.panes.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Spawns `cmd` in a new PTY. Output buffers until a sink is attached.
    pub fn spawn(&self, cmd: CommandBuilder, cols: u16, rows: u16) -> Result<PaneId, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not open a PTY: {e}"))?;
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| e.to_string())?;
        // Our copy of the slave must close, or the reader never sees EOF.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let killer = child.clone_killer();
        let master: Master = Arc::new(Mutex::new(Some(pair.master)));
        let output = Arc::new(Output::default());
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let (exit_tx, exit_rx) = mpsc::channel::<Option<u32>>();

        thread::spawn(move || {
            for data in input_rx {
                if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        });

        let waiter_master = Arc::clone(&master);
        thread::spawn(move || {
            let code = child
                .wait()
                .ok()
                .and_then(|s| s.signal().is_none().then(|| s.exit_code()));
            // ConPTY keeps the output pipe open after the child exits; the
            // reader only sees EOF once the pseudoconsole is closed. On Unix
            // closing the master would discard unread output, so keep it.
            #[cfg(windows)]
            drop(waiter_master.lock().map(|mut m| m.take()));
            #[cfg(not(windows))]
            drop(waiter_master);
            let _ = exit_tx.send(code);
        });

        let reader_output = Arc::clone(&output);
        let timing = Arc::clone(&self.timing);
        thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        timing.mark(Mark::FirstByte);
                        reader_output.push(&buf[..n]);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    // EIO on Linux once the child is gone: treat as EOF.
                    Err(_) => break,
                }
            }
            reader_output.finish(exit_rx.recv().ok().flatten());
        });

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.panes().insert(
            id,
            Pane {
                master,
                input,
                killer,
                output,
            },
        );
        Ok(id)
    }

    fn with_pane<T>(&self, id: PaneId, f: impl FnOnce(&Pane<S>) -> T) -> Result<T, String> {
        self.panes()
            .get(&id)
            .map(f)
            .ok_or_else(|| format!("no pane {id}"))
    }

    pub fn attach(&self, id: PaneId, sink: S) -> Result<(), String> {
        self.with_pane(id, |p| p.output.attach(sink))
    }

    pub fn ack(&self, id: PaneId, bytes: usize) -> Result<(), String> {
        self.with_pane(id, |p| p.output.ack(bytes))
    }

    /// Queues input; never blocks the caller.
    pub fn write(&self, id: PaneId, data: Vec<u8>) -> Result<(), String> {
        self.with_pane(id, |p| p.input.send(data).map_err(|_| "process has exited".to_string()))?
    }

    pub fn resize(&self, id: PaneId, cols: u16, rows: u16) -> Result<(), String> {
        self.with_pane(id, |p| {
            let master = p.master.lock().unwrap_or_else(|e| e.into_inner());
            match master.as_ref() {
                Some(m) => m
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|e| e.to_string()),
                None => Ok(()), // already exited (Windows closed it)
            }
        })?
    }

    /// Kills every child process. Called on app exit so no shell outlives
    /// the window.
    pub fn kill_all(&self) {
        for pane in self.panes().values_mut() {
            let _ = pane.killer.kill();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[derive(Clone, Default)]
    struct Recorder {
        bytes: Arc<Mutex<Vec<u8>>>,
        exit: Arc<Mutex<Option<Option<u32>>>>,
    }

    impl Sink for Recorder {
        fn bytes(&self, data: Vec<u8>) {
            self.bytes.lock().unwrap().extend(data);
        }
        fn exit(&self, code: Option<u32>) {
            *self.exit.lock().unwrap() = Some(code);
        }
    }

    #[test]
    fn spawned_command_streams_output_and_reports_exit_code() {
        let manager = PtyManager::new(Arc::new(Timing::new(Instant::now())));
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "printf hi; exit 3"]);
        let id = manager.spawn(cmd, 80, 24).expect("spawn");

        let rec = Recorder::default();
        manager.attach(id, rec.clone()).expect("attach");

        let deadline = Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(code) = *rec.exit.lock().unwrap() {
                break code;
            }
            assert!(Instant::now() < deadline, "no exit notice within 5 s");
            thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(exit, Some(3));
        let bytes = rec.bytes.lock().unwrap();
        assert!(
            bytes.windows(2).any(|w| w == b"hi"),
            "output missing 'hi': {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}
