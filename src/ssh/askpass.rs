//! ssh's prompts answered in the app (spec 0006, flows 2 to 4; threat model
//! 0001). Tern starts ssh with its own binary as `SSH_ASKPASS`. When ssh
//! needs an answer it runs that binary as a helper, with the prompt as its
//! argument; the helper hands the prompt to the running app over a local
//! socket and prints the app's answer for ssh.
//!
//! The app's side is one [`Server`] per run, opened with the first SSH
//! connection: a Unix socket in a private directory, or a named pipe only
//! the user can open. Each connection gets a random token, which only its
//! ssh holds (in its environment) and its helpers send along; a prompt
//! without a live token gets no answer.
//!
//! Messages are the [`MAGIC`] and then fields, each a little-endian `u32`
//! length and its bytes. A helper sends its token, `SSH_ASKPASS_PROMPT` and
//! the prompt; the server answers with one field, or closes the connection
//! to decline, which the helper reports to ssh as exit code 1.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui::{App, Global};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Where the helper finds the app's endpoint. Helper mode needs this and
/// [`TOKEN_VAR`] (TM-askpass-18).
const ENDPOINT_VAR: &str = "TERN_ASKPASS";
/// The connection's token, as hex.
const TOKEN_VAR: &str = "TERN_ASKPASS_TOKEN";
/// The app's process ID, which the endpoint's server must have.
const PID_VAR: &str = "TERN_ASKPASS_PID";

/// What every request starts with: the protocol and its version, so a
/// helper from an upgrade installed while the app runs is refused.
const MAGIC: &[u8; 4] = b"ZTA1";
/// The longest message either side reads (TM-askpass-16).
const MAX_MESSAGE: usize = 64 * 1024;
/// How long the server waits for a helper's request (TM-askpass-16).
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a server waiting on the user looks whether ssh ended the
/// helper.
const POLL: Duration = Duration::from_millis(100);

/// 128 random bits from the OS (TM-askpass-03).
type Token = [u8; 16];

/// What kind of answer a prompt wants, from its text and ssh's hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Something to type: a password, a passphrase, a one-time code.
    Secret,
    /// A yes/no question: ssh's `confirm` hint, or a host-key question.
    YesNo,
    /// Something to show while ssh waits, like "touch your security key";
    /// ssh ends the helper when it's done.
    Notice,
}

impl PromptKind {
    /// The kind of prompt `text`, which came with `SSH_ASKPASS_PROMPT` set to
    /// `hint`. ssh asks its host-key questions with no hint at all, so they
    /// are told by how they end (research 0001).
    pub fn of(hint: &str, text: &str) -> Self {
        let question = text.trim_end();
        match hint {
            "none" => Self::Notice,
            "confirm" => Self::YesNo,
            _ if ["(yes/no)?", "(yes/no/[fingerprint])?", "(yes/no):"]
                .iter()
                .any(|end| question.ends_with(end)) =>
            {
                Self::YesNo
            }
            _ => Self::Secret,
        }
    }
}

/// A prompt ssh is waiting on (spec 0006, flow 4). Dropping it declines:
/// the helper exits 1, which ssh counts as a failed attempt.
pub struct Prompt {
    /// As ssh wrote it: matched as is, shown only cleaned (TM-askpass-08).
    pub text: String,
    pub kind: PromptKind,
    answer: mpsc::Sender<Zeroizing<Vec<u8>>>,
    ended: oneshot::Receiver<()>,
}

impl Prompt {
    /// Hands `answer` to the helper, which prints it for ssh.
    pub fn answer(&self, answer: &str) {
        let _ = self.answer.send(Zeroizing::new(answer.as_bytes().to_vec()));
    }

    /// Resolves once the helper is done with the prompt: answered, declined,
    /// or ended by ssh, which ends a notice's helper when it's done.
    pub async fn ended(&mut self) {
        let _ = (&mut self.ended).await;
    }
}

/// One SSH connection's askpass: what its ssh needs in its environment, and
/// the prompts its helpers send. Dropping `prompts` ends the connection: the
/// token gets no more answers, and helpers still waiting exit 1.
pub struct Connection {
    pub env: Vec<(String, String)>,
    pub prompts: UnboundedReceiver<Prompt>,
}

/// A live connection's token, and where its prompts go.
struct Registered {
    token: Token,
    prompts: UnboundedSender<Prompt>,
}

type Connections = Arc<Mutex<Vec<Registered>>>;

/// The app's end of the askpass socket (spec 0006, flow 2). Helpers are
/// served on threads of their own, never on the UI thread.
pub struct Server {
    endpoint: Endpoint,
    /// `SSH_ASKPASS`: this binary by its canonical path, never a name looked
    /// up on `PATH` (TM-askpass-06).
    exe: String,
    connections: Connections,
}

impl Server {
    /// Opens the endpoint and starts serving helpers. Without
    /// `$XDG_RUNTIME_DIR` (Unix) there is no endpoint, and ssh asks in the
    /// terminal as usual.
    pub fn start() -> Result<Self, String> {
        let exe = std::env::current_exe()
            .and_then(|exe| exe.canonicalize())
            .map_err(|error| format!("Tern can't find its own binary: {error}"))?
            .into_os_string()
            .into_string()
            .map_err(|_| "the path of Tern's binary isn't UTF-8".to_owned())?;
        let (endpoint, listener) = Endpoint::open()?;
        Ok(Self::serve(endpoint, listener, exe))
    }

    fn serve(endpoint: Endpoint, listener: Listener, exe: String) -> Self {
        let connections = Connections::default();
        let peers = endpoint.peers();
        let served = Arc::clone(&connections);
        // Detached: the thread blocks in `accept` until the app exits.
        let _ = std::thread::Builder::new()
            .name("askpass".into())
            .spawn(move || {
                for stream in listener.incoming().filter_map(Result::ok) {
                    let connections = Arc::clone(&served);
                    let _ = std::thread::Builder::new()
                        .name("askpass helper".into())
                        .spawn(move || serve(&stream, &connections, peers));
                }
            });
        Self {
            endpoint,
            exe,
            connections,
        }
    }

    /// A new connection with a fresh token, whose ssh gets [`Connection::env`]
    /// and nothing else of this (TM-askpass-03, -12).
    pub fn connect(&self) -> Result<Connection, String> {
        let token = random().map_err(|error| error.to_string())?;
        let (sender, prompts) = unbounded();
        let mut connections = self
            .connections
            .lock()
            .map_err(|_| "the askpass server stopped".to_owned())?;
        connections.retain(|connection| !connection.prompts.is_closed());
        connections.push(Registered {
            token,
            prompts: sender,
        });
        let env = [
            ("SSH_ASKPASS", self.exe.clone()),
            ("SSH_ASKPASS_REQUIRE", "force".into()),
            (ENDPOINT_VAR, self.endpoint.name()),
            (TOKEN_VAR, hex(&token)),
            (PID_VAR, std::process::id().to_string()),
        ];
        Ok(Connection {
            env: env
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
            prompts,
        })
    }
}

