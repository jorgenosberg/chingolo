# Chingui

A little help for big ideas.

A local-first desktop app for students who work with academic literature. Import papers, ask questions across them, follow every answer back to the source passage, and keep what you find as organized evidence. Bring your own AI key. Your documents stay on your Mac.

Status: early development (Milestone 1). The name is provisional.

## Development

Needs Rust, bun and Xcode command line tools.

```sh
bun install
bun run tauri dev
```

Dev builds are signed with a local Apple Development certificate when one is installed, so macOS Keychain access is remembered across rebuilds. See `src-tauri/scripts/dev-run.sh`.

See `docs/PLAN.md`, `docs/ARCHITECTURE.md` and `docs/STATUS.md`.

## License

MIT
