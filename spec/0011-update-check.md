---
status: implemented        # draft | approved | implemented
phase: 2
tasks: [T-029]
related: [spec 0001, spec 0005, spec 0009, OQ-022, T-026]
---

# Spec: Checking GitHub for a new release

Pins an opt-in check for a new Tern release on GitHub: asked once after
the first launch, then done daily or never. It only tells the user;
installing stays with their package manager.

## Problem & goal

Tern ships as `.deb`, `.rpm` and AppImage (T-026). AppImage users, and
anyone who installed a downloaded package by hand, get no updates and
never hear about new releases. Contacting GitHub sends the user's IP
address to a US service, so Tern must not do it unasked (ground rule 1).

Done means: the first launch asks whether to check for updates. With
"Check daily", a new release on GitHub shows a notice that links its
release notes. With "Don't check", Tern never contacts GitHub unless the
user runs the palette command.

## Scope

**In scope:**

- A setting, `update_check: daily | off`, on the settings screen and in the config template; it has no default
- A one-time question while the setting is unset
- A daily background check against GitHub's latest-release API while it's `daily`
- A palette command, `app.checkForUpdates`, that checks now, whatever the setting says
- A notice when a newer release exists, with a button that opens its release page

**Out of scope:**

- Downloading or installing anything, or replacing the binary: package managers do that [load-bearing]
- Pre-releases, release channels, and a "skip this version" option
- Changing the interval: daily is fixed
- An apt/dnf repository or winget manifest (*Later*: own task once T-026 ships)

## Design

### Affected areas

- `src/update.rs` [new]: the check (HTTP, parsing, comparing versions), the state file and the scheduling
- `src/config.rs` and `src/default-config.yaml`: the `update_check` key
- the settings screen (spec 0009): General → Updates
- `commands/`: `app.checkForUpdates`
- the workspace view: the question and the update notice, both after the first frame

### Model

- **Setting:** `update_check` is `Option<UpdateCheck>` with
  `UpdateCheck = Daily | Off`. "Not chosen yet" is the missing key and
  nothing else: there's no `ask` value, and nothing ever writes one
  [load-bearing]. Any other value is a config error under spec 0005's
  rules and is treated as unset, so the question still appears. The
  template has the key commented out with both values explained, because
  an uncommented line would answer the question for the user. Answering
  the question writes the key through spec 0009's in-place edit.
- **Settings screen:** General → Updates offers "Check daily" and
  "Don't check". While the key is unset, neither is selected and the
  field says the question is still open. Picking one writes it. The
  screen can't go back to unset; deleting the line by hand does that.
- **Source:** `GET https://api.github.com/repos/TheZedrem/tern/releases/latest`
  sent with `User-Agent: tern/<version>` and nothing else: no OS, no ID and
  no cookies [load-bearing]. GitHub's `latest` already leaves out drafts and
  pre-releases. Only `tag_name` and `html_url` are read.
- **Versions:** a tag must match `vMAJOR.MINOR.PATCH` exactly and is
  compared to `CARGO_PKG_VERSION` as three integers. Any other tag means
  there's no update. That's the whole parser, with no `semver` crate.
- **Release URL:** it opens only if it starts with
  `https://github.com/TheZedrem/tern/releases/` [load-bearing]. Anything
  else counts as a failed check.
- **State:** `<data_local_dir>/tern/update.json` holds only `last_check` (Unix
  seconds). It's written atomically (temp file plus rename). If it's
  missing or doesn't parse, it counts as empty. If two instances race on it, the worst case is
  one extra request.
- **HTTP client:** a blocking client on its own thread, with TLS that uses
  the OS's certificate store and honors `HTTPS_PROXY`. A 10 s timeout and a
  64 KiB response cap. The crate is `ureq` 3 with `rustls` and its platform verifier (OQ-022).

### Flows

1. **Boot:** none of this runs before the first frame [load-bearing].
   After the first frame the setting decides what happens next:
   unset → flow 2, `daily` → flow 3, `off` → nothing.
2. **Question:** a notice that stays until it's answered: "Check GitHub
   for new Tern releases once a day? This sends your IP address to GitHub."
   It has two buttons, "Check daily" and "Don't check". A button writes
   `daily` or `off`, and "Check daily" starts flow 3 right away. Closing the
   notice without picking writes nothing, so the question comes back on the
   next launch. A failed write shows spec 0009's failure notice. The notice
   never takes focus from the pane.
