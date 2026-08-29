.PHONY: help build check fmt lint test scan clean

help:
	@echo "build   compile the workspace"
	@echo "check   fmt + clippy + tests, the gate everything must pass"
	@echo "test    run tests"
	@echo "scan    scan this repository with the freshly built binary"

build:
	cargo build

check: fmt lint test app-test

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

# The Swift half of the gate: the stream parser, and the budgets from the plan that can actually
# fail — orphaned agent processes, a second daemon, a bundle that grew.
app-test:
	swift test --package-path app

.PHONY: app-test

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
