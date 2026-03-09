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


class TestTimingModel(unittest.TestCase):
    def test_functional(self):
        from helm.timing import TimingModel
        m = TimingModel.fe()
        self.assertEqual(m.level, "FE")

    def test_ite_with_params(self):
        from helm.timing import TimingModel
        m = TimingModel.ite(l1_latency=3, dram_latency=200)
        self.assertEqual(m.params["dram_latency"], 200)

    def test_all_levels(self):
        from helm.timing import TimingModel
        levels = [TimingModel.fe(), TimingModel.ite(), TimingModel.cae()]
        names = [m.level for m in levels]
        self.assertEqual(names, ["FE", "ITE", "CAE"])


class TestPlatformDevices(unittest.TestCase):
    def test_platform_with_devices(self):
        from helm.device import Device
        from helm.timing import TimingModel
        p = Platform(
            "test",
            isa=RiscV(),
            cores=[Core("c0")],
            memory=MemorySystem(),
            devices=[Device("uart", region_size=8, base_address=0x1000)],
            timing=TimingModel.cae(),
        )
        d = p.to_dict()
        self.assertEqual(len(d["devices"]), 1)
        self.assertEqual(d["timing"]["level"], "CAE")

    def test_add_device_chaining(self):
        from helm.device import Device
        p = Platform("test", isa=RiscV(), cores=[Core("c0")], memory=MemorySystem())
        p.add_device(Device("a")).add_device(Device("b"))
        self.assertEqual(len(p.devices), 2)


class TestPluginInterface(unittest.TestCase):
    def test_add_plugin_returns_self(self):
        p = Platform("t", isa=RiscV(), cores=[Core("c")], memory=MemorySystem())
        sim = Simulation(p, binary="/dev/null")
        ret = sim.add_plugin(self._dummy_plugin())
        self.assertIs(ret, sim)

    def test_plugin_lookup_by_name(self):
        from helm.plugins import InsnCount
        p = Platform("t", isa=RiscV(), cores=[Core("c")], memory=MemorySystem())
        sim = Simulation(p, binary="/dev/null")
        sim.add_plugin(InsnCount())
        self.assertIsNotNone(sim.plugin("insn-count"))
        self.assertIsNone(sim.plugin("nonexistent"))

    def test_multiple_plugins(self):
        from helm.plugins import InsnCount, SyscallTrace, CacheSim
        p = Platform("t", isa=RiscV(), cores=[Core("c")], memory=MemorySystem())
        sim = Simulation(p, binary="/dev/null")
        sim.add_plugin(InsnCount()).add_plugin(SyscallTrace()).add_plugin(CacheSim())
        self.assertEqual(len(sim.plugins), 3)

    def test_plugins_in_stub_output(self):
        from helm.plugins import InsnCount
        p = Platform("t", isa=RiscV(), cores=[Core("c")], memory=MemorySystem())
        sim = Simulation(p, binary="/dev/null")
        sim.add_plugin(InsnCount())
        results = sim.run()
        self.assertIn("plugins", results.raw)

    def test_insn_count_plugin(self):
        from helm.plugins import InsnCount
        ic = InsnCount()
        ic.on_insn(0, 0x1000, "ADD_imm")
        ic.on_insn(0, 0x1004, "SUB_imm")
        ic.on_insn(1, 0x2000, "B")
        self.assertEqual(ic.total, 3)
        self.assertEqual(ic.per_vcpu[0], 2)
        self.assertEqual(ic.per_vcpu[1], 1)

    def test_cache_sim_plugin(self):
        from helm.plugins import CacheSim
        cs = CacheSim(l1d_size="32KB")
        cs.on_mem(0, 0x1000, 8, False)
        cs.on_mem(0, 0x2000, 4, True)
        self.assertEqual(cs._l1d_hits, 2)
        self.assertAlmostEqual(cs.l1d_hit_rate, 1.0)

    def test_howvec_classification(self):
        from helm.plugins import HowVec
        hv = HowVec()
        hv.on_insn(0, 0, "ADD_imm")
        hv.on_insn(0, 0, "B")
        hv.on_insn(0, 0, "LDR_x_uimm")
        hv.on_insn(0, 0, "SVC")
        hv.atexit()
        self.assertEqual(hv.results["IntAlu"], 1)
        self.assertEqual(hv.results["Branch"], 1)
        self.assertEqual(hv.results["Load"], 1)
        self.assertEqual(hv.results["Syscall"], 1)

    def test_execlog_respects_max(self):
        from helm.plugins import ExecLog
        el = ExecLog(max_lines=2)
        el.on_insn(0, 0x1000, "A")
        el.on_insn(0, 0x1004, "B")
        el.on_insn(0, 0x1008, "C")
        self.assertEqual(len(el.lines), 2)

    def test_syscall_trace(self):
        from helm.plugins import SyscallTrace
        st = SyscallTrace()
        st.on_syscall(0, 64, [1, 0x1000, 6, 0, 0, 0])
        st.on_syscall_ret(0, 64, 6)
        self.assertEqual(len(st.entries), 2)
        self.assertEqual(st.entries[0]["type"], "entry")
        self.assertEqual(st.entries[1]["type"], "return")

    def test_plugin_base_to_dict(self):
        from helm.plugins.base import PluginBase
        p = PluginBase("test", foo=42, bar="baz")
        d = p.to_dict()
        self.assertEqual(d["name"], "test")
        self.assertEqual(d["args"]["foo"], 42)

    @staticmethod
    def _dummy_plugin():
        from helm.plugins.base import PluginBase
        return PluginBase("dummy")
