"""Tests for the Python configuration layer."""

import json
import unittest

from helm import Platform, Core, Cache, MemorySystem, Simulation
from helm.predictor import BranchPredictor
from helm.isa import RiscV, X86, Arm
from helm.components.platforms import SingleCoreRiscV, QuadCoreX86


class TestCore(unittest.TestCase):
    def test_default_values(self):
        c = Core("c0")
        self.assertEqual(c.width, 4)
        self.assertEqual(c.rob_size, 128)
        self.assertEqual(c.iq_size, 64)

    def test_custom_values(self):
        c = Core("c0", width=8, rob_size=256)
        self.assertEqual(c.width, 8)
        self.assertEqual(c.rob_size, 256)

    def test_to_dict_has_required_keys(self):
        d = Core("c0").to_dict()
        self.assertIn("name", d)
        self.assertIn("width", d)
        self.assertIn("rob_size", d)
        self.assertIn("branch_predictor", d)


class TestCache(unittest.TestCase):
    def test_default_values(self):
        c = Cache()
        self.assertEqual(c.size, "32KB")
        self.assertEqual(c.assoc, 8)

    def test_to_dict(self):
        d = Cache("64KB", assoc=4, latency=2).to_dict()
        self.assertEqual(d["size"], "64KB")
        self.assertEqual(d["associativity"], 4)
        self.assertEqual(d["latency_cycles"], 2)


class TestMemorySystem(unittest.TestCase):
    def test_no_caches(self):
        m = MemorySystem(dram_latency=200)
        d = m.to_dict()
        self.assertIsNone(d["l1i"])
        self.assertEqual(d["dram_latency_cycles"], 200)

    def test_with_caches(self):
        m = MemorySystem(l1i=Cache("32KB"), l2=Cache("256KB"))
        d = m.to_dict()
        self.assertIsNotNone(d["l1i"])
        self.assertIsNotNone(d["l2"])
        self.assertIsNone(d["l1d"])


class TestBranchPredictor(unittest.TestCase):
    def test_static(self):
        bp = BranchPredictor.static()
        self.assertEqual(bp.kind, "Static")

    def test_tage(self):
        bp = BranchPredictor.tage(history_length=128)
        self.assertEqual(bp.params["history_length"], 128)

    def test_to_dict_simple(self):
        d = BranchPredictor.static().to_dict()
        self.assertEqual(d, "Static")

    def test_to_dict_with_params(self):
        d = BranchPredictor.bimodal(4096).to_dict()
        self.assertEqual(d, {"Bimodal": {"table_size": 4096}})


class TestPlatform(unittest.TestCase):
    def test_construction(self):
        p = Platform(
            "test",
            isa=RiscV(),
            cores=[Core("c0")],
            memory=MemorySystem(),
        )
        self.assertEqual(p.name, "test")
        self.assertEqual(len(p.cores), 1)

    def test_to_dict_is_valid_json(self):
        p = SingleCoreRiscV()
        d = p.to_dict()
        j = json.dumps(d)
        self.assertIn("single-rv64", j)

    def test_isa_kinds(self):
        self.assertEqual(RiscV().kind, "RiscV64")
        self.assertEqual(X86().kind, "X86_64")
        self.assertEqual(Arm().kind, "Arm64")


class TestSimulation(unittest.TestCase):
    def test_stub_returns_results(self):
        p = SingleCoreRiscV()
        sim = Simulation(p, binary="/dev/null", mode="se")
        results = sim.run()
        self.assertEqual(results.cycles, 0)
        self.assertEqual(results.ipc, 0.0)

    def test_prebuilt_platform_simulates(self):
        p = QuadCoreX86()
        sim = Simulation(p, binary="/dev/null", mode="microarch")
        results = sim.run()
        self.assertTrue(hasattr(results, "ipc"))
        self.assertTrue(hasattr(results, "branch_mpki"))


if __name__ == "__main__":
    unittest.main()


class TestDevice(unittest.TestCase):
    def test_base_device(self):
        from helm.device import Device
        d = Device("test-dev", region_size=0x100, base_address=0x4000)
        self.assertEqual(d.name, "test-dev")
        self.assertEqual(d.region_size, 0x100)
        self.assertEqual(d.read(0, 4), 0)

    def test_custom_device_subclass(self):
        from helm.device import Device

        class Counter(Device):
            def __init__(self):
                super().__init__("counter", region_size=8)
                self.value = 0

            def read(self, offset, size):
                return self.value

            def write(self, offset, size, value):
                self.value = value

        c = Counter()
        c.write(0, 4, 42)
        self.assertEqual(c.read(0, 4), 42)

    def test_device_to_dict(self):
        from helm.device import Device
        d = Device("uart", region_size=0x10, base_address=0x1000, irq=3)
        data = d.to_dict()
        self.assertEqual(data["name"], "uart")
        self.assertEqual(data["irq"], 3)


class TestTimingMode(unittest.TestCase):
    def test_functional(self):
        from helm.timing import TimingMode
        m = TimingMode.functional()
        self.assertEqual(m.level, "Functional")

    def test_stall_annotated_with_params(self):
        from helm.timing import TimingMode
        m = TimingMode.stall_annotated(l1_latency=3, dram_latency=200)
        self.assertEqual(m.params["dram_latency"], 200)

    def test_all_levels(self):
        from helm.timing import TimingMode
        for factory in [TimingMode.functional, TimingMode.microarchitectural,
                        TimingMode.cycle_accurate]:
            m = factory()
            self.assertIn(m.level, ["Functional", "Microarchitectural", "CycleAccurate"])


class TestPlatformDevices(unittest.TestCase):
    def test_platform_with_devices(self):
        from helm.device import Device
        from helm.timing import TimingMode
        p = Platform(
            "test",
            isa=RiscV(),
            cores=[Core("c0")],
            memory=MemorySystem(),
            devices=[Device("uart", region_size=8, base_address=0x1000)],
            timing=TimingMode.microarchitectural(),
        )
        d = p.to_dict()
        self.assertEqual(len(d["devices"]), 1)
        self.assertEqual(d["timing"]["level"], "Microarchitectural")

    def test_add_device_chaining(self):
        from helm.device import Device
        p = Platform("test", isa=RiscV(), cores=[Core("c0")], memory=MemorySystem())
        p.add_device(Device("a")).add_device(Device("b"))
        self.assertEqual(len(p.devices), 2)
