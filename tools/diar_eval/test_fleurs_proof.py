import json
import unittest
import tempfile
from pathlib import Path

from prepare_fleurs_main import choose

from prepare_fleurs_speakers import validate_selection


class SelectionTest(unittest.TestCase):
    def test_committed_proof_matches_selection_rules(self):
        proof = json.loads(Path(__file__).with_name("proof-fleurs-regression.json").read_text(encoding="utf-8"))
        self.assertEqual(proof["schema"], 1)
        for language in ("ru_ru", "en_us"):
            records = proof["languages"][language]["selected"]
            self.assertEqual(len(records), 4)
            validate_selection(records)
    def test_main_uses_different_sentences_and_opposite_genders(self):
        with tempfile.TemporaryDirectory() as tmp:
            tsv = Path(tmp) / "test.tsv"
            tsv.write_text("1\ta.wav\ttext\tx\tx\tx\tMALE\n"
                           "1\tb.wav\ttext\tx\tx\tx\tFEMALE\n"
                           "2\tc.wav\tbeta\tx\tx\tx\tFEMALE\n", encoding="utf-8")
            first, second = choose(tsv)
            self.assertEqual((first[1], second[1]), ("a.wav", "c.wav"))

    def test_pairwise_proof_and_collision(self):
        records = [
            {"source_id": "a", "sentence_id": "1", "gender": "MALE"},
            {"source_id": "b", "sentence_id": "1", "gender": "MALE"},
            {"source_id": "c", "sentence_id": "2", "gender": "FEMALE"},
            {"source_id": "d", "sentence_id": "2", "gender": "FEMALE"},
        ]
        validate_selection(records)
        with self.assertRaises(ValueError):
            validate_selection([*records[:3], {**records[3], "source_id": "a"}])
        with self.assertRaises(ValueError):
            validate_selection([*records[:3], {**records[3], "sentence_id": "3"}])


if __name__ == "__main__":
    unittest.main()
