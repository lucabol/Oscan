from pathlib import Path
import re
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
RELEASE_POWERSHELL_SCRIPTS = (
    REPO_ROOT / "scripts" / "assemble-release.ps1",
    REPO_ROOT / "scripts" / "build-runtime-archive.ps1",
    REPO_ROOT / "scripts" / "smoke-release.ps1",
    REPO_ROOT / "scripts" / "stage-release.ps1",
)
WINDOWS_STYLE_RELEASE_PATH = re.compile(
    r"(?:\./)?(?:scripts|packaging|build|runtime-archives|runtime-toolchains)"
    r"\\[^\s\"']*|\$sidecarBase\\[^\s\"']*"
)


class ReleaseWorkflowPathTests(unittest.TestCase):
    def test_workflow_paths_use_cross_platform_separators(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        windows_style_paths = WINDOWS_STYLE_RELEASE_PATH.findall(workflow)
        self.assertEqual([], windows_style_paths)

    def test_cross_platform_release_scripts_use_portable_paths(self) -> None:
        for script_path in RELEASE_POWERSHELL_SCRIPTS:
            with self.subTest(script=script_path.name):
                script = script_path.read_text(encoding="utf-8")
                windows_style_paths = WINDOWS_STYLE_RELEASE_PATH.findall(script)
                self.assertEqual([], windows_style_paths)


if __name__ == "__main__":
    unittest.main()
