//! Per-pane output delivery with backpressure (architecture §Key decisions).
//!
//! The PTY reader thread pushes every read here. Bytes go to the attached
//! sink (a Tauri Channel in the app), or are buffered while none is attached
//! (boot, or between a dev reload and the re-attach). Once `ACK_LIMIT` bytes
//! are outstanding, `push` blocks until the frontend acks, so a flood
//! throttles the program instead of growing xterm's buffer. The exit notice
//! is delivered after all output, so it can never overtake it.

use std::sync::{Condvar, Mutex, MutexGuard};

/// Unacknowledged bytes (sent or buffered) after which the reader blocks.
pub const ACK_LIMIT: usize = 256 * 1024;

/// Where a pane's output goes. Implemented by the Tauri Channel wrapper in
/// the app and by plain recorders in tests.
pub trait Sink: Send + 'static {
    fn bytes(&self, data: Vec<u8>);
    /// The process exited; `None` if it was killed by a signal.
    fn exit(&self, code: Option<u32>);
}

pub struct Output<S> {
    state: Mutex<State<S>>,
    acked: Condvar,
}

struct State<S> {
    sink: Option<S>,
    pending: Vec<u8>,
    unacked: usize,
    exit: Option<Option<u32>>,
}

impl<S: Sink> Default for Output<S> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                sink: None,
                pending: Vec::new(),
                unacked: 0,
                exit: None,
            }),
            acked: Condvar::new(),
        }
    }
}

impl<S: Sink> Output<S> {
    fn lock(&self) -> MutexGuard<'_, State<S>> {
        // A panic while holding the lock can't leave the state inconsistent
        // in a way that matters more than losing the pane's output.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Delivers or buffers `data`. Blocks while `ACK_LIMIT` bytes are unacked.
    pub fn push(&self, data: &[u8]) {
        let mut st = self.lock();
        while st.unacked >= ACK_LIMIT {
            st = self.acked.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.unacked += data.len();
        match &st.sink {
            Some(sink) => sink.bytes(data.to_vec()),
            None => st.pending.extend_from_slice(data),
        }
    }

    /// The frontend has consumed `n` bytes.
    pub fn ack(&self, n: usize) {
        let mut st = self.lock();
        st.unacked = st.unacked.saturating_sub(n);
        self.acked.notify_all();
    }

    /// Attaches a new sink, replacing any previous one (a dev reload orphans
    /// the old Channel). Flushes buffered output, then the exit notice if the
    /// process already ended. Bytes sent to a replaced sink are never acked,
    /// so the unacked count restarts from what this sink is about to receive.
    pub fn attach(&self, sink: S) {
        let mut st = self.lock();
        let pending = std::mem::take(&mut st.pending);
        st.unacked = pending.len();
        if !pending.is_empty() {
            sink.bytes(pending);
        }
        if let Some(code) = st.exit {
            sink.exit(code);
        }
        st.sink = Some(sink);
        self.acked.notify_all();
    }

    /// Called by the reader once the PTY reached EOF and the exit code is known.
    pub fn finish(&self, code: Option<u32>) {
        let mut st = self.lock();
        st.exit = Some(code);
        if let Some(sink) = &st.sink {
            sink.exit(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[derive(Debug, PartialEq, Clone)]
    enum Event {
        Bytes(Vec<u8>),
        Exit(Option<u32>),
    }

    /// Records every delivery in order; cloned handles share the log.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Event>>>);

    impl Recorder {
        fn events(&self) -> Vec<Event> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Sink for Recorder {
        fn bytes(&self, data: Vec<u8>) {
            self.0.lock().unwrap().push(Event::Bytes(data));
        }
        fn exit(&self, code: Option<u32>) {
            self.0.lock().unwrap().push(Event::Exit(code));
        }
    }

    /// Concatenated bytes, with the exit notice's position (index into events).
    fn flatten(events: &[Event]) -> (Vec<u8>, Option<usize>) {
        let mut bytes = Vec::new();
        let mut exit_at = None;
        for (i, ev) in events.iter().enumerate() {
            match ev {
                Event::Bytes(b) => bytes.extend_from_slice(b),
                Event::Exit(_) => exit_at = Some(i),
            }
        }
        (bytes, exit_at)
    }

    #[test]
    fn output_before_attach_is_delivered_in_order_then_exit() {
        let out = Output::<Recorder>::default();
        out.push(b"one ");
        out.push(b"two ");
        out.finish(Some(0));
        out.push(b"three");

        let rec = Recorder::default();
        out.attach(rec.clone());

        let events = rec.events();
        let (bytes, exit_at) = flatten(&events);
        assert_eq!(bytes, b"one two three");
        assert_eq!(exit_at, Some(events.len() - 1), "exit must come last");
        assert_eq!(events.last(), Some(&Event::Exit(Some(0))));
    }

    #[test]
    fn exit_after_attach_follows_all_bytes() {
        let out = Output::<Recorder>::default();
        let rec = Recorder::default();
        out.attach(rec.clone());
        out.push(b"bye");
        out.finish(None);

        assert_eq!(
            rec.events(),
            vec![Event::Bytes(b"bye".to_vec()), Event::Exit(None)]
        );
    }

    #[test]
    fn push_blocks_at_ack_limit_until_acked() {
        let out = Arc::new(Output::<Recorder>::default());
        let rec = Recorder::default();
        out.attach(rec.clone());
        out.push(&vec![0u8; ACK_LIMIT]);

        let (done_tx, done_rx) = mpsc::channel();
        let pusher = Arc::clone(&out);
        thread::spawn(move || {
            pusher.push(b"x");
            let _ = done_tx.send(());
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "push must block while ACK_LIMIT bytes are unacked"
        );
        assert_eq!(rec.events().len(), 1, "blocked bytes must not be delivered");

        out.ack(1);
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("push must unblock after ack");
        assert_eq!(rec.events().len(), 2);
    }

    #[test]
    fn reattach_after_exit_delivers_exit_notice_again() {
        let out = Output::<Recorder>::default();
        let first = Recorder::default();
        out.attach(first.clone());
        out.push(b"data");
        out.finish(Some(3));

        // Page reload: the old sink is orphaned, a fresh one attaches.
        let second = Recorder::default();
        out.attach(second.clone());

        assert_eq!(second.events(), vec![Event::Exit(Some(3))]);
        // The old sink already got everything, and gets nothing more.
        assert_eq!(
            first.events(),
            vec![Event::Bytes(b"data".to_vec()), Event::Exit(Some(3))]
        );
    }
}
