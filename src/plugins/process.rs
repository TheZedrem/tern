//! One plugin's process (spec 0016, Module surface): spawned as an argv
//! with only its three pipes (TM-plug-14, -15), each pipe on a thread of
//! its own so a plugin that floods or never reads blocks itself, not Tern
//! (TM-plug-10). Tern holds the [`Child`] and ends the plugin's whole
//! process tree through it, never by PID or name (OQ-035, TM-plug-11).
//! Dropping a [`Process`] kills it.

use super::protocol::{Action, log_line};
use futures::SinkExt;
use futures::channel::mpsc::{Receiver, Sender, channel};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// A longer line is dropped (spec 0016, Bounds).
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// How many messages wait for the UI thread before the plugin's readers
/// block.
const WAITING: usize = 64;
/// How often the writer thread looks whether the plugin has exited.
const EXIT_POLL: Duration = Duration::from_millis(50);

/// What a plugin's threads tell the host.
#[derive(Debug)]
pub enum Message {
    /// A line of stdout: an action, or why the line is dropped.
    Line(Result<Action, String>),
    /// A line of stderr, cleaned.
    Log(String),
    /// The event handed to [`Process::write`] is in the plugin's pipe, or
    /// the pipe is closed.
    Written,
    /// The process ended; `None`: killed by a signal.
    Exited(Option<i32>),
}

pub struct Process {
    child: Arc<Mutex<Child>>,
    /// The writer thread's inbox. Dropping it closes the plugin's stdin.
    stdin: Option<mpsc::Sender<String>>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Process {
    /// Starts `program` with `args` in `folder`, never through a shell.
    /// The receiver ends once the plugin's pipes are closed and it has
    /// exited.
    pub fn spawn(
        program: &Path,
        args: &[String],
        folder: &Path,
    ) -> std::io::Result<(Self, Receiver<Message>)> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::ssh::CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so its children end with it.
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = job::Job::holding(&child);
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let child = Arc::new(Mutex::new(child));
        let (messages, inbox) = channel(WAITING);
        let (lines, outbox) = mpsc::channel();
        let mut process = Self {
            child: Arc::clone(&child),
            stdin: Some(lines),
            #[cfg(windows)]
            job,
        };
        let threads = [
            thread("plugin-out", {
                let messages = messages.clone();
                move || read_lines(stdout, messages, |line| Message::Line(Action::parse(&line)))
            }),
            thread("plugin-err", {
                let messages = messages.clone();
                move || read_lines(stderr, messages, |line| Message::Log(log_line(&line)))
            }),
            thread("plugin-in", move || {
                write_lines(stdin, outbox, child, messages)
            }),
        ];
        if let Some(error) = threads.into_iter().find_map(Result::err) {
            process.kill();
            return Err(error);
        }
        Ok((process, inbox))
    }

    /// Writes one event's line to the plugin's stdin, on the writer
    /// thread; [`Message::Written`] follows.
    pub fn write(&self, line: String) {
        if let Some(stdin) = &self.stdin {
            let _ = stdin.send(line);
        }
    }

    /// Closes the plugin's stdin: the signal to stop (spec 0016, EOF).
    pub fn close(&mut self) {
        self.stdin = None;
    }

    /// Whether the process has ended.
    pub fn exited(&self) -> bool {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        !matches!(child.try_wait(), Ok(None))
    }

