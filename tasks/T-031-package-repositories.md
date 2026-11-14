---
status: next
blocked-by: [T-007]
---
# T-031: dnf and apt repositories on GitHub Pages

A Fedora, openSUSE, Debian or Ubuntu user adds Tern's repository once and
gets updates with `dnf upgrade`, `zypper up` or `apt upgrade`. Both
repositories ship together, signed with one GPG key. Area: meta.

- `release.yml`, after the release is created: download the `.rpm` and
  `.deb` of every release (`gh release download`), build both
  repositories, and publish them as one site with `actions/deploy-pages`.
  Nothing binary is committed to git.
- **dnf/zypper (`rpm/`):** sign each `.rpm` with `rpmsign`, run
  `createrepo_c`, sign `repodata/repomd.xml`. The site serves `tern.repo`
  (`gpgcheck=1`, `repo_gpgcheck=1`). Users run
  `sudo dnf config-manager addrepo --from-repofile=https://thezedrem.github.io/<repo>/rpm/tern.repo`,
  then `sudo dnf install tern`.
- **apt (`deb/`):** a flat repository: `apt-ftparchive` writes `Packages`
  and `Release`, signed as `InRelease`. The site serves `tern.sources`
  (deb822, `Signed-By: /etc/apt/keyrings/tern.asc`). Users download the key
  to `/etc/apt/keyrings/tern.asc` and `tern.sources` to
  `/etc/apt/sources.list.d/`, then `sudo apt update && sudo apt install tern`.
- The public key is served once at the site's root.
- Pages allows 1 GB per site, and every release adds an `.rpm` and a
  `.deb` (~20 MB): once that gets close, publish only the latest
  releases; older ones stay in GitHub Releases.
- Owner's steps: create the GPG signing key and store it as a CI secret,
  and switch Pages to "GitHub Actions" as its source.
- `docs/development.md` says how the repositories are published, and
  `README.md` how to install from them.
- Check: on Fedora, openSUSE, Debian 13 and Ubuntu 24.04, add the
  repository, install, tag a newer version, and the system's update picks
  it up with signatures verified.

**Prepared (2026-11-14):** `.github/workflows/repositories.yml` runs after
each Release (and by hand) and publishes through
`packaging/linux/repositories.sh`; `docs/development.md` describes it.
Checked in containers with a throwaway key and the v0.9.0 packages: the
site built on Ubuntu 24.04, and `dnf install tern` on Fedora 45 and
`apt install tern` on Debian 13 installed from it with signatures checked.

**Live (2026-11-14):** the owner ran the setup script; the site is at
https://thezedrem.github.io/tern/ and README's Install section has the
commands. With them, v0.9.0 installed from the live site on Fedora 45,
openSUSE Tumbleweed, Debian 13 and Ubuntu 24.04 (containers), signatures
checked; the owner also installed it on a second PC. Left: with the next tag, check that each system's update picks
up the newer version.

Spec: ../spec/architecture.md
