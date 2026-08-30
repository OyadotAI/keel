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
#
# Published to a *public* releases-only repository: this one is private, and Sparkle on a
# tester's machine has no token. The DMG and the appcast are all that repo ever holds.
RELEASES = OyadotAI/keel-releases
# The version is the workspace version in Cargo.toml, and the release moves it: bumping by
# hand and forgetting was how two builds went out calling themselves the same thing, which
# Sparkle then refuses to offer. `make release` bumps the patch; `make release VERSION=0.3.0`
# sets it. The bump is committed before the build, so the version in the bundle is the version
# in the tag.
release:
	@current="$$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"; \
	next="$(VERSION)"; \
	if [ -z "$$next" ]; then \
	  next="$$(echo "$$current" | awk -F. '{ printf "%d.%d.%d", $$1, $$2, $$3 + 1 }')"; \
	fi; \
	if [ "$$next" = "$$current" ]; then echo "    version unchanged ($$current)"; else \
	  sed -i '' "s/^version = \"$$current\"/version = \"$$next\"/" Cargo.toml; \
	  cargo build -p keel --quiet; \
	  git commit -qm "chore: $$next" -- Cargo.toml Cargo.lock; \
	fi; \
	$(MAKE) dmg; \
	gh release create "v$$next" dist/Keel.dmg dist/appcast.xml -R $(RELEASES) \
	  --title "Keel $$next" --notes "Signed and notarised. Installed copies are offered the update." --latest || \
	gh release upload "v$$next" dist/Keel.dmg dist/appcast.xml -R $(RELEASES) --clobber; \
	git push -q origin main || true; \
	echo "    released v$$next"

.PHONY: app dmg sparkle-tools sparkle-keys release
