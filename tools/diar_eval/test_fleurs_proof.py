import unittest

from prepare_fleurs_speakers import validate_selection


class SelectionTest(unittest.TestCase):
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
