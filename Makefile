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

dmg: app sparkle-tools
	@packaging/build-dmg.sh

# Sparkle's command-line tools (generate_keys, generate_appcast), fetched once from the
# release the framework came from. Not vendored: 10 MB of somebody else's binaries.
sparkle-tools: packaging/sparkle/bin/generate_appcast
packaging/sparkle/bin/generate_appcast:
	@mkdir -p packaging/sparkle
	@gh release download -R sparkle-project/Sparkle --pattern 'Sparkle-*.tar.xz' -O packaging/sparkle/sparkle.tar.xz --clobber
	@tar -xJf packaging/sparkle/sparkle.tar.xz -C packaging/sparkle
	@rm -f packaging/sparkle/sparkle.tar.xz

# One-time on the release machine: the EdDSA keypair. The private key goes to the keychain;
# the public one is printed, and build-app.sh reads it from there afterwards.
sparkle-keys: sparkle-tools
	@packaging/sparkle/bin/generate_keys

# Build, sign, notarise, write the appcast, and publish a GitHub release the updater feed
# points at. After this, every installed Keel offers the update by itself.
release: dmg
	@version="$$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"; \
	gh release create "v$$version" dist/Keel.dmg dist/appcast.xml \
	  --title "Keel $$version" --notes "See CHANGELOG.md" --latest || \
	gh release upload "v$$version" dist/Keel.dmg dist/appcast.xml --clobber
	@echo "    released v$$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"

.PHONY: app dmg sparkle-tools sparkle-keys release
