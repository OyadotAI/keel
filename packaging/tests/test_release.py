# Verify Factory display names and release compatibility using only scratch bundles and stdlib.
import json
import os
import subprocess
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[2]
CONFIG = ROOT / "desktop/src-tauri/tauri.conf.json"
WORKFLOW = ROOT / ".github/workflows/release.yml"
BUNDLE = Path("desktop/src-tauri/target/universal-apple-darwin/release/bundle")
APP_NAME = "Oya Factory"
LEGACY_ARCHIVE = "Keel.app.tar.gz"


class ReleaseBrandingTests(unittest.TestCase):
    """Keep product presentation separate from persistent and published technical identities."""

    def test_display_names_change_without_changing_installed_identity_or_feed(self):
        """Existing installations retain their data, signed update feed and daemon filenames."""
        config = json.loads(CONFIG.read_text())
        self.assertEqual(APP_NAME, config["productName"])
        self.assertEqual(APP_NAME, config["app"]["windows"][0]["title"])
        self.assertEqual("ai.oya.keel", config["identifier"])
        self.assertEqual(["binaries/keel", "binaries/keel-node"], config["bundle"]["externalBin"])
        self.assertEqual(
            ["https://github.com/OyadotAI/keel-releases/releases/latest/download/latest.json"],
            config["plugins"]["updater"]["endpoints"],
        )

    def test_bundle_paths_with_spaces_keep_published_updater_aliases(self):
        """Execute the workflow's real assignments/copies against a disposable branded bundle."""
        lines = [line.strip() for line in WORKFLOW.read_text().splitlines()]
        assignment = next(line for line in lines if line.startswith("app="))
        copies = [line for line in lines if line.startswith('cp "$bundle/macos/')]
        self.assertEqual(2, len(copies))
        with TemporaryDirectory(prefix="oya factory release ") as temporary:
            root = Path(temporary)
            bundle = root / BUNDLE
            (bundle / "macos" / f"{APP_NAME}.app").mkdir(parents=True)
            (root / "out").mkdir()
            for suffix in ("", ".sig"):
                (bundle / "macos" / f"{APP_NAME}.app.tar.gz{suffix}").write_text(suffix or "archive")
            environment = {"PATH": os.defpath, "bundle": str(bundle)}
            subprocess.run(["sh", "-c", assignment + '\ntest -d "$app"'], cwd=root, env=environment, check=True)
            subprocess.run(["sh", "-c", "\n".join(copies)], cwd=root, env=environment, check=True)
            self.assertEqual("archive", (root / "out" / LEGACY_ARCHIVE).read_text())
            self.assertEqual(".sig", (root / "out" / f"{LEGACY_ARCHIVE}.sig").read_text())

    def test_runtime_smoke_uses_the_quoted_branded_bundle_path(self):
        """A space in the displayed product name must remain one runtime-smoke argument."""
        expected = f'node desktop/scripts/runtime-smoke.mjs "{BUNDLE}/macos/{APP_NAME}.app"'
        self.assertIn(expected, WORKFLOW.read_text())

    def test_windows_retains_the_legacy_installer_identity_and_factory_window_title(self):
        """NSIS uses productName for upgrade keys, independently of the bundle identifier."""
        shared = json.loads(CONFIG.read_text())
        windows = json.loads((CONFIG.parent / "tauri.windows.conf.json").read_text())
        self.assertEqual("Keel", windows["productName"])
        self.assertNotIn("app", windows)
        self.assertNotIn("identifier", windows)
        self.assertEqual(APP_NAME, shared["app"]["windows"][0]["title"])
        hook = CONFIG.parent / windows["bundle"]["windows"]["nsis"]["installerHooks"]
        self.assertTrue(hook.is_file())

    def test_windows_shortcut_hooks_keep_ownership_and_lifecycle_guards(self):
        """Pin the native contracts; compilation and real upgrade behavior need Windows coverage."""
        hook = (CONFIG.parent / "factory-installer.nsh").read_text()
        self.assertNotIn("CreateShortcut", hook)
        self.assertNotIn("!define PRODUCTNAME", hook)
        self.assertIn('!insertmacro IsShortcutTarget "${SOURCE}" "$FactoryExecutable"', hook)
        self.assertIn('!insertmacro IsShortcutTarget "${LINK}" "$FactoryExecutable"', hook)
        self.assertIn('!insertmacro IsShortcutTarget "${DESTINATION}" "$FactoryExecutable"', hook)
        self.assertIn('Delete "${SOURCE}"', hook)
        self.assertIn("${If} $FactoryInstalled == 1", hook)
        self.assertIn("${If} $UpdateMode != 1", hook)
        self.assertIn('WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "Oya Factory"', hook)
        for name in ("FactoryRenameOwnedShortcut", "FactoryRemoveOwnedShortcut"):
            macro = hook.split(f"!macro {name} ", 1)[1].split("!macroend", 1)[0]
            self.assertEqual(1, macro.count("Push $0"))
            matches = macro.count("!insertmacro IsShortcutTarget")
            self.assertEqual(matches + 1, macro.count("Pop $0"))  # results, then saved register
            for register in ("$1", "$2", "$3"):
                self.assertEqual(1, macro.count(f"Push {register}"))
                self.assertEqual(1, macro.count(f"Pop {register}"))

    def test_windows_contract_checks_run_before_installer_publication(self):
        """A passing native shortcut fixture is required before release artifacts can publish."""
        windows = WORKFLOW.read_text().split("  windows:\n", 1)[1].split("  linux-cli:\n", 1)[0]
        check = "Verify Windows installer shortcut contracts before publication"
        self.assertIn("-p 'test_windows_installer.py'", windows)
        self.assertLess(windows.index(check), windows.index("Upload to the draft"))

    def test_every_referenced_native_icon_ships_in_the_desktop_bundle(self):
        """A regenerated mark must reach each configured native icon format."""
        config = json.loads(CONFIG.read_text())
        for reference in config["bundle"]["icon"]:
            path = CONFIG.parent / reference
            self.assertTrue(path.is_file(), reference)
            self.assertGreater(path.stat().st_size, 0)