/// The app's running server, started by the first SSH connection, or why
/// it couldn't start.
struct Running(Result<Server, String>);

impl Global for Running {}

/// A new askpass connection on the app's server, which starts now if this is
/// the first. Without one, ssh asks in the terminal, and the error says why.
pub fn connect(cx: &mut App) -> Result<Connection, String> {
    if !cx.has_global::<Running>() {
        let server = Server::start();
        if server.is_ok() {
            // Dropping the server takes its socket away with the app.
            cx.on_app_quit(|cx| {
                cx.remove_global::<Running>();
                async {}
            })
            .detach();
        }
        cx.set_global(Running(server));
    }
    cx.global::<Running>()
        .0
        .as_ref()
        .map_err(Clone::clone)?
        .connect()
}

/// Serves one helper on its own thread: reads its request, hands the prompt
/// to the token's connection, and writes the answer back. Returning without
/// one drops the stream, and the helper exits 1.
fn serve(stream: &Stream, connections: &Mutex<Vec<Registered>>, peers: Peers) {
    if !peers.admits(stream) || stream.set_nonblocking(true).is_err() {
        return;
    }
    let mut reader = Deadline {
        stream,
        until: Instant::now() + READ_TIMEOUT,
    };
    let Ok(request) = Request::read(&mut reader) else {
        return;
    };
    let Some(prompts) = find(connections, &request.token) else {
        return;
    };
    let (answer, answers) = mpsc::channel();
    // Dropped when this returns, which tells the pane the helper is done.
    let (_done, ended) = oneshot::channel::<()>();
    let prompt = Prompt {
        kind: PromptKind::of(&request.hint, &request.prompt),
        text: request.prompt,
        answer,
        ended,
    };
    if prompts.unbounded_send(prompt).is_err() {
        return;
    }
    loop {
        match answers.recv_timeout(POLL) {
            Ok(answer) => {
                let _ = reply(stream, &answer);
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) if helper_gone(stream) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// The live connection that `token` belongs to, compared in constant time.
fn find(connections: &Mutex<Vec<Registered>>, token: &Token) -> Option<UnboundedSender<Prompt>> {
    let connections = connections.lock().ok()?;
    connections
        .iter()
        .find(|connection| same_token(&connection.token, token) && !connection.prompts.is_closed())
        .map(|connection| connection.prompts.clone())
}

/// Whether two tokens are the same, in a time that doesn't depend on where
/// they differ, so timing can't guess one byte by byte (TM-askpass-03).
fn same_token(a: &Token, b: &Token) -> bool {
    let difference = a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y));
    std::hint::black_box(difference) == 0
}

/// Whether the helper hung up (ssh ended it) or broke the protocol by
/// sending more after its request.
#[cfg(unix)]
fn helper_gone(mut stream: &Stream) -> bool {
    !matches!(stream.read(&mut [0; 1]), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
}

#[cfg(windows)]
fn helper_gone(stream: &Stream) -> bool {
    !matches!(pending(stream), Ok(0))
}

/// How many bytes the helper sent that aren't read yet, or an error once it
/// hung up. Windows has to peek: a nonblocking named pipe answers a read
/// with nothing to read like one after the helper hung up (`ERROR_NO_DATA`,
/// which arrives as end of file), so a waiting helper looked gone.
#[cfg(windows)]
fn pending(stream: &Stream) -> io::Result<usize> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    let Stream::NamedPipe(pipe) = stream;
    let mut available = 0;
    // SAFETY: a live pipe handle, and the one out pointer is a local; no
    // buffer is passed, so nothing is read.
    let peeked = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            pipe.as_handle().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if peeked == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available as usize)
}

fn reply(mut stream: &Stream, answer: &[u8]) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.write_all(&field_length(answer)?)?;
    stream.write_all(answer)
}

/// A helper's request.
struct Request {
    token: Token,
    hint: String,
    prompt: String,
}

impl Request {
    fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut magic = [0; 4];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut budget = MAX_MESSAGE - MAGIC.len();
        let token = read_field(reader, &mut budget)?;
        let hint = read_field(reader, &mut budget)?;
        let prompt = read_field(reader, &mut budget)?;
        Ok(Self {
            token: token[..]
                .try_into()
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
            hint: String::from_utf8_lossy(&hint).into_owned(),
            prompt: String::from_utf8_lossy(&prompt).into_owned(),
        })
    }
}

/// Reads a nonblocking stream, waiting for data until `until`. Named pipes
/// have no read timeouts of their own.
struct Deadline<'a> {
    stream: &'a Stream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            #[cfg(windows)]
            let ready = pending(self.stream)? > 0;
            #[cfg(unix)]
            let ready = true;
            if ready {
                match self.stream.read(buf) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    read => return read,
                }
            }
            if Instant::now() >= self.until {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A field of `budget` bytes at most, which it takes from the budget.
fn read_field(reader: &mut impl Read, budget: &mut usize) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    *budget = budget
        .checked_sub(length.saturating_add(4))
        .ok_or(io::ErrorKind::InvalidData)?;
    let mut field = Zeroizing::new(vec![0; length]);
    reader.read_exact(&mut field)?;
    Ok(field)
}

/// The length that starts a field holding `bytes`.
fn field_length(bytes: &[u8]) -> io::Result<[u8; 4]> {
    u32::try_from(bytes.len())
        .map(u32::to_le_bytes)
        .map_err(|_| io::ErrorKind::InvalidInput.into())
}

/// Helper mode, when ssh runs Tern as its askpass program (spec 0006,
/// flow 3). `None` when the endpoint or the token isn't set: this is the
/// app. Otherwise the exit code, 0 with the answer on stdout or 1 with no
/// output at all. Nothing is ever written to stderr, which is ssh's
/// terminal (TM-askpass-11).
pub fn helper() -> Option<i32> {
    let endpoint = std::env::var_os(ENDPOINT_VAR)?;
    let token = std::env::var_os(TOKEN_VAR)?;
    // A panic message would go to stderr.
    std::panic::set_hook(Box::new(|_| {}));
    let asked = (|| {
        let token = parse_token(token.to_str()?)?;
        let app = std::env::var(PID_VAR).ok()?.parse().ok()?;
        let prompt = std::env::args_os().nth(1)?.into_encoded_bytes();
        let hint = std::env::var_os("SSH_ASKPASS_PROMPT")
            .map(OsString::into_encoded_bytes)
            .unwrap_or_default();
        let answer = ask(&endpoint, &token, app, &hint, &prompt).ok()?;
        print_answer(&answer).ok()
    })();
    Some(if asked.is_some() { 0 } else { 1 })
}

