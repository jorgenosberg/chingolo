#!/bin/sh
# Cargo runner for dev builds. Signs the app binary with a local Apple Development certificate
# so the macOS Keychain "Always Allow" choice survives rebuilds. Ad-hoc signatures change on
# every build, which makes macOS ask again. Does nothing if no certificate is installed.
bin="$1"
if [ "$(basename "$bin")" = "chingui" ]; then
	id=$(security find-identity -v -p codesigning 2>/dev/null | grep "Apple Development" | grep -v REVOKED | head -1 | awk '{print $2}')
	if [ -n "$id" ]; then
		codesign --force --sign "$id" --identifier app.chingui.desktop "$bin" 2>/dev/null ||
			echo "warning: could not sign $bin; Keychain may prompt on every run" >&2
	fi
fi
exec "$@"
