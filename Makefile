.PHONY: help build check fmt lint test scan clean

help:
	@echo "build   compile the workspace"
	@echo "check   fmt + clippy + tests, the gate everything must pass"
	@echo "test    run tests"
	@echo "scan    scan this repository with the freshly built binary"

build:
	cargo build

check: fmt lint test

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

scan: build
	cargo run --quiet -- scan .

clean:
	cargo clean

# ── macOS packaging ──────────────────────────────────────────────────────────
app:
	@packaging/build-app.sh

dmg: app
	@packaging/build-dmg.sh

.PHONY: app dmg
