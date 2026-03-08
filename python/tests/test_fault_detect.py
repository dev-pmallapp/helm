"""Tests for the FaultDetect debug plugin."""

import unittest

from helm.plugins.debug import FaultDetect
from helm.plugins.base import PluginBase


class TestFaultDetectConstruction(unittest.TestCase):
    """FaultDetect can be constructed with various trigger options."""

    def test_default_is_active(self):
        fd = FaultDetect()
        self.assertTrue(fd.active)
        self.assertEqual(fd.name, "fault-detect")

    def test_after_insns_starts_inactive(self):
        fd = FaultDetect(after_insns=1000)
        self.assertFalse(fd.active)
        self.assertEqual(fd.args["after_insns"], 1000)

    def test_at_pc_starts_inactive(self):
        fd = FaultDetect(at_pc=0x411120)
        self.assertFalse(fd.active)
        self.assertEqual(fd.args["at_pc"], 0x411120)

    def test_at_symbol_starts_inactive(self):
        fd = FaultDetect(at_symbol="main")
        self.assertFalse(fd.active)
        self.assertEqual(fd.args["at_symbol"], "main")

    def test_manual_activate(self):
        fd = FaultDetect(after_insns=999999)
        self.assertFalse(fd.active)
        fd.activate()
        self.assertTrue(fd.active)


class TestFaultDetectActivation(unittest.TestCase):
    """Deferred activation triggers."""

    def test_activates_after_insns(self):
        fd = FaultDetect(after_insns=3)
        fd.on_insn(0, 0x1000, "nop")
        self.assertFalse(fd.active)
        fd.on_insn(0, 0x1004, "nop")
        self.assertFalse(fd.active)
        fd.on_insn(0, 0x1008, "nop")
        self.assertTrue(fd.active)

    def test_activates_at_pc(self):
        fd = FaultDetect(at_pc=0x2000)
        fd.on_insn(0, 0x1000, "nop")
        self.assertFalse(fd.active)
        fd.on_insn(0, 0x2000, "add")
        self.assertTrue(fd.active)

    def test_stays_active_once_triggered(self):
        fd = FaultDetect(after_insns=1)
        fd.on_insn(0, 0x1000, "nop")
        self.assertTrue(fd.active)
        fd.on_insn(0, 0x1004, "nop")
        self.assertTrue(fd.active)


class TestFaultDetectCallbacks(unittest.TestCase):
    """Callback behaviour."""

    def test_enabled_callbacks_include_fault(self):
        fd = FaultDetect()
        self.assertIn("fault", fd.enabled_callbacks)
        self.assertIn("insn", fd.enabled_callbacks)
        self.assertIn("syscall", fd.enabled_callbacks)

    def test_on_fault_collects_reports_when_active(self):
        fd = FaultDetect()
        fd.on_fault({"kind": "NullJump", "pc": 0, "message": "test"})
        self.assertEqual(len(fd.reports), 1)
        self.assertEqual(fd.reports[0]["kind"], "NullJump")

    def test_on_fault_ignored_when_inactive(self):
        fd = FaultDetect(after_insns=999)
        fd.on_fault({"kind": "NullJump", "pc": 0, "message": "test"})
        self.assertEqual(len(fd.reports), 0)

    def test_is_plugin_base_subclass(self):
        fd = FaultDetect()
        self.assertIsInstance(fd, PluginBase)


class TestFaultDetectSerialization(unittest.TestCase):
    """to_dict produces the right config."""

    def test_to_dict_always_active(self):
        fd = FaultDetect()
        d = fd.to_dict()
        self.assertEqual(d["name"], "fault-detect")
        self.assertNotIn("after_insns", d["args"])

    def test_to_dict_with_after_insns(self):
        fd = FaultDetect(after_insns=5000)
        d = fd.to_dict()
        self.assertEqual(d["args"]["after_insns"], 5000)

    def test_to_dict_with_at_pc(self):
        fd = FaultDetect(at_pc=0x400000)
        d = fd.to_dict()
        self.assertEqual(d["args"]["at_pc"], 0x400000)

    def test_atexit_populates_results(self):
        fd = FaultDetect()
        fd.on_insn(0, 0x1000, "nop")
        fd.atexit()
        self.assertEqual(fd.results["total_insns_seen"], 1)
        self.assertTrue(fd.results["was_active"])


if __name__ == "__main__":
    unittest.main()
