#!/usr/bin/env bash
# Builds Tern's package repositories (T-031) into one static site: a dnf
# and zypper repository under rpm/, a flat apt repository under deb/, and
# the public key at the root. Every .rpm and .deb in <packages> goes in,
# signed with GPG key <key>, which must be in the keyring without a
# passphrase. Needs rpmsign, createrepo_c, apt-ftparchive and gpg.
# Usage: packaging/linux/repositories.sh <packages> <site> <key> <base URL>
set -euo pipefail

packages=$1
site=$2
key=$3
base=${4%/}

rm -rf "$site"
mkdir -p "$site/rpm" "$site/deb"
gpg --armor --export "$key" > "$site/tern.asc"
sign() { gpg --batch --yes --local-user "$key" "$@"; }

# dnf and zypper: each package signed, then the metadata.
cp "$packages"/*.rpm "$site/rpm/"
rpmsign --define "_gpg_name $key" --define "__gpg $(command -v gpg)" --addsign "$site"/rpm/*.rpm
createrepo_c --quiet "$site/rpm"
sign --armor --detach-sign --output "$site/rpm/repodata/repomd.xml.asc" "$site/rpm/repodata/repomd.xml"
cat > "$site/rpm/tern.repo" <<REPO
[tern]
name=Tern
baseurl=$base/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$base/tern.asc
REPO

# apt: a flat repository, signed as InRelease.
cp "$packages"/*.deb "$site/deb/"
(
  cd "$site/deb"
  apt-ftparchive packages . > Packages
  gzip --keep --best Packages
  apt-ftparchive release . > Release
)
sign --clearsign --output "$site/deb/InRelease" "$site/deb/Release"
cat > "$site/deb/tern.sources" <<SOURCES
Types: deb
URIs: $base/deb/
Suites: ./
Signed-By: /etc/apt/keyrings/tern.asc
SOURCES

# Pages serves the site as it is, without Jekyll.
touch "$site/.nojekyll"
