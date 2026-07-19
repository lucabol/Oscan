from pathlib import Path
import re
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
SMOKE_SCRIPT = REPO_ROOT / "scripts" / "smoke-release.ps1"
STANDARD_USER_HELPER = REPO_ROOT / "scripts" / "windows-standard-user.ps1"


class ReleaseSmokeTests(unittest.TestCase):
    def test_initial_package_compile_explicitly_uses_c_backend(self) -> None:
        script = SMOKE_SCRIPT.read_text(encoding="utf-8")
        initial_compile = re.search(
            r"Push-Location \$ScratchDir(?P<body>.*?)"
            r"\$CompileText = Get-Content \$CompileLog",
            script,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(initial_compile)
        body = initial_compile.group("body")
        self.assertRegex(
            body,
            r'\$compileArgs\s*=\s*@\(\s*"--backend"\s*,\s*"c"\s*\)',
        )
        self.assertIn('$compileArgs += "--libc"', body)
        self.assertRegex(body, r"& \$OscanCommand @compileArgs")

    def test_standard_user_launch_loads_new_account_profile(self) -> None:
        helper = STANDARD_USER_HELPER.read_text(encoding="utf-8")
        launch = re.search(
            r"\$process\s*=\s*Start-Process(?P<body>.*?)-PassThru",
            helper,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(launch)
        self.assertRegex(
            launch.group("body"),
            r"-Credential\s+\$credential\s+`\s*"
            r"-LoadUserProfile\s+`\s*"
            r"-WorkingDirectory\s+\$working",
        )


if __name__ == "__main__":
    unittest.main()
