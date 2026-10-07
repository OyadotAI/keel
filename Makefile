.PHONY: help build check fmt lint test scan clean dev

help:
	@echo "build   compile the workspace"
	@echo "check   fmt + clippy + tests, the gate everything must pass"
	@echo "test    run tests"
	@echo "scan    scan this repository with the freshly built binary"
	@echo "dev     run the desktop app (desktop/, Tauri) against a daemon built from this tree"

build:
	cargo build

check: fmt lint test desktop-test

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

.PHONY: desktop-test dev

# The cross-platform app (desktop/, Tauri). Type-checked, its reducer tested, and bundled — the
# bundle is what the 400 KB budget is measured on. Installs its own dependencies the first time.
desktop-test:
	cd desktop && pnpm install --frozen-lockfile --silent && pnpm exec tsc --noEmit && pnpm lint && pnpm test && pnpm test:runtime && pnpm runtime:build && pnpm runtime:stage && pnpm build && pnpm budget
	packaging/sidecar.sh && cd desktop/src-tauri && cargo test --quiet

# The desktop app with a live reload, against a daemon built from this tree.
dev:
	packaging/sidecar.sh && cd desktop && pnpm tauri dev

scan: build
	cargo run --quiet -- scan .

clean:
	cargo clean

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
# no updater then offers. The app's version is the shell's (desktop/src-tauri/Cargo.toml), moved
# in step, and the workflow refuses a tag either disagrees with. `make release` bumps the patch; `make release VERSION=0.3.0`
# sets it. The bump is committed before the tag, and the workflow refuses a tag that disagrees.
release:
	@set -e; \
	current="$$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"; \
	next="$(VERSION)"; \
	if [ -z "$$next" ]; then \
	  next="$$(echo "$$current" | awk -F. '{ printf "%d.%d.%d", $$1, $$2, $$3 + 1 }')"; \
	fi; \
	if git rev-parse -q --verify "refs/tags/v$$next" >/dev/null; then \
	  echo "    v$$next is already released — run make release with a newer VERSION"; exit 1; \
	fi; \
	if [ "$$next" = "$$current" ]; then echo "    version unchanged ($$current)"; else \
	  sed -i '' "s/^version = \"$$current\"/version = \"$$next\"/" Cargo.toml; \
	  sed -i '' "s/^version = \"$$current\"/version = \"$$next\"/" desktop/src-tauri/Cargo.toml; \
	  cargo build -p keel --quiet; \
	  (cd desktop/src-tauri && cargo update -p keel-desktop --quiet); \
	  git commit -qm "chore: $$next" -- Cargo.toml Cargo.lock desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock; \
	fi; \
	git push -q origin HEAD; \
	git tag -a "v$$next" -m "Keel $$next"; \
	git push -q origin "v$$next"; \
	echo "    tagged v$$next — the release workflow builds, signs, notarises and publishes it"

# One-time: the updater's signing key pair. The private half stays in ~/.tauri and goes into the
# repository's TAURI_SIGNING_PRIVATE_KEY secret; the public half is in tauri.conf.json already.
updater-keys:
	@test -f ~/.tauri/keel-updater.key || (cd desktop && pnpm tauri signer generate --ci -w ~/.tauri/keel-updater.key)
	@echo "    gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/keel-updater.key"

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

.PHONY: sparkle-tools sparkle-keys updater-keys release evals
