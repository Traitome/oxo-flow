"""Contract checks for the frontier intent fixture (`intents.json`).

The intent set is the fixture the frontier benchmark scores against; the
README states its size, domain coverage, and tier split so readers can size
a campaign without opening the JSON. Those numbers are contract statements,
not commentary: they must be derived from the fixture, never hand-copied
(the audit's "reuse, never retype" invariant). This test fails when the
README drifts from `intents.json` (observed: the README said 29 intents /
10 domains while the fixture carried 36 / 12 — issue #1168).
"""

import json
import unittest
from collections import Counter
from pathlib import Path

import run_eval

HERE = Path(__file__).resolve().parent
INTENTS = HERE / "intents.json"
README = HERE / "README.md"
TIERS = {"easy", "medium", "hard"}


def load_intents():
    return json.loads(INTENTS.read_text())["intents"]


class IntentsContractTests(unittest.TestCase):
    def test_every_intent_has_the_required_shape(self):
        intents = load_intents()
        ids = [row["id"] for row in intents]
        self.assertEqual(len(ids), len(set(ids)), "intent ids must be unique")
        for row in intents:
            for key in ("id", "tier", "domain", "intent"):
                self.assertIn(key, row, f"{row.get('id')}: missing {key}")
                self.assertTrue(row[key], f"{row.get('id')}: empty {key}")
            self.assertIn(row["tier"], TIERS, row["id"])
            self.assertIsInstance(row.get("requires"), list, row["id"])
            # The vague intents deliberately carry no required tool list;
            # every specified intent must name at least one.
            if not row.get("vague"):
                self.assertTrue(row["requires"], f"{row['id']}: empty requires")

    def test_readme_counts_are_derived_from_the_fixture(self):
        intents = load_intents()
        domains = {row["domain"] for row in intents}
        tiers = Counter(row["tier"] for row in intents)
        vague = sum(1 for row in intents if row.get("vague"))
        readme = README.read_text()

        variants = ",".join(run_eval.ARMS)
        self.assertIn(f"all {len(intents)} intents x {variants}", readme)
        self.assertIn(f"spans {len(domains)} domains", readme)
        self.assertIn(f"Intent set is {len(intents)}", readme)
        self.assertIn(f"{vague} of them deliberately vague", readme)
        self.assertIn(
            f"{tiers['easy']} easy / {tiers['medium']} medium / {tiers['hard']} hard",
            readme,
        )


if __name__ == "__main__":
    unittest.main()
