# Safety Disk Cleaner — Rust native

Native macOS rewrite (Rust + AppKit through `objc2`) of the Go app in the parent folder.
Same safety rules, same SQLite database (`~/Library/Application Support/Safety Disk Cleaner/cleaner.db`),
new UI: translucent sidebar, cards, live disk-activity chart.

## Layout

| Path | What |
|---|---|
| `crates/sdc-core` | Pure logic, unit-tested: safety rules (`model.rs`), scan (`scan.rs`), re-validation before cleanup (`action.rs`), Claude CLI review (`review.rs`), SQLite store (`store.rs`), live disk stats via IOKit/libproc (`sysstats.rs`) |
| `crates/sdc-app` | AppKit app: window + sidebar + pages (`pages.rs`), state/actions (`app.rs`), custom views (`ui.rs`), Trash/notifications/login item/tray (`system.rs`) |
| `bundle/Info.plist` | App bundle metadata (`LSUIElement`, bundle id shared with the Go app) |
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

- The bundle id matches the Go app, so only one of the two runs at a time (a second launch shows the running window).
- The UI language follows the system's preferred language (Vietnamese or English).
- Trash uses `NSFileManager.trashItemAtURL` directly; the Swift `trash-helper` is no longer needed.
- Ad-hoc signed only; notarization (Developer ID) is not set up yet.

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
