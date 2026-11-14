import unittest

from metrics import _SelfTest


def load_tests(loader, tests, pattern):
    return loader.loadTestsFromTestCase(_SelfTest)


if __name__ == "__main__":
    unittest.main()