3. **Daily check:** read the state file on a background thread. If
   `last_check` is under 24 h old, wait until it's due. Otherwise check
   (flow 4) and save `last_check`, whatever the result. Repeat every 24 h
   while Tern runs. Switching to `off` or removing the key cancels the timer at the
   next config reload, and switching to `daily` starts it.
4. **Check:** send the request. If the release is newer than the running
   version, show "Tern X.Y.Z is available" with an "Open release page"
   button. This happens on every check that finds a newer release, so an
   ignored update shows again the next day. The notice stays until it's
   closed, and its button opens the URL through `cx.open_url`. A notice
   still open from an earlier check is replaced, not stacked.
5. **Failure:** offline, a timeout, a non-200 response (rate limits
   included), an oversized or malformed body, or a bad URL. An automatic
   check fails silently and tries again in 24 h. Otherwise an offline
   laptop would get a notice every day; this is a stated exception to the
   "every error is a notice" rule (architecture §Errors).
6. **Command:** `app.checkForUpdates` runs flow 4 now, under any setting,
   because the user started it. If Tern is up to date, it says so.
   A failure shows as an error notice with the reason.

### Startup impact

None on the boot path: no file read, thread or socket before the first
frame. The first-frame hook decides from the already-loaded config
whether to spawn anything. T-029's commit reports the TTFP to show it.

### Platform notes

- TLS must verify against the OS store on both platforms: the system CA
  bundle on Linux, Windows' certificate store on Windows 11. That way
  corporate TLS proxies work as they do in the browser.
- The release notice links to GitHub's page and doesn't tell users how to
  install the update. The steps differ between `.deb`, `.rpm`, AppImage and
  Windows, and the release page lists them.
- `cx.open_url` uses `xdg-open` (portal) on Linux and the shell's default
  browser on Windows. Check both.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| The user's IP address, `tern/<version>` | Sent to GitHub (Microsoft, US) on each check | Finding out whether a newer release exists | Consent: the question in flow 2 names GitHub and the IP; withdrawn by setting `off` | GitHub's policy, outside Tern's control |
| `last_check` | `<data_local_dir>/tern/update.json` | Checking at most daily | The user's own device; no one else processes it | Until uninstall; overwritten on each check |

Ground rules touched: **1** (no network at startup: nothing before the
first frame, and nothing at all without consent or the command) and **2**
(no telemetry: the request carries nothing about the user or the
machine). `architecture.md` §Ground rules records the update check as
the one network call the user agrees to once instead of starting each
time.

Trust boundaries:

- **The response is untrusted:** its size is capped, only two fields are
  parsed, the version must be three integers, and the URL must match the
  fixed prefix. Nothing from it is run or written to disk,
  and only the parsed version is shown.
- **No download, no execution [load-bearing].** This is a notice only.
  There's nothing in it to sign or verify, so it needs no threat model of
  its own.

## Acceptance criteria

1. With no `update_check` in the config, the question appears after the first frame. "Check daily" writes `update_check: daily` and "Don't check" writes `update_check: off`, each without changing another line of `config.yaml`.
2. Closing the question without picking writes nothing, and the question appears again on the next launch.
3. With `off` or the key unset, Tern makes no network connection at all during a session in which the command isn't run.
4. With `daily` and a GitHub tag newer than the running version, the notice names that version, and its button opens the release page.
5. Each daily check that finds a newer release shows the notice again, also for a release it already showed; a second check's notice replaces the first instead of stacking.
6. Launches less than 24 h after a check send no request. A Tern left running for more than 24 h checks again.
7. A tag that isn't `vX.Y.Z`, a URL outside `https://github.com/TheZedrem/tern/releases/`, an oversized body or a timeout shows no update notice. Through the command, each shows an error notice instead.
8. `app.checkForUpdates` checks with the setting `off`, and says "up to date" when there's no newer release.
9. The request carries no header that identifies the user or the OS beyond `User-Agent: tern/<version>`.
10. The setting is on the settings screen under General → Updates, shows neither choice while unset, and can only write `daily` or `off`; the TTFP is unchanged within noise.

## Open questions

- None.
