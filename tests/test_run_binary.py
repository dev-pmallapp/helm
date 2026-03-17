import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "examples" / "se" / "run_binary.py"


def _write_fake_helm_module(tmp_path: Path, run_result: str) -> None:
    module = tmp_path / "_helm_ng.py"
    module.write_text(
        "\n".join(
            [
                "import os",
                "",
                "class FakeSim:",
                "    def __init__(self):",
                "        self.has_exited = False",
                "        self.exit_code = 0",
                "        self.insn_count = 0",
                "        self.pc = 0x1234",
                "        self.has_unimplemented_instructions = os.environ.get('HELM_TEST_HAS_UNIMPL', '0') == '1'",
                "        self.unimplemented_instruction_count = int(os.environ.get('HELM_TEST_UNIMPL_COUNT', '0'))",
                "",
                "    def load_elf(self, *args, **kwargs):",
                "        pass",
                "",
                "    def add_plugin(self, *args, **kwargs):",
                "        pass",
                "",
                "    def run(self, max_insns):",
                "        self.insn_count += max_insns",
                "        return os.environ['HELM_TEST_RUN_RESULT']",
                "",
                "    def finish(self):",
                "        pass",
                "",
                "def build_simulation(**kwargs):",
                "    return FakeSim()",
            ]
        ),
        encoding="utf-8",
    )


def test_run_binary_exits_nonzero_on_exception(tmp_path: Path) -> None:
    _write_fake_helm_module(tmp_path, "exception:boom")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "exception:boom"

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--binary", str(binary), "--max-insns", "1"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "exception:boom" in output
    assert "hit limit" not in output


def test_run_binary_exits_nonzero_on_unsupported(tmp_path: Path) -> None:
    _write_fake_helm_module(tmp_path, "unsupported")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "unsupported"

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--binary", str(binary), "--max-insns", "1"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "unsupported" in output
    assert "hit limit" not in output


def test_run_binary_reports_unimplemented_instructions_to_user(tmp_path: Path) -> None:
    _write_fake_helm_module(tmp_path, "exception:boom")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "exception:boom"
    env["HELM_TEST_HAS_UNIMPL"] = "1"
    env["HELM_TEST_UNIMPL_COUNT"] = "3"

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--binary", str(binary), "--max-insns", "1"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "unimplemented instructions" in output
    assert "3 unique" in output
