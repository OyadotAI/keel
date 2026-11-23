# Compile and execute the shipping NSIS shortcut macros on Windows with scratch-only target records.
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[2]
HOOK = ROOT / "desktop/src-tauri/factory-installer.nsh"
HARNESS = Path(__file__).with_name("windows-shortcuts.nsi")
COMPILER_TIMEOUT = 60
FIXTURE_TIMEOUT = 15
RENAME_CASES = [
    ("rename", "owned", None, False, None, "owned"),
    ("owned_collision", "owned", "owned", False, None, "owned"),
    ("unrelated_destination", "owned", "other", False, "owned", "other"),
    ("unrelated_source", "other", None, False, "other", None),
    ("failed_rename", "owned", None, True, "owned", None),
]


def write_cases(state, script):
    """Generate native operations over scratch records while retaining a register/stack sentinel."""
    commands = ['Section', 'StrCpy $FactoryExecutable "owned"', 'Push "stack sentinel"']
    commands.extend(f'StrCpy ${index} "saved{index}"' for index in range(4))
    for name, source, destination, missing_parent, _, _ in RENAME_CASES:
        (state / f"{name}.legacy").write_text(source)
        target = f"missing/{name}.factory" if missing_parent else f"{name}.factory"
        if destination is not None:
            (state / target).write_text(destination)
        native_target = target.replace("/", "\\")
        operation = f'!insertmacro FactoryRenameOwnedShortcut "$INSTDIR\\{name}.legacy" "$INSTDIR\\{native_target}"'
        commands.extend([operation, operation])  # Same installer run again must be idempotent.
    for owner in ("owned", "other"):
        (state / f"remove-{owner}").write_text(owner)
        commands.append(f'!insertmacro FactoryRemoveOwnedShortcut "$INSTDIR\\remove-{owner}"')
    commands.extend(['Pop $4', 'StrCmp $4 "stack sentinel" +3 0', 'SetErrorLevel 1', 'Abort'])
    for index in range(4):
        commands.extend([f'StrCmp ${index} "saved{index}" +3 0', 'SetErrorLevel 1', 'Abort'])
    commands.extend(['SetErrorLevel 0', 'SectionEnd'])
    script.write_text("\n".join(commands))


@unittest.skipUnless(sys.platform == "win32", "Native NSIS macro execution requires Windows")
class WindowsShortcutTests(unittest.TestCase):
    """Exercise the actual NSIS control flow, with the platform matcher faked at its seam."""

    def test_owned_links_are_idempotent_and_unrelated_or_failed_renames_are_preserved(self):
        """Cover duplicate installs, ownership collisions, missing paths and register restoration."""
        compiler = shutil.which("makensis")
        if not compiler:
            compiler = str(Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "NSIS/makensis.exe")
        self.assertTrue(Path(compiler).is_file(), "Install NSIS before running the Windows fixture")
        with TemporaryDirectory(prefix="oya factory shortcuts ") as temporary:
            fixture = Path(temporary)
            state = fixture / "state"
            state.mkdir()
            script = fixture / "cases.nsh"
            write_cases(state, script)
            subprocess.run([compiler, "/V2", f"/DFIXTURE_ROOT={fixture}", f"/DFACTORY_HOOK={HOOK}", f"/DTEST_CASES={script}", str(HARNESS)], check=True, timeout=COMPILER_TIMEOUT)
            subprocess.run([str(fixture / "shortcut-fixture.exe"), "/S"], check=True, timeout=FIXTURE_TIMEOUT)
            trace = fixture / "FactoryHarnessTrace.log"
            diagnostic = trace.read_text(encoding="utf-8", errors="replace") if trace.exists() else "Matcher trace was not written."
            for name, _, _, missing_parent, source, destination in RENAME_CASES:
                target = state / (f"missing/{name}.factory" if missing_parent else f"{name}.factory")
                for path, expected in [(state / f"{name}.legacy", source), (target, destination)]:
                    with self.subTest(case=name, path=path.name):
                        self.assertEqual(expected, path.read_text() if path.exists() else None, diagnostic)
            self.assertFalse((state / "remove-owned").exists(), diagnostic)
            self.assertEqual("other", (state / "remove-other").read_text())
