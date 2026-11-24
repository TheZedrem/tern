//! Offering the shell-integration snippet in WSL (spec 0022): noticing a
//! WSL shell that doesn't report its prompts, the bar that offers to add
//! the snippet, and the line that adds it. Tern never writes the rc file
//! itself: the line is typed into the user's own shell after a click.

use super::{PaneEvent, TerminalPane};
use crate::history::ShellKind;
use gpui::{AppContext, Context, IntoElement, ParentElement, Styled, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Sizable};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a noticed WSL shell has to report a prompt before the offer
/// shows: a shell that loads the snippet reports at once.
const WAIT: Duration = Duration::from_secs(2);
/// How long the typed line has to make the shell report.
const ADDED: Duration = Duration::from_secs(5);

/// Where a pane is with the offer.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Offer {
    /// No WSL shell without the snippet was noticed yet.
    #[default]
    None,
    /// The bar shows, with the line **Add** would type.
    Shown(String),
    /// Answered, or the snippet reported: this pane isn't asked again.
    Done,
}

/// Whether a profile's `program` is WSL's launcher. `bash.exe` isn't
/// taken for it: that is Git Bash far more often.
pub fn is_wsl(program: &str) -> bool {
    Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("wsl"))
}

/// The folder holding the snippets: next to `tern.exe`, where the MSI
/// puts it, else the clone's when run from `target/<profile>/`.
fn snippet_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let clone = dir.parent().and_then(Path::parent);
    [Some(dir), clone]
        .into_iter()
        .flatten()
        .map(|dir| dir.join("shell-integration"))
        .find(|dir| dir.join("tern.bash").is_file())
}

/// The line **Add** types into a WSL shell (spec 0022, flow 4), for the
/// snippets in the Windows folder `dir`. It starts with a space, so
/// shells that skip such lines keep it out of their history; turns the
/// folder into a WSL path with `wslpath`; picks zsh's snippet and rc file
/// in zsh and bash's otherwise; appends the source line unless the rc
/// file already names the snippet; and sources it at once. Only `dir`
/// goes into it [load-bearing], single-quoted, never terminal output.
pub fn add_line(dir: &Path) -> String {
    let dir = dir.to_string_lossy().replace('\'', r"'\''");
    format!(
        " s=$(wslpath -u '{dir}'); \
         if [ -n \"$ZSH_VERSION\" ]; then s=$s/tern.zsh; r=~/.zshrc; \
         else s=$s/tern.bash; r=~/.bashrc; fi; \
         grep -qsF \"$s\" \"$r\" || printf '\\n. \"%s\"\\n' \"$s\" >> \"$r\"; . \"$s\""
    )
}

/// Whether a `wsl.exe` runs under the process `root`: one look at the
/// process list (spec 0022, flow 1). Nothing of it is kept.
#[cfg(windows)]
fn runs_wsl(root: u32) -> bool {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut parents = HashMap::new();
    let mut launchers = Vec::new();
    // SAFETY: the snapshot handle is checked, used only here and closed;
    // `entry` is a plain struct whose size field is set as the API asks.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            let name = &entry.szExeFile;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            if String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case("wsl.exe") {
                launchers.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    launchers
        .into_iter()
        .any(|pid| descends_from(pid, root, &parents))
}

#[cfg(not(windows))]
fn runs_wsl(_: u32) -> bool {
    false
}

/// Whether `pid` is `root` or has it among its ancestors in `parents`.
/// Bounded, because a reused process ID can make the map loop.
#[cfg_attr(not(windows), allow(dead_code))]
fn descends_from(pid: u32, root: u32, parents: &std::collections::HashMap<u32, u32>) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if at == root {
            return true;
        }
        match parents.get(&at) {
            Some(&parent) if parent != at => at = parent,
            _ => return false,
        }
    }
    false
}

