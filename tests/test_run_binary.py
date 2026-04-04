import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "examples" / "se" / "run_binary.py"


def _copy_script(tmp_path: Path) -> Path:
    script = tmp_path / "examples" / "se" / "run_binary.py"
    script.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(SCRIPT, script)
    return script


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
                "    def stats(self):",
                "        return {",
                "            'insn_count': self.insn_count,",
                "            'tick_count': self.insn_count,",
                "            'sim_freq': 1000000000,",
                "            'ipc': 1.0,",
                "        }",
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


def _run_script(script: Path, env: dict[str, str], *args: str) -> subprocess.CompletedProcess[str]:
    launcher = script.parents[2] / "target" / "debug" / "helm-aarch64"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.write_text("", encoding="utf-8")
    wrapper = (
        "import runpy, sys; "
        "sys._helm_launcher = 'helm-aarch64'; "
        "sys.executable = sys.argv[1]; "
        "script = sys.argv[2]; "
        "sys.argv = sys.argv[2:]; "
        "runpy.run_path(script, run_name='__main__')"
    )
    return subprocess.run(
        [sys.executable, "-c", wrapper, str(launcher), str(script), *args],
        cwd=script.parents[2],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def test_run_binary_exits_nonzero_on_exception(tmp_path: Path) -> None:
    script = _copy_script(tmp_path)
    _write_fake_helm_module(tmp_path, "exception:boom")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "exception:boom"

    result = _run_script(script, env, "--binary", str(binary), "--max-insns", "1")

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "exception:boom" in output
    assert "hit limit" not in output


def test_run_binary_exits_nonzero_on_unsupported(tmp_path: Path) -> None:
    script = _copy_script(tmp_path)
    _write_fake_helm_module(tmp_path, "unsupported")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "unsupported"

    result = _run_script(script, env, "--binary", str(binary), "--max-insns", "1")

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "unsupported" in output
    assert "hit limit" not in output


def test_run_binary_reports_unimplemented_instructions_to_user(tmp_path: Path) -> None:
    script = _copy_script(tmp_path)
    _write_fake_helm_module(tmp_path, "exception:boom")
    binary = tmp_path / "guest.bin"
    binary.write_bytes(b"\x7fELF")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path)
    env["HELM_TEST_RUN_RESULT"] = "exception:boom"
    env["HELM_TEST_HAS_UNIMPL"] = "1"
    env["HELM_TEST_UNIMPL_COUNT"] = "3"

    result = _run_script(script, env, "--binary", str(binary), "--max-insns", "1")

    output = result.stdout + result.stderr
    assert result.returncode != 0
    assert "unimplemented instructions" in output
    assert "3 unique" in output
