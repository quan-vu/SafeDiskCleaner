.PHONY: test lint build bundle dmg run

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

build:
	cargo build --release -p sdc-app

bundle:
	./scripts/bundle.sh

dmg:
	./scripts/bundle.sh --dmg

run: bundle
	open "dist/Safety Disk Cleaner.app"