    /// Kills the plugin and what it started, and waits for it. A child
    /// already waited on isn't signalled again, so a reused PID is never
    /// hit.
    pub fn kill(&mut self) {
        self.close();
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(child.try_wait(), Ok(None)) {
            #[cfg(unix)]
            // SAFETY: the child is unreaped under this lock, so its PID,
            // which is its process group's ID, still names it.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            #[cfg(windows)]
            if let Some(job) = &self.job {
                job.kill();
            }
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

fn thread(name: &str, run: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(run)
        .map(drop)
}

/// Hands each line of `pipe` to the host until the pipe closes. The
/// channel is bounded: while the UI thread is busy, the plugin blocks on
/// its full pipe.
fn read_lines(
    pipe: Option<impl Read>,
    mut messages: Sender<Message>,
    message: impl Fn(String) -> Message,
) {
    let Some(pipe) = pipe else {
        return;
    };
    let mut pipe = BufReader::new(pipe);
    while let Some(line) = next_line(&mut pipe, MAX_LINE_BYTES) {
        let message = match line {
            Some(line) => message(line),
            None => Message::Line(Err("a line over 1 MiB".into())),
        };
        if futures::executor::block_on(messages.send(message)).is_err() {
            break;
        }
    }
}

/// The next line of `pipe` without its line break, `Some(None)` for a line
/// over `max` bytes, which is read past without being kept, and `None` at
/// the end.
fn next_line(pipe: &mut impl BufRead, max: usize) -> Option<Option<String>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let (used, ended) = {
            let buffer = match pipe.fill_buf() {
                Ok(buffer) if !buffer.is_empty() => buffer,
                // The end, or a broken pipe: what came before is a line.
                _ if line.is_empty() && !too_long => return None,
                _ => break,
            };
            let end = buffer.iter().position(|byte| *byte == b'\n');
            let part = &buffer[..end.unwrap_or(buffer.len())];
            if line.len() + part.len() > max {
                too_long = true;
                line = Vec::new();
            } else if !too_long {
                line.extend_from_slice(part);
            }
            (part.len() + usize::from(end.is_some()), end.is_some())
        };
        pipe.consume(used);
        if ended {
            break;
        }
    }
    if too_long {
        return Some(None);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(Some(String::from_utf8_lossy(&line).into_owned()))
}

/// Writes the lines of `outbox` to the plugin's stdin, closes it once the
/// outbox is dropped, and reports the plugin's exit. The exit is polled,
/// never a blocking wait under the lock: [`Process::kill`] takes the same
/// lock on the UI thread.
fn write_lines(
    mut stdin: Option<ChildStdin>,
    outbox: mpsc::Receiver<String>,
    child: Arc<Mutex<Child>>,
    mut messages: Sender<Message>,
) {
    let mut tell = |message| futures::executor::block_on(messages.send(message)).is_ok();
    loop {
        match outbox.recv_timeout(EXIT_POLL) {
            Ok(line) => {
                let written = stdin.as_mut().is_some_and(|stdin| {
                    stdin
                        .write_all(line.as_bytes())
                        .and_then(|()| stdin.write_all(b"\n"))
                        .and_then(|()| stdin.flush())
                        .is_ok()
                });
                // A plugin that closed its stdin gets nothing more.
                if !written {
                    stdin = None;
                }
                if !tell(Message::Written) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                stdin = None;
                std::thread::sleep(EXIT_POLL);
            }
        }
        let status = child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_wait();
        match status {
            Ok(None) => {}
            Ok(Some(status)) => {
                tell(Message::Exited(status.code()));
                return;
            }
            Err(_) => {
                tell(Message::Exited(None));
                return;
            }
        }
    }
}

/// A Job object holding one plugin's process tree (OQ-035): terminating
/// it ends the plugin's children too, and Windows does so by itself when
/// Tern is gone, a crash included.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub struct Job(HANDLE);

    impl Job {
        /// A job with `child` in it, or `None` if Windows refuses one:
        /// the plugin is then killed alone.
        pub fn holding(child: &Child) -> Option<Self> {
            // SAFETY: plain calls on handles this function owns or
            // borrows from `child`, which outlives them.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let assigned = AssignProcessToJobObject(job.0, child.as_raw_handle().cast());
                (set != 0 && assigned != 0).then_some(job)
            }
        }

        pub fn kill(&self) {
            // SAFETY: the handle is open until `self` drops.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by `holding` and is closed
            // once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Spec 0016 §Bounds: "A line over 1 MiB is dropped", and what a plugin
/// wrote around it still arrives. The processes themselves are tested
/// through the host.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Every line of `bytes`, read through a buffer of `buffer` bytes.
    fn lines(bytes: &[u8], buffer: usize, max: usize) -> Vec<Option<String>> {
        let mut pipe = BufReader::with_capacity(buffer, Cursor::new(bytes));
        std::iter::from_fn(|| next_line(&mut pipe, max)).collect()
    }

    fn kept(line: &str) -> Option<String> {
        Some(line.to_owned())
    }

    #[test]
    fn lines_come_one_by_one_without_their_line_break() {
        let read = lines(b"{\"action\":\"ready\"}\n\nsecond\n", 8192, 1024);
        assert_eq!(
            read,
            [kept("{\"action\":\"ready\"}"), kept(""), kept("second")]
        );
        assert_eq!(lines(b"", 8192, 1024), []);
    }

    /// Windows programs end their lines with CRLF.
    #[test]
    fn a_crlf_line_break_is_no_part_of_the_line() {
        let read = lines(b"first\r\nsecond\r\n\r\nthird\n", 8192, 1024);
        assert_eq!(
            read,
            [kept("first"), kept("second"), kept(""), kept("third")]
        );
    }

    /// A plugin that exits without ending its last line still said it.
    #[test]
    fn a_last_line_without_a_line_break_is_a_line() {
        assert_eq!(
            lines(b"first\nlast", 8192, 1024),
            [kept("first"), kept("last")]
        );
        assert_eq!(lines(b"only", 8192, 1024), [kept("only")]);
    }

    /// A line arrives whole however the pipe hands it over.
    #[test]
    fn a_line_longer_than_the_pipes_buffer_arrives_whole() {
        let long = "x".repeat(1000);
        let text = format!("{long}\r\nshort\n{long}");
        for buffer in [1, 2, 7, 64, 999, 1000, 1001, 1002, 4096] {
            let read = lines(text.as_bytes(), buffer, 1024);
            assert_eq!(read, [kept(&long), kept("short"), kept(&long)], "{buffer}");
        }
    }

    /// Bounds: a line over the limit is dropped, not cut, and the lines
    /// around it are kept; one of exactly the limit isn't over.
    #[test]
    fn a_line_over_the_limit_is_dropped_and_the_next_one_is_read() {
        for buffer in [1, 5, 16, 17, 4096] {
            let read = lines(b"before\n0123456789abcdefX\nafter\n", buffer, 16);
            assert_eq!(read, [kept("before"), None, kept("after")], "{buffer}");
            let read = lines(b"0123456789abcdef\nafter\n", buffer, 16);
            assert_eq!(read, [kept("0123456789abcdef"), kept("after")], "{buffer}");
            // Many times over, and never ended.
            let flood = "y".repeat(1000);
            let read = lines(format!("before\n{flood}").as_bytes(), buffer, 16);
            assert_eq!(read, [kept("before"), None], "{buffer}");
        }
    }

    /// Text that isn't UTF-8 can't break the reader.
    #[test]
    fn bytes_that_are_not_utf8_still_make_a_line() {
        let read = lines(b"caf\xff\nnext\n", 8192, 1024);
        assert_eq!(read.len(), 2);
        assert_eq!(read[1], kept("next"));
    }

    /// Bounds and flow 4: the limit is 1 MiB. The line over it reaches the
    /// host as a dropped line, never as an action, and what the plugin
    /// wrote after it is still read.
    #[test]
    fn a_plugins_line_over_1_mib_is_dropped_and_its_next_action_arrives() {
        const MIB: usize = 1024 * 1024;
        let padding = |bytes: usize| " ".repeat(bytes - r#"{"action":"ready"}"#.len());
        // Valid JSON of exactly 1 MiB, then of one byte more.
        let fits = format!(r#"{{"action":{}"ready"}}"#, padding(MIB));
        let over = format!(r#"{{"action":{}"ready"}}"#, padding(MIB + 1));
        assert_eq!((fits.len(), over.len()), (MIB, MIB + 1));
        let stdout = format!("{fits}\n{over}\r\n{{\"action\":\"notify\",\"text\":\"after\"}}\n");

        let (messages, mut inbox) = channel(WAITING);
        read_lines(Some(Cursor::new(stdout.into_bytes())), messages, |line| {
            Message::Line(Action::parse(&line))
        });
        let mut said = Vec::new();
        while let Ok(Message::Line(line)) = inbox.try_recv() {
            said.push(line);
        }
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(said[0], Ok(Action::Ready));
        assert!(said[1].is_err(), "a line over 1 MiB was taken");
        assert_eq!(
            said[2],
            Ok(Action::Notify {
                text: "after".into()
            })
        );
    }
}
