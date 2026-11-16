.PHONY: test lint build bundle dmg install run

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

# Build and install the single latest copy in /Applications.
install:
	./scripts/install.sh

run:
	./scripts/install.sh --open
