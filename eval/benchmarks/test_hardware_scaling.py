import json
import tempfile
import unittest
from pathlib import Path

from hardware_scaling import (
    generate_markdown_report,
    load_measured_benchmark,
    model_hardware_scaling_projection,
)


def measured_run(run_id, config, seed, wall, throughput):
    return {
        "run_id": run_id,
        "config_name": config,
        "cohort_size": 10,
        "seed": seed,
        "wall_time_hours": wall,
        "samples_per_hour": throughput,
        "l3_cache_miss_rate_pct": 5.0,
        "remote_numa_access_pct": 4.0,
        "interconnect_bandwidth_gbps": 20.0,
        "io_wait_time_pct": 8.0,
        "peak_memory_gb": 100.0,
        "energy_kwh": 2.0,
    }


class HardwareScalingTests(unittest.TestCase):
    def test_projection_report_is_not_presented_as_experiment(self):
        results = model_hardware_scaling_projection(10)
        markdown = generate_markdown_report(
            results,
            {"os": "test", "cores": 2},
            {"description": "model target"},
            "model_projection",
        )
        self.assertIn("MODEL PROJECTION", markdown)
        self.assertIn("Not Measurement", markdown)
        self.assertNotIn("Experimental Hardware Topology", markdown)
        self.assertNotIn("Nature", markdown)

    def test_measured_loader_aggregates_runs_and_cis(self):
        payload = {
            "host_inventory": {"os": "Linux", "cores": 128},
            "target_topology": {"sockets": 2},
            "runs": [
                measured_run("r1", "baseline", 1, 10.0, 1.0),
                measured_run("r2", "baseline", 2, 12.0, 0.8),
                measured_run("r3", "tuned", 1, 6.0, 1.7),
                measured_run("r4", "tuned", 2, 7.0, 1.4),
            ],
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "measurements.json"
            path.write_text(json.dumps(payload))
            results, host, target = load_measured_benchmark(path)
        self.assertEqual(host["cores"], 128)
        self.assertEqual(target["sockets"], 2)
        self.assertEqual(len(results), 2)
        self.assertTrue(all(result.n_runs == 2 for result in results))
        self.assertTrue(all(result.wall_time_ci_hours for result in results))

    def test_measured_loader_rejects_duplicate_run_ids(self):
        payload = {
            "host_inventory": {},
            "target_topology": {},
            "runs": [
                measured_run("same", "baseline", 1, 10.0, 1.0),
                measured_run("same", "baseline", 2, 11.0, 0.9),
            ],
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "measurements.json"
            path.write_text(json.dumps(payload))
            with self.assertRaisesRegex(ValueError, "duplicate measured run_id"):
                load_measured_benchmark(path)

    def test_measured_loader_requires_replication(self):
        payload = {
            "host_inventory": {},
            "target_topology": {},
            "runs": [measured_run("only", "baseline", 1, 10.0, 1.0)],
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "measurements.json"
            path.write_text(json.dumps(payload))
            with self.assertRaisesRegex(ValueError, "at least two measured runs"):
                load_measured_benchmark(path)


if __name__ == "__main__":
    unittest.main()
