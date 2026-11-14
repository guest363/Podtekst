import unittest

from report import aggregate, compare_baseline


class ReportVersionsTest(unittest.TestCase):
    def test_versions_cannot_be_mixed(self):
        rows = [{"cpWER": "0.1"}, {"cpWER": "0.2", "cpwer_version": "speaker-stream-v2"}]
        with self.assertRaisesRegex(ValueError, "версии cpWER"):
            aggregate(rows)

    def test_new_cpwer_is_not_compared_to_an_unversioned_baseline(self):
        current = {"ru": {"cpWER": 0.5, "cpwer_version": "speaker-stream-v2"}}
        baseline = {"ru": {"cpWER": 0.1}}
        comparison = compare_baseline(current, baseline)[0]
        self.assertEqual(comparison["status"], "разные версии cpWER")
        self.assertEqual(comparison["delta"], "—")
        baseline["ru"]["cpwer_version"] = "speaker-stream-v2"
        self.assertEqual(compare_baseline(current, baseline)[0]["status"], "регресс")


if __name__ == "__main__":
    unittest.main()
