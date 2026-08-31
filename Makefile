.PHONY: help build check fmt lint test clean

help:
	@echo "build   compile the workspace"
	@echo "check   fmt + clippy + tests, the gate everything must pass"
	@echo "test    run tests"

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

# Bump the version, commit it, and push the tag. `.github/workflows/release.yml` does the rest:
# build, sign, notarise, write the appcast, publish. Building it here instead is how a release
# came to mean one laptop, one keychain and one person — the tag is the handover point now.
#
# The release is published to a *public* releases-only repository: this one is private, and
# Sparkle on a tester's machine has no token. The DMG and the appcast are all that repo holds.
#
# The version is the workspace version in Cargo.toml, and the release moves it: bumping by
# hand and forgetting was how two builds went out calling themselves the same thing, which
# Sparkle then refuses to offer. `make release` bumps the patch; `make release VERSION=0.3.0`
# sets it. The bump is committed before the tag, and the workflow refuses a tag that disagrees.
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
	git push -q origin HEAD; \
	git tag -a "v$$next" -m "Keel $$next"; \
	git push -q origin "v$$next"; \
	echo "    tagged v$$next — the release workflow builds, signs, notarises and publishes it"

# Ten evals: the product's promises against a real agent — the daemon, the hook, `claude`, the gate.
#
# Not part of `make check`, because it spends real tokens — a few cents — and takes a minute. It
# is the test the 336 unit tests could not be: `keel approve` being passed an argument it did not
# accept is a fact about two files agreeing, and no test of either file could see it, so a build
# that refused every Bash call shipped to people.
#
# Run it before `make release`. `KEEL_RECORD=<path>` also keeps the first eval's stream, which is
# where the replay fixtures come from. Four of the ten cost nothing and run with `make check`.
evals:
	@KEEL_EVALS=1 cargo test -p keel --test evals -- --nocapture --test-threads=1

.PHONY: app dmg sparkle-tools sparkle-keys release evals
