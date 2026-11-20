#!/bin/bash
# Builds a signed release and publishes it on GitHub Releases.
#
#   scripts/release.sh 0.1.1 ["release notes"]
#   DRY_RUN=1 scripts/release.sh 0.1.1       build and write latest.json, publish nothing
#
# Needs the updater signing key (default ~/.tauri/chingolo.key, override with CHINGOLO_SIGNING_KEY)
# and its password (asked for, or set TAURI_SIGNING_PRIVATE_KEY_PASSWORD). Run it from a normal
# terminal so the prompt works. Apple Silicon only.
set -euo pipefail
cd "$(dirname "$0")/.."

version=${1:?usage: scripts/release.sh <version> [notes]}
notes=${2:-}
repo=jorgenosberg/chingolo
key=${CHINGOLO_SIGNING_KEY:-$HOME/.tauri/chingolo.key}
tag="v$version"

[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3" >&2; exit 1; }
[ -f "$key" ] || { echo "signing key not found: $key" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the working tree has changes, commit or stash them first" >&2; exit 1; }
[ -f src-tauri/vendor/pdfium/libpdfium.dylib ] || { echo "libpdfium is missing, run src-tauri/scripts/fetch-pdfium.sh" >&2; exit 1; }

if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+x}" ]; then
	read -r -s -p "Signing key password: " TAURI_SIGNING_PRIVATE_KEY_PASSWORD
	echo
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
TAURI_SIGNING_PRIVATE_KEY=$(cat "$key")
export TAURI_SIGNING_PRIVATE_KEY

# A stable signing identity keeps Keychain "Always Allow" choices across updates.
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
	APPLE_SIGNING_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null |
		grep "Apple Development" | grep -v REVOKED | head -1 | awk '{print $2}' || true)
	[ -n "$APPLE_SIGNING_IDENTITY" ] && export APPLE_SIGNING_IDENTITY
fi

echo "== version $version"
jq --arg v "$version" '.version = $v' src-tauri/tauri.conf.json >src-tauri/tauri.conf.json.new
mv src-tauri/tauri.conf.json.new src-tauri/tauri.conf.json
jq --arg v "$version" '.version = $v' package.json >package.json.new
mv package.json.new package.json
perl -0pi -e "s/^version = \"[^\"]*\"/version = \"$version\"/m" src-tauri/Cargo.toml
# jq reindents the files, so put the repo's formatting back.
bunx biome check --write package.json src-tauri/tauri.conf.json >/dev/null

echo "== build (signing identity: ${APPLE_SIGNING_IDENTITY:-ad hoc})"
bun run tauri build

out=src-tauri/target/release/bundle
archive=$out/macos/Chingolo.app.tar.gz
dmg=$out/dmg/Chingolo_${version}_aarch64.dmg
for f in "$archive" "$archive.sig" "$dmg"; do [ -f "$f" ] || { echo "missing build output: $f" >&2; exit 1; }; done

jq -n --arg v "$version" --arg notes "$notes" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
	--arg sig "$(cat "$archive.sig")" \
	--arg url "https://github.com/$repo/releases/download/$tag/Chingolo.app.tar.gz" \
	'{version: $v, notes: $notes, pub_date: $date,
	  platforms: {"darwin-aarch64": {signature: $sig, url: $url}}}' >"$out/latest.json"
echo "== wrote $out/latest.json"

if [ "${DRY_RUN:-}" = 1 ]; then
	echo "dry run: nothing committed or published. Undo the version bump with: git checkout ."
	exit 0
fi

read -r -p "Commit the version bump, push, and publish $tag on $repo? [y/N] " answer
[ "$answer" = y ] || { echo "stopped. Undo the version bump with: git checkout ."; exit 1; }
git add -A
if ! git diff --cached --quiet; then git commit -m "chore: release $tag"; fi
git push origin HEAD
gh release create "$tag" --repo "$repo" --target "$(git rev-parse HEAD)" --title "Chingolo $version" \
	--notes "${notes:-Chingolo $version}" "$archive" "$archive.sig" "$dmg" "$out/latest.json"
echo "== published https://github.com/$repo/releases/tag/$tag"
