import unittest

from compare_oha import median_row, validate


class OhaTests(unittest.TestCase):
    def test_medians(self):
        rows = [{"summary": {"requestsPerSec": n},
                 "latencyPercentiles": {"p50": n / 1000, "p99": n / 100}}
                for n in (1, 9, 2)]
        row = median_row("GET /", rows)
        self.assertEqual(row["requests_per_second"], 2)
        self.assertEqual(row["latency_ms"], {"p50": 2, "p99": 20})

    def test_success(self):
        validate({"statusCodeDistribution": {"200": 10}, "errorDistribution": {}})

    def test_empty_or_failed(self):
        for statuses, errors in (({}, {}), ({"500": 1}, {}), ({"200": 1}, {"timeout": 1})):
            with self.subTest(statuses=statuses, errors=errors), self.assertRaises(RuntimeError):
                validate({"statusCodeDistribution": statuses, "errorDistribution": errors})


if __name__ == "__main__":
    unittest.main()
