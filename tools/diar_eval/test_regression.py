import unittest

from regression import check_metrics


class RegressionTest(unittest.TestCase):
    def test_gate_rejects_bad_metrics_and_missing_rows(self):
        scenarios = [("en", 2, 0, 0), ("ru", 3, 5, 0)]
        rows = {
            "synth-en-2spk-60s-ov0": {"exit_code": "0", "error": "", "DER": "0.15", "speakers_hyp": "2"},
            "synth-ru-3spk-60s-ov5": {"exit_code": "0", "error": "", "DER": "0.405047", "speakers_hyp": "2"},
        }
        baseline = {"synth-ru-3spk-60s-ov5": {"DER": 0.405047, "speakers_hyp": 2}}
        self.assertEqual(check_metrics(rows, scenarios, baseline), [])
        rows["synth-en-2spk-60s-ov0"]["DER"] = "0.476534"
        self.assertEqual(len(check_metrics(rows, scenarios, baseline)), 1)
        rows["synth-en-2spk-60s-ov0"]["DER"] = "NaN"
        rows["synth-ru-3spk-60s-ov5"]["speakers_hyp"] = "1"
        self.assertEqual(len(check_metrics(rows, scenarios, baseline)), 2)
        rows.pop("synth-en-2spk-60s-ov0")
        self.assertEqual(len(check_metrics(rows, scenarios, baseline)), 2)


if __name__ == "__main__":
    unittest.main()
