# SafeDiskCleaner

Native macOS disk-cleanup assistant written in Rust + AppKit (through `objc2`).
It finds large, old files, asks the Claude CLI for a conservative second opinion
(metadata only, never file contents) and lets you move them to the Trash. Nothing is
ever deleted automatically.

Data lives in `~/Library/Application Support/Safety Disk Cleaner/cleaner.db`
(the same database the earlier Go version used, so settings carry over).

## Layout

| Path | What |
|---|---|
| `crates/sdc-core` | Pure logic, unit-tested: safety rules (`model.rs`), scan (`scan.rs`), re-validation before cleanup (`action.rs`), Claude CLI review (`review.rs`), SQLite store (`store.rs`), live disk stats via IOKit/libproc (`sysstats.rs`) |
| `crates/sdc-app` | AppKit app: window + sidebar + pages (`pages.rs`), state/actions (`app.rs`), custom views (`ui.rs`), Trash/notifications/login item/tray (`system.rs`) |
| `bundle/Info.plist` | App bundle metadata (`LSUIElement`, bundle id kept from the Go version) |
| `scripts/bundle.sh` | Builds `dist/Safety Disk Cleaner.app` (ad-hoc signed), `--dmg` for a DMG |

## Commands

```bash
make test     # cargo test --workspace
make lint     # clippy, warnings as errors
make bundle   # dist/Safety Disk Cleaner.app
make dmg      # + dist/safety-disk-cleaner-<version>-arm64.dmg
make run      # bundle and open
```

## Notes

- Only one instance runs at a time; a second launch shows the running window.
- The UI language follows the system's preferred language (Vietnamese or English).
- Trash uses `NSFileManager.trashItemAtURL`.
- Ad-hoc signed only; notarization (Developer ID) is not set up yet.
- CI (`.github/workflows/build.yml`) runs tests and clippy and uploads the `.app` zip and DMG as build artifacts; pushing a `v*` tag also attaches them to a GitHub Release.

## Testing with fake files (never touches real data)

```bash
scripts/make-test-files.sh   # dev-sandbox/home: 18 GB of *sparse* old files (~0 real disk)
scripts/run-sandbox.sh       # quits the real app, runs it with HOME=dev-sandbox/home
```

The sandbox has its own settings database, so the real one is not read or written.
It contains 5 files that should be suggested and 7 decoys that must not be (recent,
small, hidden folder, git checkout, `.photoslibrary`, `~/Library`, symlink).
Set `SDC_DEBUG=1` (done by `run-sandbox.sh`) to trace actions to `dev-sandbox/app.log`.
Items moved to the Trash from the sandbox land in your real Trash (they take no space).