/// Sends `prompt` to the app at `endpoint` and waits for its answer, as
/// long as the user takes. Refuses an endpoint whose server isn't process
/// `app` before sending anything: someone else could have created it first
/// (TM-askpass-02).
fn ask(
    endpoint: &OsStr,
    token: &Token,
    app: u32,
    hint: &[u8],
    prompt: &[u8],
) -> io::Result<Zeroizing<Vec<u8>>> {
    let stream = Stream::connect(endpoint_name(endpoint)?)?;
    let server = stream.peer_creds()?.pid().map(i64::from);
    if server != Some(i64::from(app)) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut request = MAGIC.to_vec();
    for field in [&token[..], hint, prompt] {
        request.extend(field_length(field)?);
        request.extend_from_slice(field);
    }
    if request.len() > MAX_MESSAGE {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut stream = &stream;
    stream.write_all(&request)?;
    let mut budget = MAX_MESSAGE;
    read_field(&mut stream, &mut budget)
}

/// Writes the answer and a line break straight to stdout's file, so no
/// buffer of ours keeps a copy (TM-askpass-13).
fn print_answer(answer: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    let handle = std::os::fd::AsFd::as_fd(&io::stdout()).try_clone_to_owned()?;
    #[cfg(windows)]
    let handle = std::os::windows::io::AsHandle::as_handle(&io::stdout()).try_clone_to_owned()?;
    let mut stdout = std::fs::File::from(handle);
    stdout.write_all(answer)?;
    stdout.write_all(b"\n")
}

/// Where helpers reach the server, and what keeps it to the user.
#[cfg(unix)]
struct Endpoint {
    /// A directory of its own, only the user may enter (TM-askpass-01).
    dir: std::path::PathBuf,
    socket: std::path::PathBuf,
    uid: u32,
}

#[cfg(unix)]
impl Endpoint {
    fn open() -> Result<(Self, Listener), String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|dir| !dir.is_empty())
            .ok_or("$XDG_RUNTIME_DIR isn't set")?;
        Self::open_in(std::path::Path::new(&runtime))
            .map_err(|error| format!("the askpass socket couldn't be opened: {error}"))
    }

    /// A socket in a new directory under `runtime`, created with mode 0700,
    /// so no one else can reach it.
    fn open_in(runtime: &std::path::Path) -> io::Result<(Self, Listener)> {
        use interprocess::local_socket::GenericFilePath;
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let dir = runtime.join(format!("tern-{}", hex(&random()?)));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let endpoint = Self {
            uid: std::fs::metadata(&dir)?.uid(),
            socket: dir.join("askpass.sock"),
            dir,
        };
        let listener = ListenerOptions::new()
            .name(endpoint.socket.as_path().to_fs_name::<GenericFilePath>()?)
            .mode(0o600)
            .create_sync()?;
        Ok((endpoint, listener))
    }

    fn name(&self) -> String {
        self.socket.to_string_lossy().into_owned()
    }

    fn peers(&self) -> Peers {
        Peers { uid: self.uid }
    }
}

#[cfg(unix)]
impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(windows)]
struct Endpoint {
    pipe: String,
}

#[cfg(windows)]
impl Endpoint {
    /// A pipe with 128 random bits in its name, which only its owner, the
    /// user, may open (TM-askpass-01). interprocess creates it with
    /// `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a pipe someone made first under
    /// the name fails the start instead of being shared (TM-askpass-02).
    fn open() -> Result<(Self, Listener), String> {
        use interprocess::local_socket::GenericNamespaced;
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        let opened = (|| -> io::Result<_> {
            let pipe = format!("tern-askpass-{}", hex(&random()?));
            let owner_only =
                SecurityDescriptor::deserialize(widestring::u16cstr!("D:P(A;;GA;;;OW)"))?;
            let listener = ListenerOptions::new()
                .name(pipe.as_str().to_ns_name::<GenericNamespaced>()?)
                .security_descriptor(owner_only)
                .create_sync()?;
            Ok((Self { pipe }, listener))
        })();
        opened.map_err(|error| format!("the askpass pipe couldn't be opened: {error}"))
    }

    fn name(&self) -> String {
        self.pipe.clone()
    }

    fn peers(&self) -> Peers {
        Peers {}
    }
}

#[cfg(unix)]
fn endpoint_name(endpoint: &OsStr) -> io::Result<Name<'_>> {
    use interprocess::local_socket::GenericFilePath;
    endpoint.to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
fn endpoint_name(endpoint: &OsStr) -> io::Result<Name<'_>> {
    use interprocess::local_socket::GenericNamespaced;
    endpoint.to_ns_name::<GenericNamespaced>()
}

/// Who may talk to the server: the user it runs as (TM-askpass-01).
#[derive(Clone, Copy)]
struct Peers {
    #[cfg(unix)]
    uid: u32,
}

impl Peers {
    /// On Windows the pipe's DACL keeps everyone else out already.
    fn admits(self, stream: &Stream) -> bool {
        #[cfg(unix)]
        return stream.peer_creds().ok().and_then(|creds| creds.euid()) == Some(self.uid);
        #[cfg(not(unix))]
        {
            let _ = stream;
            true
        }
    }
}

