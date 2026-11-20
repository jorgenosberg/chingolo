<img src="src-tauri/icons/128x128@2x.png" alt="Chingolo app icon" width="96">

# Chingolo

A little help for big ideas.

A local-first desktop app for students who work with academic literature. Import papers, ask questions across them, and follow every answer back to the source passage. Bring your own AI key. Your documents stay on your Mac.

Status: early development. macOS on Apple Silicon only.

## Install

Download the `.dmg` from the [latest release](https://github.com/jorgenosberg/chingolo/releases/latest) and drag Chingolo to Applications. The app is not notarized (no paid Apple Developer account), so macOS blocks it twice: once when you open the `.dmg`, and again when you first launch the app. Each time, open System Settings > Privacy & Security and choose Open Anyway. Or, after dragging the app to Applications, skip the second block by running:

```sh
xattr -dr com.apple.quarantine /Applications/Chingolo.app
```

Later versions install from inside the app, which checks for updates when it starts. Add an API key for Gemini or OpenAI in Settings.

## Development

Needs Rust, bun and Xcode command line tools.

```sh
bun install
src-tauri/scripts/fetch-pdfium.sh
bun run tauri dev
```

Dev builds are signed with a local Apple Development certificate when one is installed, so macOS Keychain access is remembered across rebuilds. See `src-tauri/scripts/dev-run.sh`.

See `docs/ARCHITECTURE.md` for how the code is laid out. `scripts/release.sh` builds and publishes a signed release.

## License

MIT, see `LICENSE`. Third-party components and downloaded models are listed in `NOTICE.md`.
