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

# Render the side panels in a headless DOM against a running Keel. Every UI bug in this project so
# far has been "the code did not run" rather than "it looked wrong", and that is what this catches.
# Needs `keel serve` on :7777 and `bun install` in ui/.
ui:
	@bun ui/check.mjs

.PHONY: ui