fn random() -> io::Result<Token> {
    let mut bytes = Token::default();
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_token(hex: &str) -> Option<Token> {
    let mut token = Token::default();
    if hex.len() != 2 * token.len() || !hex.is_ascii() {
        return None;
    }
    for (byte, pair) in token.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tern binary `cargo test` builds into the profile directory
    /// (`target/debug`), found from this test binary's path: an ancestor
    /// of it, at a depth that depends on cargo's version (`deps/` before
    /// Rust 1.100, `build/tern/<hash>/` since).
    fn built_binary() -> std::path::PathBuf {
        let name = format!("tern{}", std::env::consts::EXE_SUFFIX);
        let tests = std::env::current_exe().expect("the test binary's path");
        tests
            .ancestors()
            .skip(1)
            .map(|dir| dir.join(&name))
            .find(|binary| binary.is_file())
            .unwrap_or_else(|| {
                panic!(
                    "no {name} above {}: `cargo test` builds it, `cargo test --bin` doesn't",
                    tests.display()
                )
            })
    }

    /// ssh's question for a host it doesn't know yet (OpenSSH 10.6), which
    /// it asks with no hint.
    const UNKNOWN_HOST: &str = "The authenticity of host 'prod.example.com (192.0.2.10)' can't be established.\n\
        ED25519 key fingerprint is: SHA256:ufr9ck+x5zXLjNm4HvDvXiRs6L/6qjQoTXCJCukwfy8\n\
        This key is not known by any other names.\n\
        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";

    /// Spec 0006, flow 4: when ssh gives a hint, the hint says what the
    /// prompt is, whatever its text: `none` a notice to show while ssh
    /// waits, `confirm` a yes/no question.
    #[test]
    fn the_hint_none_is_a_notice_and_confirm_a_yes_no_question_whatever_the_text() {
        let texts = [
            "Confirm user presence for key ED25519-SK SHA256:ufr9ck+x5zXLjNm4HvDvXiRs6L",
            "alice@prod.example.com's password: ",
            UNKNOWN_HOST,
        ];
        for text in texts {
            assert_eq!(PromptKind::of("none", text), PromptKind::Notice, "{text:?}");
            assert_eq!(
                PromptKind::of("confirm", text),
                PromptKind::YesNo,
                "{text:?}"
            );
        }
    }

    /// Spec 0006, flow 4 (TM-askpass-07; research 0001): ssh asks its
    /// host-key questions without a hint. They are yes/no questions all the
    /// same, never something to type.
    #[test]
    fn ssh_host_key_questions_are_yes_no_questions_without_a_hint() {
        let questions = [
            UNKNOWN_HOST,
            // A known host whose key differs from the one known for its IP.
            "Warning: the ED25519 host key for 'prod.example.com' differs from the key for the IP address '192.0.2.10'\n\
             Offending key for IP in /home/alice/.ssh/known_hosts:3\n\
             Are you sure you want to continue connecting (yes/no)? ",
            // `UpdateHostKeys ask`.
            "Accept updated hostkeys? (yes/no): ",
        ];
        for question in questions {
            assert_eq!(
                PromptKind::of("", question),
                PromptKind::YesNo,
                "{question:?}"
            );
        }
    }

    /// Spec 0006, flow 4: everything else ssh asks is typed into a field:
    /// passwords, passphrases, PINs, a server's one-time codes.
    #[test]
    fn passwords_passphrases_and_one_time_codes_are_secrets_to_type() {
        let prompts = [
            "alice@prod.example.com's password: ",
            "(alice@prod.example.com) Password: ",
            "(alice@prod.example.com) Verification code: ",
            "Enter passphrase for key '/home/alice/.ssh/id_ed25519': ",
            "Enter PIN for ED25519-SK key /home/alice/.ssh/id_ed25519_sk: ",
        ];
        for prompt in prompts {
            assert_eq!(PromptKind::of("", prompt), PromptKind::Secret, "{prompt:?}");
        }
    }

    /// TM-askpass-03: a token is the same only when all 16 bytes are, so a
    /// guess that is right but for one byte, wherever it is, gets nothing.
    #[test]
    fn tokens_are_the_same_only_when_all_16_bytes_are() {
        let token: Token = std::array::from_fn(|ix| ix as u8 * 17);
        assert!(same_token(&token, &token));
        for ix in 0..token.len() {
            let mut guess = token;
            guess[ix] ^= 1;
            assert!(!same_token(&token, &guess), "byte {ix} differs");
        }
    }

    /// The Unix half (spec 0006, flows 2 to 4): a real server on a socket in
    /// a temp dir, and the built binary as the helper ssh runs. The Windows
    /// pipe is checked on Windows 11 (T-011).
    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::config::tests::TempDir;
        use crate::secrets::Key;
        use crate::ssh::target::Target;
        use futures::{FutureExt, StreamExt};
        use std::collections::HashSet;
        use std::fs;
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::path::{Path, PathBuf};
        use std::process::{Child, Command, ExitStatus, Output, Stdio};
        use std::task::{Wake, Waker};

        /// How long a test waits on a helper, a prompt or ssh before it fails.
        const WAIT: Duration = Duration::from_secs(10);
        /// `SSH_ASKPASS` where no ssh runs.
        const EXE: &str = "/usr/libexec/tern";
        /// What ssh asks for password authentication.
        const PASSWORD_PROMPT: &str = "alice@prod.example.com's password: ";
        /// A stand-in for a saved password.
        const PASSWORD: &str = "zt-test-password-not-a-secret";
        /// The passphrase of the end-to-end test's throwaway key.
        const PASSPHRASE: &str = "zt-test-passphrase-not-a-secret";

        type Env = Vec<(String, String)>;
        /// What [`ask`] returned on the thread it ran on.
        type Asking = mpsc::Receiver<io::Result<Zeroizing<Vec<u8>>>>;

        /// A server with its endpoint in `dir`, as `Server::start` opens one
        /// in `$XDG_RUNTIME_DIR`.
        fn serve_in(dir: &TempDir, exe: &str) -> Server {
            let (endpoint, listener) = Endpoint::open_in(dir.path()).expect("open an endpoint");
            Server::serve(endpoint, listener, exe.to_owned())
        }

        /// The value of `key` in what ssh gets in its environment.
        fn var<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
            env.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .unwrap_or_else(|| panic!("no {key} in {env:?}"))
        }

        /// `env` without `key`.
        fn without(env: &[(String, String)], key: &str) -> Env {
            env.iter()
                .filter(|(name, _)| name != key)
                .cloned()
                .collect()
        }

        /// `env` with `key` set to `value`.
        fn with(env: &[(String, String)], key: &str, value: &str) -> Env {
            let mut env = without(env, key);
            env.push((key.to_owned(), value.to_owned()));
            env
        }

        /// The token `connection`'s helpers send.
        fn token_of(connection: &Connection) -> Token {
            parse_token(var(&connection.env, TOKEN_VAR)).expect("a token")
        }

        /// The next prompt that reaches `connection`, within [`WAIT`].
        fn next_prompt(connection: &mut Connection) -> Prompt {
            let deadline = Instant::now() + WAIT;
            loop {
                match connection.prompts.try_recv() {
                    Ok(prompt) => return prompt,
                    Err(error) => assert!(
                        error.is_empty() && Instant::now() < deadline,
                        "no prompt came: {error}"
                    ),
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// Whether no prompt has reached `connection`.
        fn no_prompt(connection: &mut Connection) -> bool {
            connection
                .prompts
                .try_recv()
                .is_err_and(|error| error.is_empty())
        }

        /// Asks the app at the endpoint in `env` for `prompt`, as a helper
        /// with `token` does, on a thread of its own.
        fn ask_on_thread(env: &[(String, String)], token: Token, prompt: &str) -> Asking {
            let endpoint = OsString::from(var(env, ENDPOINT_VAR));
            let prompt = prompt.to_owned();
            let (sender, asking) = mpsc::channel();
            std::thread::spawn(move || {
                let result = ask(
                    &endpoint,
                    &token,
                    std::process::id(),
                    b"",
                    prompt.as_bytes(),
                );
                let _ = sender.send(result);
            });
            asking
        }

        /// What [`ask`] returned, within [`WAIT`]: the answer, or the error
        /// of a helper that got none.
        fn asked(asking: &Asking) -> io::Result<Zeroizing<Vec<u8>>> {
            asking.recv_timeout(WAIT).expect("the helper still waits")
        }

        /// A request as a helper frames it: the magic, then each field's
        /// length as a little-endian `u32` and its bytes.
        fn framed(magic: &[u8], fields: &[&[u8]]) -> Vec<u8> {
            let mut request = magic.to_vec();
            for field in fields {
                let length = u32::try_from(field.len()).expect("a field under 4 GiB");
                request.extend(length.to_le_bytes());
                request.extend_from_slice(field);
            }
            request
        }

        /// Sends `request` to the endpoint in `env` and returns what the
        /// server wrote back before it hung up. Fails if it doesn't hang up.
        fn reply_to(env: &[(String, String)], request: &[u8]) -> Vec<u8> {
            let mut stream = UnixStream::connect(var(env, ENDPOINT_VAR)).expect("connect");
            stream.set_read_timeout(Some(WAIT)).expect("set a timeout");
            // The server may hang up before it has read all of it.
            let _ = stream.write_all(request);
            let mut reply = Vec::new();
            match stream.read_to_end(&mut reply) {
                Ok(_) => reply,
                // Hung up with some of the request unread.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => reply,
                Err(error) => panic!("the server didn't hang up: {error}"),
            }
        }

        fn binary() -> PathBuf {
            super::built_binary()
        }

        /// The binary run as ssh runs its askpass helper, with `args` (the
        /// prompt) and `env` and nothing else of the environment: without a
        /// Wayland display or `$XDG_RUNTIME_DIR`, a binary that wasn't a
        /// helper couldn't open a window either.
        fn spawn_helper(env: &[(String, String)], args: &[&str]) -> Child {
            Command::new(binary())
                .env_clear()
                .envs(env.iter().map(|(key, value)| (key, value)))
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start the helper")
        }

        /// How `child` exited. One still running after [`WAIT`] is killed by
        /// its handle, never by name, and fails the test.
        fn exited(child: &mut Child) -> ExitStatus {
            let deadline = Instant::now() + WAIT;
            loop {
                if let Some(status) = child.try_wait().expect("poll the child") {
                    return status;
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("still running after {WAIT:?}");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// `child`'s exit status and output, once it exited.
        fn finished(mut child: Child) -> Output {
            exited(&mut child);
            child.wait_with_output().expect("read the child's output")
        }

        /// Asserts that the helper failed the one way it may (spec 0006,
        /// flow 3): exit code 1, nothing on stdout, and nothing on stderr,
        /// which is ssh's terminal.
        fn assert_failed_silently(output: &Output, case: &str) {
            let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
            assert_eq!(
                (
                    output.status.code(),
                    text(&output.stdout),
                    text(&output.stderr)
                ),
                (Some(1), String::new(), String::new()),
                "{case}"
            );
        }

        /// AC7's other half (spec 0006, flow 3; TM-askpass-11): with its
        /// connection's token, the helper prints the answer and a line break
        /// for ssh, exactly, and nothing on stderr.
        #[test]
        fn with_its_connections_token_the_helper_prints_exactly_the_answer_and_a_line_break() {
            let dir = TempDir::new("askpass-answer");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let child = spawn_helper(&connection.env, &[PASSWORD_PROMPT]);
            let prompt = next_prompt(&mut connection);
            assert_eq!(prompt.text, PASSWORD_PROMPT);
            assert_eq!(prompt.kind, PromptKind::Secret);
            prompt.answer(PASSWORD);
            let output = finished(child);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
            assert_eq!(output.stdout, format!("{PASSWORD}\n").as_bytes());
            assert!(stderr.is_empty(), "stderr: {stderr}");
        }

        /// AC7 (spec 0006; TM-askpass-03, -11): run by hand with a token
        /// that isn't a live connection's, the helper exits 1 with nothing on
        /// stdout or stderr, and its prompt never reaches the app, so nothing
        /// can answer it.
        #[test]
        fn run_by_hand_with_a_wrong_token_the_helper_exits_1_and_the_app_never_sees_its_prompt() {
            let dir = TempDir::new("askpass-ac7");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let live = var(&connection.env, TOKEN_VAR).to_owned();
            let last = if live.ends_with('0') { '1' } else { '0' };
            let tokens = [
                (
                    "the live token, its last digit changed",
                    format!("{}{last}", &live[..31]),
                ),
                (
                    "a token no connection has",
                    hex(&random().expect("a token")),
                ),
                ("not hex", "zz".repeat(16)),
                ("too short", live[..30].to_owned()),
                ("empty", String::new()),
            ];
            for (case, token) in tokens {
                let env = with(&connection.env, TOKEN_VAR, &token);
                assert_failed_silently(&finished(spawn_helper(&env, &[PASSWORD_PROMPT])), case);
            }
            assert!(no_prompt(&mut connection), "a prompt reached the app");
        }

        /// Spec 0006, flow 3 (TM-askpass-11, -18): whatever fails, the helper
        /// exits 1 with nothing on stdout or stderr. It is the helper before
        /// any of the app runs: the app would complain on stderr about the
        /// prompt as its argument, and about the missing Wayland display.
        #[test]
        fn every_failure_of_the_helper_is_exit_1_with_no_output() {
            let dir = TempDir::new("askpass-failures");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let env = &connection.env;
            let no_app = dir.path().join("no-app.sock").display().to_string();
            let cases: [(&str, Env, &[&str]); 4] = [
                (
                    "no app at the endpoint",
                    with(env, ENDPOINT_VAR, &no_app),
                    &[PASSWORD_PROMPT],
                ),
                ("no app PID", without(env, PID_VAR), &[PASSWORD_PROMPT]),
                (
                    "an app PID that isn't one",
                    with(env, PID_VAR, "tern"),
                    &[PASSWORD_PROMPT],
                ),
                ("no prompt", env.clone(), &[]),
            ];
            for (case, env, args) in cases {
                assert_failed_silently(&finished(spawn_helper(&env, args)), case);
            }
            assert!(no_prompt(&mut connection), "a prompt reached the app");
        }

        /// The next helper to connect to `listener`, within [`WAIT`].
        fn accepted(listener: &UnixListener) -> UnixStream {
            listener.set_nonblocking(true).expect("poll the listener");
            let deadline = Instant::now() + WAIT;
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).expect("block on reads");
                        stream.set_read_timeout(Some(WAIT)).expect("set a timeout");
                        return stream;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("no helper connected: {error}"),
                }
            }
        }

        /// TM-askpass-02: someone else created the endpoint first. The helper
        /// checks that the endpoint's server is the app it was given before
        /// it sends anything, and gives the squatter nothing, its token least
        /// of all. Given the squatter as the app, the same helper does send
        /// its request: that check is what held it back.
        #[test]
        fn the_helper_sends_nothing_to_an_endpoint_whose_server_isnt_the_app() {
            let dir = TempDir::new("askpass-squat");
            let socket = dir.path().join("squatted.sock");
            let squatter = UnixListener::bind(&socket).expect("squat the endpoint");
            let env = |app: u32| {
                vec![
                    (ENDPOINT_VAR.to_owned(), socket.display().to_string()),
                    (TOKEN_VAR.to_owned(), hex(&random().expect("a token"))),
                    (PID_VAR.to_owned(), app.to_string()),
                ]
            };
            // The app is a process other than the squatter, this test: its
            // parent.
            let app = std::os::unix::process::parent_id();
            let output = finished(spawn_helper(&env(app), &[PASSWORD_PROMPT]));
            assert_failed_silently(&output, "a squatted endpoint");
            let mut sent = Vec::new();
            accepted(&squatter)
                .read_to_end(&mut sent)
                .expect("read what the helper sent");
            assert!(sent.is_empty(), "the squatter got {} bytes", sent.len());

            let child = spawn_helper(&env(std::process::id()), &[PASSWORD_PROMPT]);
            let mut magic = [0; 4];
            accepted(&squatter)
                .read_exact(&mut magic)
                .expect("read the request");
            assert_eq!(&magic, MAGIC);
            assert_failed_silently(&finished(child), "a squatter that hung up");
        }

        /// TM-askpass-03: a token dies with its connection. Once the
        /// connection's prompts are dropped, as a pane drops them when its
        /// ssh exits, the token that was answered a moment ago gets nothing.
        #[test]
        fn a_token_gets_no_answer_once_its_connection_ended() {
            let dir = TempDir::new("askpass-ended");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let (env, token) = (connection.env.clone(), token_of(&connection));
            let asking = ask_on_thread(&env, token, PASSWORD_PROMPT);
            next_prompt(&mut connection).answer(PASSWORD);
            assert_eq!(&asked(&asking).expect("an answer")[..], PASSWORD.as_bytes());

            drop(connection);
            let refused = asked(&ask_on_thread(&env, token, PASSWORD_PROMPT));
            assert!(refused.is_err(), "answered after the connection ended");
        }

        /// TM-askpass-03, -19: each connection gets a token of its own, a
        /// reconnect included, and a token stands for its connection: a
        /// prompt reaches the connection whose token came with it, no other.
        #[test]
        fn each_connection_has_its_own_token_and_gets_only_the_prompts_sent_with_it() {
            let dir = TempDir::new("askpass-tokens");
            let server = serve_in(&dir, EXE);
            let mut connections: Vec<Connection> =
                (0..3).map(|_| server.connect().expect("connect")).collect();
            let tokens: HashSet<&str> = connections
                .iter()
                .map(|connection| var(&connection.env, TOKEN_VAR))
                .collect();
            assert_eq!(tokens.len(), connections.len(), "{tokens:?}");

            let second = &connections[1];
            let asking = ask_on_thread(&second.env, token_of(second), PASSWORD_PROMPT);
            next_prompt(&mut connections[1]).answer(PASSWORD);
            assert_eq!(&asked(&asking).expect("an answer")[..], PASSWORD.as_bytes());
            for ix in [0, 2] {
                assert!(no_prompt(&mut connections[ix]), "connection {ix} got it");
            }
        }

        /// Spec 0006, flow 4: a prompt left without an answer (Esc, Cancel,
        /// No) is declined. Its helper exits 1 with no output, and ssh counts
        /// a failed attempt.
        #[test]
        fn a_declined_prompts_helper_exits_1_printing_nothing() {
            let dir = TempDir::new("askpass-decline");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let child = spawn_helper(&connection.env, &[PASSWORD_PROMPT]);
            drop(next_prompt(&mut connection));
            assert_failed_silently(&finished(child), "declined");
        }

        /// Spec 0006, flow 4: with `SSH_ASKPASS_PROMPT=none` the prompt is a
        /// notice, which ssh ends by killing its helper when it's done, say
        /// after a touch of the security key. The prompt sees that end, and
        /// so the notice closes.
        #[test]
        fn a_notice_ends_when_ssh_kills_its_helper() {
            let dir = TempDir::new("askpass-notice");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let env = with(&connection.env, "SSH_ASKPASS_PROMPT", "none");
            let touch =
                "Confirm user presence for key ED25519-SK SHA256:ufr9ck+x5zXLjNm4HvDvXiRs6L";
            let mut child = spawn_helper(&env, &[touch]);
            let mut notice = next_prompt(&mut connection);
            assert_eq!(notice.kind, PromptKind::Notice);
            assert!(notice.ended().now_or_never().is_none(), "ended while shown");

            child.kill().expect("end the helper");
            let _ = child.wait();
            let deadline = Instant::now() + WAIT;
            while notice.ended().now_or_never().is_none() {
                assert!(Instant::now() < deadline, "the notice didn't end");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Sends on its channel each time it's woken.
        struct Wakes(mpsc::Sender<()>);

        impl Wake for Wakes {
            fn wake(self: Arc<Self>) {
                let _ = self.0.send(());
            }
        }

        /// TM-askpass-15: a pane shows one prompt at a time, and the next one
        /// waits in the connection. Closing the tab ends the connection,
        /// which declines what still waits: its helper exits 1.
        #[test]
        fn ending_a_connection_declines_the_prompts_still_waiting_in_it() {
            let dir = TempDir::new("askpass-queued");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            // Woken when a prompt arrives, which then stays where it waits.
            let (woken, arrived) = mpsc::channel();
            let waker = Waker::from(Arc::new(Wakes(woken)));
            let polled = connection
                .prompts
                .poll_next_unpin(&mut std::task::Context::from_waker(&waker));
            assert!(polled.is_pending(), "a prompt was there before any helper");
            let child = spawn_helper(&connection.env, &[PASSWORD_PROMPT]);
            arrived.recv_timeout(WAIT).expect("no prompt came");
            drop(connection);
            assert_failed_silently(&finished(child), "a prompt that waited");
        }

        /// TM-askpass-01: the socket is in a directory of its own, named with
        /// 128 random bits, that only the user may enter (0700), and only the
        /// user may open the socket (0600). Both go with the server, which
        /// the app drops when it quits.
        #[test]
        fn the_endpoint_is_private_to_the_user_and_goes_away_with_the_server() {
            let dir = TempDir::new("askpass-private");
            let server = serve_in(&dir, EXE);
            let env = server.connect().expect("connect").env;
            let socket = PathBuf::from(var(&env, ENDPOINT_VAR));
            let private = socket.parent().expect("the socket's directory").to_owned();
            assert_eq!(private.parent(), Some(dir.path()));
            let name = private.file_name().and_then(OsStr::to_str).expect("a name");
            let random = name.strip_prefix("tern-").expect(name);
            assert!(
                random.len() == 32 && random.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}"
            );
            let metadata = |path: &Path| fs::metadata(path).expect("stat the endpoint");
            assert_eq!(metadata(&private).permissions().mode() & 0o777, 0o700);
            assert_eq!(metadata(&socket).permissions().mode() & 0o777, 0o600);
            assert!(metadata(&socket).file_type().is_socket());

            drop(server);
            assert!(!socket.exists(), "the socket is left");
            assert!(!private.exists(), "its directory is left");
        }

        /// TM-askpass-16: a message is 64 KiB at most. The helper doesn't
        /// send a longer one, and the server answers none: it hangs up as
        /// soon as a field's length goes over, without reading on, and serves
        /// the next helper as before.
        #[test]
        fn a_request_over_64_kib_gets_no_answer_and_the_next_one_is_served() {
            let dir = TempDir::new("askpass-64kib");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let (env, token) = (connection.env.clone(), token_of(&connection));
            // What 64 KiB leave for a prompt after the magic, three lengths,
            // the token and no hint.
            let longest = MAX_MESSAGE - MAGIC.len() - 3 * 4 - token.len();
            let over = "x".repeat(longest + 1);

            let endpoint = OsString::from(var(&env, ENDPOINT_VAR));
            let unsent = ask(&endpoint, &token, std::process::id(), b"", over.as_bytes());
            assert_eq!(
                unsent.err().map(|error| error.kind()),
                Some(io::ErrorKind::InvalidInput)
            );
            let requests = [
                (
                    "one byte over",
                    framed(MAGIC, &[&token, b"", over.as_bytes()]),
                ),
                (
                    "a length of 4 GiB",
                    [
                        framed(MAGIC, &[&token, b""]),
                        u32::MAX.to_le_bytes().to_vec(),
                    ]
                    .concat(),
                ),
            ];
            for (case, request) in requests {
                assert!(reply_to(&env, &request).is_empty(), "{case}");
            }
            assert!(no_prompt(&mut connection), "a prompt reached the app");

            let request = framed(MAGIC, &[&token, b"", &over.as_bytes()[1..]]);
            let replying = std::thread::spawn(move || reply_to(&env, &request));
            let prompt = next_prompt(&mut connection);
            assert_eq!(prompt.text.len(), longest);
            prompt.answer(PASSWORD);
            let reply = replying.join().expect("a reply");
            assert_eq!(reply, framed(b"", &[PASSWORD.as_bytes()]));
        }

        /// TM-askpass-16: a request that doesn't start with this protocol's
        /// magic, say from a newer helper installed while the app runs, gets
        /// no answer, and the next helper is served as before.
        #[test]
        fn a_request_with_another_magic_gets_no_answer_and_the_next_one_is_served() {
            let dir = TempDir::new("askpass-magic");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let (env, token) = (connection.env.clone(), token_of(&connection));
            let fields: [&[u8]; 3] = [&token, b"", PASSWORD_PROMPT.as_bytes()];
            for magic in [b"ZTA2", b"XXXX"] {
                let reply = reply_to(&env, &framed(magic, &fields));
                assert!(reply.is_empty(), "{:?}", String::from_utf8_lossy(magic));
            }
            assert!(no_prompt(&mut connection), "a prompt reached the app");

            let request = framed(MAGIC, &fields);
            let replying = std::thread::spawn(move || reply_to(&env, &request));
            next_prompt(&mut connection).answer(PASSWORD);
            let reply = replying.join().expect("a reply");
            assert_eq!(reply, framed(b"", &[PASSWORD.as_bytes()]));
        }

        /// TM-askpass-16: every helper is served on its own, so one that
        /// connects and sends nothing, or stops halfway through its request,
        /// holds up no other.
        #[test]
        fn a_helper_that_stalls_holds_up_no_other() {
            let dir = TempDir::new("askpass-stall");
            let server = serve_in(&dir, EXE);
            let mut connection = server.connect().expect("connect");
            let (env, token) = (connection.env.clone(), token_of(&connection));
            let endpoint = var(&env, ENDPOINT_VAR);
            let _silent = UnixStream::connect(endpoint).expect("connect");
            let mut halfway = UnixStream::connect(endpoint).expect("connect");
            let request = framed(MAGIC, &[&token, b"", PASSWORD_PROMPT.as_bytes()]);
            halfway
                .write_all(&request[..request.len() / 2])
                .expect("send half a request");

            let asking = ask_on_thread(&env, token, PASSWORD_PROMPT);
            next_prompt(&mut connection).answer(PASSWORD);
            assert_eq!(&asked(&asking).expect("an answer")[..], PASSWORD.as_bytes());
        }

        /// Spec 0006, flow 2 (TM-askpass-06, -12): ssh gets what it takes to
        /// run the helper and nothing else, no secret among it: the binary
        /// the server runs as as `SSH_ASKPASS`, forced even with a terminal,
        /// the endpoint, the connection's 128-bit token as hex, and the app's
        /// PID.
        #[test]
        fn a_connections_environment_is_askpass_forced_the_endpoint_its_token_and_the_app_pid() {
            let dir = TempDir::new("askpass-env");
            let server = serve_in(&dir, EXE);
            let env = server.connect().expect("connect").env;
            let mut keys: Vec<&str> = env.iter().map(|(key, _)| key.as_str()).collect();
            keys.sort_unstable();
            let expected = [
                "SSH_ASKPASS",
                "SSH_ASKPASS_REQUIRE",
                ENDPOINT_VAR,
                PID_VAR,
                TOKEN_VAR,
            ];
            assert_eq!(keys, expected);
            assert_eq!(var(&env, "SSH_ASKPASS"), EXE);
            assert_eq!(var(&env, "SSH_ASKPASS_REQUIRE"), "force");
            let endpoint = Path::new(var(&env, ENDPOINT_VAR));
            assert!(endpoint.is_absolute(), "{}", endpoint.display());
            let metadata = fs::metadata(endpoint).expect("stat the endpoint");
            assert!(metadata.file_type().is_socket(), "{}", endpoint.display());
            let token = var(&env, TOKEN_VAR);
            assert!(
                token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()),
                "{token}"
            );
            assert_eq!(var(&env, PID_VAR), std::process::id().to_string());
        }

        /// A child killed by its own handle when dropped, so a failing test
        /// leaves no sshd or ssh behind.
        struct Reaped(Child);

        impl Drop for Reaped {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// The lines a child writes to a pipe, read on a thread as they come.
        struct Lines {
            lines: mpsc::Receiver<String>,
            seen: Vec<String>,
        }

        impl Lines {
            fn of(pipe: impl Read + Send + 'static) -> Self {
                let (sender, lines) = mpsc::channel();
                std::thread::spawn(move || {
                    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                });
                Self {
                    lines,
                    seen: Vec::new(),
                }
            }

            /// Waits up to [`WAIT`] for a line with `text` in it.
            fn wait_for(&mut self, text: &str) {
                let deadline = Instant::now() + WAIT;
                while !self.seen.iter().any(|line| line.contains(text)) {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match self.lines.recv_timeout(left) {
                        Ok(line) => self.seen.push(line),
                        Err(_) => panic!("no line with {text:?} in {:?}", self.seen),
                    }
                }
            }

            /// Everything written, up to the end of the pipe.
            fn all(mut self) -> String {
                while let Ok(line) = self.lines.recv_timeout(WAIT) {
                    self.seen.push(line);
                }
                self.seen.join("\n")
            }
        }

        /// AC6 and AC4's process part (spec 0006; TM-askpass-07, -11, -12),
        /// with the system ssh and a throwaway sshd on 127.0.0.1 that lets
        /// one encrypted key in. The unknown host's key question reaches the
        /// app as a yes/no question with ssh's full text, fingerprint
        /// included; nothing answers it by itself, and declining it fails
        /// the connection. Answered yes, it's followed by the key's
        /// passphrase prompt, which the connection's target maps to that
        /// key. Once ssh logged in with it, the passphrase is in neither
        /// ssh's environment nor its arguments nor its output (here pipes,
        /// in a pane the PTY). Nothing in `~/.ssh` is read or written.
        #[test]
        #[ignore = "needs /usr/sbin/sshd"]
        fn a_real_ssh_asks_the_host_key_then_the_passphrase_and_never_exposes_it() {
            let dir = TempDir::new("askpass-e2e");
            let file = |name: &str| dir.path().join(name).display().to_string();
            let keygen = |args: &[&str]| {
                let status = Command::new("ssh-keygen")
                    .args(args)
                    .stdin(Stdio::null())
                    .status()
                    .expect("run ssh-keygen");
                assert!(status.success(), "ssh-keygen {args:?}");
            };
            let (host_key, key) = (file("host_key"), file("id_ed25519"));
            keygen(&[
                "-q", "-t", "ed25519", "-N", "", "-C", "zt-e2e", "-f", &host_key,
            ]);
            keygen(&[
                "-q", "-t", "ed25519", "-N", PASSPHRASE, "-C", "zt-e2e", "-f", &key,
            ]);
            fs::copy(format!("{key}.pub"), file("authorized_keys")).expect("authorize the key");
            let listed = Command::new("ssh-keygen")
                .args(["-l", "-E", "sha256", "-f", &format!("{host_key}.pub")])
                .output()
                .expect("read the host key's fingerprint");
            let listed = String::from_utf8(listed.stdout).expect("UTF-8");
            let fingerprint = listed.split_whitespace().nth(1).expect("a fingerprint");

            let port = TcpListener::bind("127.0.0.1:0")
                .and_then(|listener| listener.local_addr())
                .expect("a free port")
                .port();
            let sshd_config = file("sshd_config");
            let sshd_settings = format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {host_key}\nPidFile {}\n\
                 AuthorizedKeysFile {}\nUsePAM no\nStrictModes no\nPermitUserRC no\n\
                 PerSourcePenalties no\nPasswordAuthentication no\n\
                 KbdInteractiveAuthentication no\n",
                file("sshd.pid"),
                file("authorized_keys"),
            );
            fs::write(&sshd_config, sshd_settings).expect("write sshd_config");
            let mut sshd = Reaped(
                Command::new("/usr/sbin/sshd")
                    .args(["-D", "-e", "-f", &sshd_config])
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("start sshd"),
            );
            let mut log = Lines::of(sshd.0.stderr.take().expect("sshd's log"));
            log.wait_for(&format!("Server listening on 127.0.0.1 port {port}"));

            let ssh_config = file("ssh_config");
            let ssh_settings = format!(
                "Host zt-e2e\n  HostName 127.0.0.1\n  Port {port}\n  IdentityFile {key}\n  \
                 IdentitiesOnly yes\n  IdentityAgent none\n  UserKnownHostsFile {}\n  \
                 GlobalKnownHostsFile /dev/null\n  StrictHostKeyChecking ask\n  \
                 UpdateHostKeys no\n  CheckHostIP no\n  ControlMaster no\n  ControlPath none\n  \
                 PreferredAuthentications publickey\n  LogLevel VERBOSE\n",
                file("known_hosts"),
            );
            fs::write(&ssh_config, ssh_settings).expect("write the ssh config");
            // How a pane connects to the alias, with `-F` to keep
            // `~/.ssh/config` out and `-N` to log in and run nothing.
            let args: Vec<String> = ["-F", &ssh_config, "-N", "zt-e2e"].map(String::from).into();
            let target = Target::resolve(&args).expect("ssh -G resolves the host");
            let exe = binary().canonicalize().expect("the binary's path");
            let server = serve_in(&dir, exe.to_str().expect("a UTF-8 path"));
            let connect = |connection: &Connection| {
                Reaped(
                    Command::new(crate::ssh::PROGRAM)
                        .args(&args)
                        .env_clear()
                        .envs(connection.env.iter().map(|(key, value)| (key, value)))
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .expect("start ssh"),
                )
            };

            // Declined: ssh fails as for a dropped connection.
            let mut connection = server.connect().expect("connect");
            let mut declined = connect(&connection);
            let question = next_prompt(&mut connection);
            assert_eq!(question.kind, PromptKind::YesNo, "{:?}", question.text);
            assert!(
                question.text.starts_with("The authenticity of host")
                    && question.text.contains(fingerprint),
                "{:?}",
                question.text
            );
            assert_eq!(target.secret_for(&question.text), None);
            let waiting = declined.0.try_wait().expect("poll ssh");
            assert!(waiting.is_none(), "ssh didn't wait for an answer");
            drop(question);
            let status = exited(&mut declined.0);
            assert_eq!(status.code(), Some(crate::ssh::CONNECTION_LOST));
            drop(connection);

            // Answered: the passphrase follows, for the connection's own key.
            let mut connection = server.connect().expect("connect again");
            let mut ssh = connect(&connection);
            let stdout = Lines::of(ssh.0.stdout.take().expect("ssh's stdout"));
            let mut stderr = Lines::of(ssh.0.stderr.take().expect("ssh's stderr"));
            next_prompt(&mut connection).answer("yes");
            let prompt = next_prompt(&mut connection);
            assert_eq!(prompt.kind, PromptKind::Secret, "{:?}", prompt.text);
            let passphrase = Some(Key::Passphrase(key.clone()));
            assert_eq!(
                target.secret_for(&prompt.text),
                passphrase,
                "{:?}",
                prompt.text
            );
            prompt.answer(PASSPHRASE);
            stderr.wait_for("Authenticated to");

            let read = |what: &str| {
                let path = format!("/proc/{}/{what}", ssh.0.id());
                fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
            };
            let (environ, cmdline) = (read("environ"), read("cmdline"));
            let has = |bytes: &[u8], text: &str| {
                bytes
                    .windows(text.len())
                    .any(|window| window == text.as_bytes())
            };
            assert!(has(&environ, TOKEN_VAR), "not the environment of ssh");
            assert!(
                !has(&environ, PASSPHRASE),
                "the passphrase is in its environment"
            );
            assert!(
                !has(&cmdline, PASSPHRASE),
                "the passphrase is in its arguments"
            );
            drop(ssh);
            let output = format!("{}\n{}", stdout.all(), stderr.all());
            assert!(!output.contains(PASSPHRASE), "ssh printed the passphrase");
        }
    }

    /// The Windows pipe with the built binary as the helper (spec 0006,
    /// flow 4): a prompt stays up while the user takes their time, and the
    /// answer reaches the helper's stdout. A nonblocking pipe once read as
    /// hung up, which closed every dialog after one poll.
    #[cfg(windows)]
    #[test]
    fn a_windows_helper_waits_for_the_answer() {
        use futures::FutureExt;
        use std::process::{Command, Stdio};
        let binary = built_binary();
        let (endpoint, listener) = Endpoint::open().expect("open the pipe");
        let server = Server::serve(endpoint, listener, String::new());
        let mut connection = server.connect().expect("a connection");
        let helper = Command::new(binary)
            .arg("Are you sure you want to continue connecting (yes/no/[fingerprint])? ")
            .envs(connection.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the helper");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut prompt = loop {
            if let Ok(prompt) = connection.prompts.try_recv() {
                break prompt;
            }
            assert!(Instant::now() < deadline, "no prompt came");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(prompt.kind, PromptKind::YesNo);
        // Several of the server's polls.
        std::thread::sleep(POLL * 5);
        assert!(
            prompt.ended().now_or_never().is_none(),
            "the prompt ended unanswered"
        );

        prompt.answer("yes");
        let output = helper.wait_with_output().expect("the helper ends");
        assert!(output.status.success(), "helper: {:?}", output.status);
        assert_eq!(
            output.stdout,
            b"yes
"
        );
    }
}
