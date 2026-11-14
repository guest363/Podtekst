import unittest

from check import _compare


def row(language: str, der: float, speakers: int = 2, attribution: float = 1.0) -> dict:
    return {
        "language": language,
        "speakers_ref": str(speakers),
        "speakers_hyp": str(speakers),
        "WER": "0.1",
        "CER": "0.05",
        "DER": str(der),
        "SCA": "1.0",
        "cpWER": "0.1",
        "attribution": str(attribution),
    }


class CheckTest(unittest.TestCase):
    def test_equal_is_clean(self):
        errors, changes = _compare({"a": row("ru", 0.3)}, {"a": row("ru", 0.3)})
        self.assertEqual((errors, changes), ([], []))

    def test_russian_regression_blocks_and_improvement_does_not(self):
        errors, changes = _compare({"a": row("ru", 0.3)}, {"a": row("ru", 0.31)})
        self.assertEqual(len(errors), 1)
        self.assertEqual(len(changes), 1)
        errors, changes = _compare({"a": row("ru", 0.3)}, {"a": row("ru", 0.29)})
        self.assertEqual(errors, [])
        self.assertEqual(len(changes), 1)

    def test_english_regression_is_reported_but_not_blocking(self):
        errors, changes = _compare({"a": row("en", 0.3)}, {"a": row("en", 0.31)})
        self.assertEqual(errors, [])
        self.assertEqual(len(changes), 1)

    def test_speaker_count_change_blocks_russian(self):
        actual = row("ru", 0.3, 2)
        actual["speakers_hyp"] = "1"
        errors, changes = _compare({"a": row("ru", 0.3, 2)}, {"a": actual})
        self.assertEqual(len(errors), 1)
        self.assertEqual(len(changes), 1)

    def test_missing_and_extra_files_are_errors(self):
        errors, changes = _compare({"a": row("ru", 0.3)}, {})
        self.assertEqual(len(errors), 1)
        self.assertEqual(changes, [])
        errors, changes = _compare({}, {"a": row("ru", 0.3)})
        self.assertEqual(len(errors), 1)
        self.assertEqual(changes, [])
        errors, changes = _compare({}, {"a": row("ru", 0.3)}, allow_extra=True)
        self.assertEqual((errors, changes), ([], []))


if __name__ == "__main__":
    unittest.main()