impl TerminalPane {
    /// Takes note that the pane's shell reported a prompt as `shell`: a
    /// WSL shell that does has the snippet, so there is nothing to offer.
    pub(super) fn snippet_reported(&mut self, shell: ShellKind) {
        if matches!(shell, ShellKind::Bash | ShellKind::Zsh) {
            self.snippet_seen = true;
            if matches!(self.offer, Offer::Shown(_)) {
                self.offer = Offer::Done;
            }
        }
    }

    /// Looks for a WSL shell in this pane and offers the snippet if it
    /// doesn't report within [`WAIT`] (spec 0022, flows 1 to 3). Called
    /// when the program starts and when its title changes. At the start
    /// (`started`) only the profile's program name counts, so a pane that
    /// boots with the app reads no process list before the first frame;
    /// later the list is read off the UI thread, and never on a timer.
    pub(super) fn look_for_wsl(&mut self, started: bool, cx: &mut Context<Self>) {
        let super::State::Running(running) = &self.state else {
            return;
        };
        if !cfg!(windows)
            || self.bench
            || self.profile.ssh
            || !self.settings.offer_snippet
            || self.offer != Offer::None
            || self.snippet_seen
            || self.looking_for_wsl
        {
            return;
        }
        let launcher = is_wsl(&self.launch.command.program);
        if started && !launcher {
            return;
        }
        let pid = running.child_pid;
        self.looking_for_wsl = true;
        cx.spawn(async move |this, cx| {
            let line = cx
                .background_spawn(async move {
                    let found = launcher || pid.is_some_and(runs_wsl);
                    found.then(snippet_dir).flatten().map(|dir| add_line(&dir))
                })
                .await;
            if line.is_some() {
                cx.background_executor().timer(WAIT).await;
            }
            let _ = this.update(cx, |pane, cx| {
                pane.looking_for_wsl = false;
                pane.sync_prompt();
                if let Some(line) = line
                    && !pane.snippet_seen
                    && !pane.exited
                    && pane.offer == Offer::None
                {
                    pane.offer = Offer::Shown(line);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// **Add**: types the line into the shell (spec 0022, flow 4), and
    /// says so if the shell doesn't report a prompt afterwards.
    fn add_snippet(&mut self, cx: &mut Context<Self>) {
        let Offer::Shown(line) = std::mem::replace(&mut self.offer, Offer::Done) else {
            return;
        };
        self.snippet_seen = false;
        self.send(format!("{line}\r").into_bytes());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ADDED).await;
            let _ = this.update(cx, |pane, cx| {
                pane.sync_prompt();
                if !pane.snippet_seen && !pane.exited {
                    pane.notice(
                        "The shell integration didn't load. The README's \
                         Shell integration section has the line to add by hand."
                            .into(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// The offer's bar along the pane's bottom (spec 0022, flow 3). It
    /// takes no keys: the shell keeps the keyboard.
    pub(super) fn render_offer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Offer::Shown(line) = &self.offer else {
            return None;
        };
        if self.exited || !self.settings.offer_snippet {
            return None;
        }
        let theme = cx.theme();
        let button = |id: &'static str, label: &'static str| Button::new(id).label(label).xsmall();
        Some(
            div()
                .absolute()
                .bottom(px(6.))
                .left(px(6.))
                .right(px(6.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .text_sm()
                .shadow_md()
                .child(
                    "Add Tern's shell integration to this WSL shell? It enables \
                     command suggestions. Answer at the shell's prompt: Add types \
                     this line there.",
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_xs()
                        .font_family(self.settings.font.family.clone())
                        .text_color(theme.muted_foreground)
                        .child(line.trim_start().to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .child(
                            button("offer-never", "Never ask")
                                .ghost()
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.offer = Offer::Done;
                                    cx.emit(PaneEvent::NeverOfferSnippet);
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("offer-later", "Not now")
                                .ghost()
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.offer = Offer::Done;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("offer-add", "Add")
                                .primary()
                                .on_click(cx.listener(|pane, _, _, cx| pane.add_snippet(cx))),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, mpsc};

    /// Held by every test that starts or looks for a `wsl.exe` under the
    /// test process, so one test's child isn't another's finding.
    static WSL: Mutex<()> = Mutex::new(());

    /// Spec 0022, flow 1: a profile running `wsl.exe` or `wsl` is a WSL
    /// session, however the launcher is spelled or pathed.
    #[test]
    fn the_wsl_launcher_is_noticed_by_name_case_and_path() {
        for program in [
            "wsl",
            "wsl.exe",
            "WSL.EXE",
            "Wsl.exe",
            r"C:\Windows\System32\wsl.exe",
            "C:/Windows/System32/wsl.exe",
        ] {
            assert!(is_wsl(program), "{program}");
        }
    }

    /// Spec 0022, flow 1 and AC7: `bash.exe` doesn't count (Git Bash far
    /// more often), nor does any program that merely resembles `wsl`.
    #[test]
    fn other_programs_are_not_taken_for_wsl() {
        for program in [
            "",
            "bash.exe",
            "bash",
            r"C:\Program Files\Git\bin\bash.exe",
            "pwsh",
            "powershell.exe",
            "cmd.exe",
            "wslview",
            "wslhost.exe",
            "wsl2.exe",
            "ubuntu.exe",
            r"C:\wsl\pwsh.exe",
        ] {
            assert!(!is_wsl(program), "{program}");
        }
    }

    /// Spec 0022, flow 4: the line starts with a space, so shells that
    /// skip such lines keep it out of their history.
    #[test]
    fn the_line_starts_with_a_space() {
        let line = add_line(Path::new(r"C:\Program Files\Tern\shell-integration"));
        assert!(line.starts_with(' '), "{line:?}");
        assert!(!line.trim().is_empty());
    }

    /// Spec 0022, flow 4: Tern types one line and presses Enter itself.
    /// A newline or any other control character inside it would run a
    /// part early or send the shell a key.
    #[test]
    fn the_line_is_one_line_without_control_characters() {
        for dir in [r"C:\Program Files\Tern\shell-integration", r"C:\it's\tern"] {
            let line = add_line(Path::new(dir));
            assert!(!line.chars().any(char::is_control), "{line:?}");
        }
    }

    /// Spec 0022, flow 4: the line turns the Windows path into a WSL one
    /// with `wslpath`, and knows both rc files and both snippets, so zsh
    /// gets `~/.zshrc` and bash `~/.bashrc`.
    #[test]
    fn the_line_names_wslpath_both_rc_files_and_both_snippets() {
        let line = add_line(Path::new(r"C:\Tern\shell-integration"));
        for part in ["wslpath", "~/.bashrc", "~/.zshrc", "tern.bash", "tern.zsh"] {
            assert!(line.contains(part), "{part} missing in {line:?}");
        }
        assert!(line.contains("ZSH_VERSION"), "{line:?}");
    }

    /// What a POSIX shell reads as the one word starting at `text`:
    /// single quotes kept literal, a backslash outside them escaping the
    /// next character. Returns the word and what follows it.
    fn shell_word(text: &str) -> (String, &str) {
        let mut word = String::new();
        let mut chars = text.char_indices();
        let mut quoted = false;
        while let Some((at, c)) = chars.next() {
            match c {
                '\'' => quoted = !quoted,
                '\\' if !quoted => word.extend(chars.next().map(|(_, c)| c)),
                c if !quoted && (c.is_whitespace() || ";)&|<>$`\"(".contains(c)) => {
                    return (word, &text[at..]);
                }
                c => word.push(c),
            }
        }
        assert!(!quoted, "unclosed quote in {text:?}");
        (word, "")
    }

    /// Spec 0022 §Security, trust boundary: the snippet's path goes in
    /// single-quoted. Read as a shell would, `wslpath`'s argument is the
    /// folder itself, character for character, and the fixed text follows
    /// unchanged: a quote, `$(…)` or `;` in the folder's name runs nothing.
    #[test]
    fn the_folder_is_one_single_quoted_word_whatever_it_holds() {
        let plain = add_line(Path::new("X"));
        let (before, after) = plain.split_once("'X'").expect("the folder, single-quoted");
        for dir in [
            r"C:\Program Files\Tern\shell-integration",
            r"C:\Users\it's me\tern",
            "'; touch /tmp/owned; '",
            "a''b",
            "'",
            r"C:\$(reboot)\`id`\$HOME",
            r"C:\back\'slash",
            "C:\\quote\"d",
        ] {
            let line = add_line(Path::new(dir));
            let rest = line
                .strip_prefix(before)
                .unwrap_or_else(|| panic!("{dir}: fixed text changed: {line:?}"));
            let (word, rest) = shell_word(rest);
            assert_eq!(word, dir, "{line:?}");
            assert_eq!(rest, after, "{dir}: the folder leaked into {line:?}");
        }
    }

    /// Spec 0022 §Security: a single quote in the folder's name is written
    /// in the escaped form `'\''`, never bare.
    #[test]
    fn a_single_quote_in_the_folder_is_escaped() {
        let line = add_line(Path::new(r"C:\it's\tern"));
        assert!(line.contains(r"'C:\it'\''s\tern'"), "{line:?}");
    }

    /// Runs `script` in WSL's bash without the user's rc files, giving
    /// what it printed. `started` gets `wsl.exe`'s process ID while bash
    /// still waits for the script. `None` when WSL can't be started; a
    /// run that outlasts the timeout is stopped by its own handle.
    fn wsl_bash(script: &str, started: impl FnOnce(u32)) -> Option<String> {
        let mut child = Command::new("wsl.exe")
            .args(["--", "bash", "--norc", "--noprofile", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        started(child.id());
        let mut stdin = child.stdin.take()?;
        let mut stdout = child.stdout.take()?;
        let (done, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            let _ = done.send(String::from_utf8_lossy(&bytes).into_owned());
        });
        // Dropping `stdin` ends the script, so bash can't wait for more.
        let written = stdin.write_all(script.as_bytes());
        drop(stdin);
        let output = output.recv_timeout(Duration::from_secs(120));
        if output.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        written.ok().and(output.ok())
    }

    /// Spec 0022, flow 1: the test process has no `wsl.exe` under it, so
    /// the process list finds none (and on Linux there is never one).
    #[test]
    fn a_process_without_wsl_under_it_runs_no_wsl() {
        let _alone = WSL.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!runs_wsl(std::process::id()));
    }

    /// Spec 0022, flow 4, AC2 and AC3, in a real WSL bash (and zsh, where
    /// installed) with `HOME` pointed at a fresh temp folder: the line run
    /// twice leaves exactly one source line in the shell's own rc file,
    /// naming a snippet that exists under the `wslpath` of a Windows
    /// folder with spaces in it. Flow 1 on the way: while that `wsl.exe`
    /// runs, the process list finds it under the test process.
    /// Skipped where WSL isn't available.
    #[test]
    fn the_line_run_twice_in_wsl_adds_one_source_line_naming_the_snippet() {
        let _alone = WSL.lock().unwrap_or_else(|e| e.into_inner());
        if !cfg!(windows) {
            return;
        }
        let ready = wsl_bash("echo tern-ready\n", |_| ());
        if !ready.is_some_and(|out| out.contains("tern-ready")) {
            eprintln!("skipped: no WSL bash on this machine");
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shell-integration");
        assert!(dir.join("tern.bash").is_file() && dir.join("tern.zsh").is_file());
        let line = add_line(&dir);
        let script = format!(
            r#"tern_home=$(mktemp -d /tmp/tern-offer.XXXXXX) || exit 3
case "$tern_home" in /tmp/tern-offer.*) ;; *) exit 3 ;; esac
export HOME=$tern_home
cd "$tern_home" || exit 3
{line}
{line}
report() {{
  echo "tern-$1-lines=$(grep -c . "$tern_home/$2")"
  tern_path=$(sed -n 's/^\. "\(.*\)"$/\1/p' "$tern_home/$2")
  echo "tern-$1-path=$tern_path"
  [ -f "$tern_path" ] && echo "tern-$1-exists"
}}
report bash .bashrc
[ -e "$tern_home/.zshrc" ] && echo tern-bash-wrote-zshrc
if command -v zsh >/dev/null 2>&1; then
  cat > "$tern_home/add" <<'TERN_EOF'
{line}
TERN_EOF
  HOME=$tern_home zsh -f "$tern_home/add" </dev/null >/dev/null 2>&1
  HOME=$tern_home zsh -f "$tern_home/add" </dev/null >/dev/null 2>&1
  report zsh .zshrc
  echo "tern-after-zsh-lines=$(grep -c . "$tern_home/.bashrc")"
else
  echo tern-no-zsh
fi
rm -rf "$tern_home"
echo tern-done
"#
        );
        let out = wsl_bash(&script, |_wsl| {
            #[cfg(windows)]
            {
                assert!(runs_wsl(std::process::id()), "wsl.exe under the test");
                assert!(runs_wsl(_wsl), "wsl.exe itself");
            }
        })
        .expect("the line returns in WSL bash");
        let has = |what: &str| out.lines().any(|l| l.trim_end().ends_with(what));
        assert!(has("tern-done"), "{out}");
        assert!(has("tern-bash-lines=1"), "{out}");
        assert!(has("/shell-integration/tern.bash"), "{out}");
        assert!(has("tern-bash-exists"), "{out}");
        assert!(!has("tern-bash-wrote-zshrc"), "{out}");
        if has("tern-no-zsh") {
            eprintln!("zsh half skipped: no zsh in this WSL distro");
        } else {
            assert!(has("tern-zsh-lines=1"), "{out}");
            assert!(has("/shell-integration/tern.zsh"), "{out}");
            assert!(has("tern-zsh-exists"), "{out}");
            assert!(has("tern-after-zsh-lines=1"), "{out}");
        }
    }

    /// The process tree `1 → 2 → 3`, with `9` alone under a parent that
    /// is gone.
    fn tree() -> HashMap<u32, u32> {
        HashMap::from([(2, 1), (3, 2), (9, 77)])
    }

    /// Spec 0022, flow 1: a `wsl.exe` counts when it is the pane's
    /// program, its child, or deeper in its tree.
    #[test]
    fn a_process_descends_from_itself_its_parent_and_its_grandparent() {
        assert!(descends_from(1, 1, &tree()));
        assert!(descends_from(2, 1, &tree()));
        assert!(descends_from(3, 1, &tree()));
        assert!(descends_from(3, 2, &tree()));
    }

    /// Spec 0022, flow 1 and AC7: a `wsl.exe` elsewhere (another pane's,
    /// or one whose parent already exited) doesn't make this pane a WSL
    /// one, nor does the pane's own parent.
    #[test]
    fn an_unrelated_process_or_an_ancestor_does_not_descend() {
        assert!(!descends_from(9, 1, &tree()));
        assert!(!descends_from(1, 2, &tree()));
        assert!(!descends_from(2, 3, &tree()));
        assert!(!descends_from(42, 1, &tree()));
        assert!(!descends_from(42, 1, &HashMap::new()));
    }

    /// Spec 0022, flow 1: a reused process ID can make the parents loop;
    /// the look still ends, with "no" for a root outside the loop and
    /// "yes" for one on it.
    #[test]
    fn a_loop_of_parents_ends_the_search() {
        let own = HashMap::from([(5, 5)]);
        assert!(!descends_from(5, 1, &own));
        let pair = HashMap::from([(5, 6), (6, 5), (7, 6)]);
        assert!(!descends_from(5, 1, &pair));
        assert!(!descends_from(7, 1, &pair));
        assert!(descends_from(7, 5, &pair));
    }
}
