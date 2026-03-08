"""Tests for the SeSession pause/load/continue workflow."""

import unittest

from helm.session import SeSession, StopReason, StopResult
from helm.plugins.debug import FaultDetect
from helm.plugins.trace import InsnCount


class TestSeSessionConstruction(unittest.TestCase):
    def test_creates_with_defaults(self):
        s = SeSession("/dev/null")
        self.assertEqual(s.insn_count, 0)
        self.assertFalse(s.has_exited)
        self.assertEqual(len(s.plugins), 0)

    def test_argv_defaults_to_basename(self):
        s = SeSession("/usr/bin/foo")
        self.assertEqual(s.argv, ["foo"])


class TestSeSessionPluginHotLoad(unittest.TestCase):
    def test_add_plugin_before_run(self):
        s = SeSession("/dev/null")
        fd = FaultDetect()
        s.add_plugin(fd)
        self.assertEqual(len(s.plugins), 1)
        self.assertIs(s.plugins[0], fd)

    def test_add_plugin_between_runs(self):
        s = SeSession("/dev/null")
        s.run(100)
        self.assertEqual(len(s.plugins), 0)

        fd = FaultDetect()
        s.add_plugin(fd)
        self.assertEqual(len(s.plugins), 1)

        s.run(100)
        self.assertEqual(s.insn_count, 200)

    def test_multiple_plugins_loaded_incrementally(self):
        s = SeSession("/dev/null")

        s.run(1000)
        s.add_plugin(InsnCount())

        s.run(2000)
        s.add_plugin(FaultDetect())

        self.assertEqual(len(s.plugins), 2)
        self.assertEqual(s.insn_count, 3000)


class TestSeSessionRunMethods(unittest.TestCase):
    def test_run_returns_insn_limit(self):
        s = SeSession("/dev/null")
        r = s.run(500)
        self.assertEqual(r.reason, StopReason.INSN_LIMIT)

    def test_run_until_insns_accumulates(self):
        s = SeSession("/dev/null")
        s.run(100)
        r = s.run_until_insns(300)
        self.assertEqual(r.reason, StopReason.INSN_LIMIT)
        self.assertEqual(s.insn_count, 300)

    def test_run_until_insns_already_past(self):
        s = SeSession("/dev/null")
        s.run(500)
        r = s.run_until_insns(100)
        self.assertEqual(r.reason, StopReason.INSN_LIMIT)
        self.assertEqual(s.insn_count, 500)

    def test_finish_calls_atexit(self):
        s = SeSession("/dev/null")
        fd = FaultDetect()
        s.add_plugin(fd)
        s.run(10)
        s.finish()
        self.assertIn("was_active", fd.results)


class TestStopResult(unittest.TestCase):
    def test_repr_insn_limit(self):
        r = StopResult(StopReason.INSN_LIMIT)
        self.assertIn("INSN_LIMIT", repr(r))

    def test_repr_breakpoint(self):
        r = StopResult(StopReason.BREAKPOINT, pc=0x1000)
        self.assertIn("0x1000", repr(r))

    def test_repr_exited(self):
        r = StopResult(StopReason.EXITED, exit_code=42)
        self.assertIn("42", repr(r))

    def test_repr_error(self):
        r = StopResult(StopReason.ERROR, message="boom")
        self.assertIn("boom", repr(r))


class TestPauseLoadContinuePattern(unittest.TestCase):
    """Full workflow: warm-up → load plugin → continue."""

    def test_warmup_then_fault_detect(self):
        s = SeSession("/dev/null")

        # Phase 1: warm-up without any plugins
        s.run(1_000_000)
        self.assertEqual(s.insn_count, 1_000_000)
        self.assertEqual(len(s.plugins), 0)

        # Phase 2: hot-load fault-detect
        fd = FaultDetect()
        s.add_plugin(fd)
        self.assertTrue(fd.active)

        # Phase 3: continue
        s.run(500_000)
        self.assertEqual(s.insn_count, 1_500_000)

        # Phase 4: finish
        s.finish()
        self.assertEqual(fd.results["was_active"], True)

    def test_deferred_activate_then_hotload(self):
        s = SeSession("/dev/null")

        # Phase 1: warm-up
        s.run(100)

        # Phase 2: load with deferred activation
        fd = FaultDetect(after_insns=50)
        self.assertFalse(fd.active)
        s.add_plugin(fd)

        # Phase 3: run enough to trigger
        for _ in range(60):
            fd.on_insn(0, 0x1000, "nop")
        self.assertTrue(fd.active)


if __name__ == "__main__":
    unittest.main()
