import json
import unittest
from pathlib import Path

SUITES = Path(__file__).with_name("suites")


class SuiteTest(unittest.TestCase):
    def test_small_full_and_perf_are_defined(self):
        for name in ("small", "full", "perf"):
            suite = json.loads((SUITES / f"{name}.json").read_text(encoding="utf-8"))
            self.assertEqual(suite["name"], name)
            self.assertEqual(len({entry["name"] for entry in suite["manifests"]}), len(suite["manifests"]))
            self.assertTrue(all(entry.get("path") for entry in suite["manifests"]))
            self.assertTrue(all(entry.get("ids") for entry in suite["manifests"] if name == "perf"))
        small = json.loads((SUITES / "small.json").read_text(encoding="utf-8"))
        full = json.loads((SUITES / "full.json").read_text(encoding="utf-8"))
        self.assertLess(len(small["manifests"]), len(full["manifests"]))
        self.assertIn("asr-ru", {entry["name"] for entry in full["manifests"]})


if __name__ == "__main__":
    unittest.main()
