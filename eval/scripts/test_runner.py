import csv
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import runner
import ai_judge


class ToolJudgingTests(unittest.TestCase):
    def test_negative_answer_that_suggests_real_tool_fails(self):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow(
                    {
                        "id": "tool-001",
                        "trial": 1,
                        "answer": "not found, maybe you meant fastqc",
                        "error": "",
                    }
                )
            gold_rows = [
                {
                    "id": "tool-001",
                    "expected_tool": "",
                    "expected_version": "",
                    "negative_sample": "1",
                }
            ]
            with mock.patch("common.known_tool_names", return_value={"fastqc"}):
                results = runner.judge_tool(gold_rows, str(captures))
            self.assertEqual(results[0]["no_hallucination"], 0.0)


class SummaryTests(unittest.TestCase):
    def test_per_item_summary_computes_pass_at_k(self):
        rows = [
            {"id": "wf-001", "trial": 1, "overall": 0.5},
            {"id": "wf-001", "trial": 2, "overall": 1.0},
            {"id": "wf-002", "trial": 1, "overall": 0.0},
        ]
        items = runner.per_item_summary(rows)
        self.assertEqual(items[0]["pass_at_k"], 1.0)
        self.assertEqual(items[1]["pass_at_k"], 0.0)

    def test_per_item_summary_computes_pass_at_one_from_first_trial(self):
        rows = [
            {"id": "wf-001", "trial": 1, "overall": 0.0},
            {"id": "wf-001", "trial": 2, "overall": 1.0},
            {"id": "wf-002", "trial": 1, "overall": 1.0},
        ]
        items = runner.per_item_summary(rows)
        self.assertEqual(items[0]["first_trial_perfect"], 0.0)
        self.assertEqual(items[1]["first_trial_perfect"], 1.0)

    def test_missing_first_trial_is_excluded_from_pass_at_one_mean(self):
        # B2 fix: items without trial 1 must not drag pass@1 down as failures.
        rows = [
            {"id": "wf-001", "trial": 2, "overall": 1.0},
            {"id": "wf-002", "trial": 1, "overall": 1.0},
        ]
        items = runner.per_item_summary(rows)
        pass_at_1 = runner.mean([
            row["first_trial_perfect"]
            for row in items
            if row["first_trial_available"] == 1.0
        ])
        self.assertEqual(pass_at_1, 1.0)

    def test_capture_availability_clamps_at_one(self):
        availability = runner.capture_availability(
            [{"id": "a"}],
            [{"id": "a", "trial": 1}, {"id": "a", "trial": 2}],
            {"trials": 1},
        )
        self.assertEqual(availability["capture_success_rate"], 1.0)

    def test_bootstrap_ci_is_deterministic_and_brackets_mean(self):
        ci = runner.bootstrap_ci([0.0, 1.0], iterations=500)
        self.assertEqual(ci, runner.bootstrap_ci([0.0, 1.0], iterations=500))
        self.assertLessEqual(ci["lower"], 0.5)
        self.assertGreaterEqual(ci["upper"], 0.5)

    def test_capture_availability_uses_manifest_trials(self):
        availability = runner.capture_availability(
            [{"id": "a"}, {"id": "b"}],
            [{"id": "a", "trial": 1}],
            {"trials": 2, "failures": 3},
        )
        self.assertEqual(availability["expected_trials"], 4)
        self.assertEqual(availability["capture_success_rate"], 0.25)

    def test_capture_availability_respects_limited_manifest(self):
        availability = runner.capture_availability(
            [{"id": "a"}, {"id": "b"}],
            [{"id": "a", "trial": 1}],
            {"trials": 1, "item_ids": ["a"], "failures": 0},
        )
        self.assertEqual(availability["selected_items"], 1)
        self.assertEqual(availability["capture_success_rate"], 1.0)

    def test_pick_capture_file_prefers_one_file_per_trial(self):
        with tempfile.TemporaryDirectory() as td:
            item_dir = Path(td) / "wf-001"
            item_dir.mkdir()
            (item_dir / "trial-001.oxoflow").write_text("[workflow]\nname='x'\n", encoding="utf-8")
            nested = item_dir / "trial-002"
            nested.mkdir()
            (nested / "generated.oxoflow").write_text("[workflow]\nname='y'\n", encoding="utf-8")
            files = runner.pick_capture_file(td, "wf-001")
            self.assertEqual([f["trial"] for f in files], [1, 2])


class VersionGateTests(unittest.TestCase):
    """#172: version_match is scored only when the query asks for a version
    (schema.md: expected_version is empty when the query does not ask; 45
    rows carried a version anyway and penalised correct answers)."""

    def _judge(self, query, expected_version, answer):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": answer, "error": ""})
            gold_rows = [
                {
                    "id": "tool-900",
                    "query": query,
                    "expected_tool": "fastp",
                    "expected_version": expected_version,
                    "negative_sample": "0",
                }
            ]
            return runner.judge_tool(gold_rows, str(captures))[0]

    def test_version_not_scored_when_query_never_asks(self):
        row = self._judge("adapter trimming and quality filtering", "1.3.7", "fastp")
        self.assertNotIn("version_match", row)
        self.assertEqual(row["overall"], 1.0)

    def test_version_scored_when_query_asks(self):
        row = self._judge("what is the latest version of fastp in bioconda", "1.3.7", "fastp 1.3.7")
        self.assertEqual(row["version_match"], 1.0)

    def test_missing_version_fails_when_query_asks(self):
        row = self._judge("what is the latest version of fastp in bioconda", "1.3.7", "fastp")
        self.assertEqual(row["version_match"], 0.0)

    def test_version_match_is_boundary_checked(self):
        # 2.0.40 / 12.0.4 must not satisfy expected 2.0.4; v-prefix still does.
        self.assertEqual(self._judge("what is the latest version of x", "2.0.4", "x 2.0.40")["version_match"], 0.0)
        self.assertEqual(self._judge("what is the latest version of x", "2.0.4", "x 12.0.4")["version_match"], 0.0)
        self.assertEqual(self._judge("what is the latest version of x", "2.0.4", "x v2.0.4")["version_match"], 1.0)


class CaptureFailureTests(unittest.TestCase):
    """A failed capture (rate limit, transport) is infrastructure, not a
    model answer — scoring the empty string would book the rate limiter as a
    hallucination (#172 live smoke against the school gateway)."""

    def test_errored_capture_is_skipped_not_scored_zero(self):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": "", "error": "HTTP 429"})
            gold_rows = [
                {
                    "id": "tool-900",
                    "query": "adapter trimming",
                    "expected_tool": "fastp",
                    "expected_version": "",
                    "negative_sample": "0",
                }
            ]
            results = runner.judge_tool(gold_rows, str(captures))
        self.assertEqual(results, [])


class RuleJudgingTests(unittest.TestCase):
    """#172 audit: reference-faithful answers must not be zeroed by
    package-name pins, docker build suffixes, or expand_inputs io."""

    def _judge(self, gold_row, capture_text):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / f"{gold_row['id']}.oxoflow"
            path.write_text(capture_text, encoding="utf-8")
            with mock.patch("common.oxo_flow_cmd", return_value=(0, "", "")):
                return runner.judge_rule([gold_row], td, "/bin/true")[0]

    def _row(self, **overrides):
        row = {
            "id": "rule-900",
            "expected_tool": "gatk",
            "expected_version": "4.5.0.0",
            "expected_key_params": "[]",
            "expected_inputs": "[]",
            "expected_outputs": "[]",
            "resource_range": "{}",
        }
        row.update(overrides)
        return row

    def test_executable_resolves_to_its_package_pin(self):
        result = self._judge(
            self._row(),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "haplotype_caller"\n'
            'shell = "gatk4=4.5.0.0; gatk HaplotypeCaller -R r -O o"\n',
        )
        self.assertEqual(result["version_pinned"], 1.0)

    def test_docker_build_suffix_is_truncated(self):
        self.assertEqual(
            runner.find_pinned_version(
                'docker = "quay.io/biocontainers/ucsc-bedgraphtobigwig:445--h954228d_0"',
                "bedGraphToBigWig",
            ),
            "445",
        )

    def test_expand_inputs_patterns_count_as_declared_io(self):
        result = self._judge(
            self._row(expected_inputs='["variants/{sample}.g.vcf.gz"]'),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "combine_gvcfs"\ninput = []\n'
            'expand_inputs = [\n    { pattern = "variants/{sample}.g.vcf.gz", variables = { sample = "config.samples" } }\n]\n'
            'output = ["variants/cohort.g.vcf.gz"]\n',
        )
        self.assertEqual(result["io_declared"], 1.0)

    def test_top_level_engine_resources_are_declared(self):
        result = self._judge(
            self._row(),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "a"\n'
            'threads = 4\nmemory = "8G"\nshell = "tool"\n',
        )
        self.assertEqual(result["resources_declared"], 1.0)
        self.assertEqual(result["resources_in_range"], 1.0)

    def test_key_params_are_boundary_checked(self):
        self.assertTrue(runner.param_present("--out", "tool --out result.txt"))
        self.assertFalse(runner.param_present("--out", "tool --output result.txt"))
        self.assertTrue(runner.param_present("-R", "gatk -R ref.fa"))
        self.assertFalse(runner.param_present("-R", "gatk -RNA ref.fa"))
        self.assertFalse(runner.param_present("index", "tool indexing"))

    def test_corrupted_gold_cell_is_surfaced_not_vacuously_perfect(self):
        # A malformed JSON cell must not read as "no expectations" — the
        # row's capture_error carries the corruption like a capture failure
        # would (same policy as check_gold.py's hard finding).
        result = self._judge(
            self._row(expected_key_params="not-json["),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "a"\nshell = "tool"\n',
        )
        self.assertIn("expected_key_params", result["capture_error"])
        self.assertNotIn("key_params", result)

    def test_corrupted_resource_range_cell_scores_zero_not_vacuous_pass(self):
        # A corrupted range cell must not fall back to the lenient defaults
        # (1-128 threads / 262144 MB) and read as a vacuous in-range pass —
        # same policy as the other gold-cell corruption checks.
        capture = (
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "a"\n'
            'threads = 4\nmemory = "8G"\nshell = "tool"\n'
        )
        for bad_cell in ("not-json[", "null", '"8G"'):
            result = self._judge(self._row(resource_range=bad_cell), capture)
            self.assertEqual(result["resources_in_range"], 0.0, bad_cell)
            self.assertIn("resource_range", result["capture_error"])
            self.assertEqual(result["resources_declared"], 1.0)


class FailedRuleCaptureTests(unittest.TestCase):
    def test_meta_error_is_skipped_not_scored_zero(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "rule-900.oxoflow"
            path.write_text("not TOML", encoding="utf-8")
            meta = Path(td) / "rule-900.oxoflow.meta.json"
            meta.write_text('{"error":"HTTP 429"}', encoding="utf-8")
            gold = [{
                "id": "rule-900", "expected_tool": "fastp", "expected_version": "",
                "expected_key_params": "[]", "expected_inputs": "[]",
                "expected_outputs": "[]", "resource_range": "{}",
            }]
            with mock.patch("common.oxo_flow_cmd", return_value=(0, "", "")):
                results = runner.judge_rule(gold, td, "/bin/true")
        self.assertEqual(results, [])


class WorkflowJudgingTests(unittest.TestCase):
    """Regression: an edge-only gold row (expected_steps empty or unparseable,
    expected_dag_edges non-empty) used to hit a NameError on the step-name map
    inside judge_workflow."""

    def _judge(self, gold_row, capture_text):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / f"{gold_row['id']}.oxoflow"
            path.write_text(capture_text, encoding="utf-8")
            with mock.patch("common.oxo_flow_cmd", return_value=(0, "", "")):
                return runner.judge_workflow([gold_row], td, "/bin/true")[0]

    def _row(self, **overrides):
        row = {
            "id": "wf-900",
            "expected_steps": "[]",
            "expected_tools": "[]",
            "expected_dag_edges": "[]",
            "expected_outputs": "[]",
        }
        row.update(overrides)
        return row

    def test_edge_only_gold_row_degrades_to_zero_coverage_not_crash(self):
        result = self._judge(
            self._row(expected_dag_edges='[["a", "b"]]'),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "a"\noutput = ["a.txt"]\n',
        )
        self.assertEqual(result["edge_coverage"], 0.0)
        self.assertNotIn("step_coverage", result)

    def test_steps_and_edges_still_score_together(self):
        result = self._judge(
            self._row(expected_steps='["a"]', expected_dag_edges='[["a", "a"]]'),
            '[workflow]\nname = "x"\n\n[[rules]]\nname = "a"\noutput = ["a.txt"]\n',
        )
        self.assertEqual(result["step_coverage"], 1.0)
        # The engine graph is empty under the mocked binary, so the mapped
        # edge cannot hit — the metric must simply score 0, not crash.
        self.assertEqual(result["edge_coverage"], 0.0)


class ParserTests(unittest.TestCase):
    def test_load_generated_merges_defaults_into_rules(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "wf.oxoflow"
            path.write_text(
                '[workflow]\nname = "x"\n\n'
                "[defaults]\nthreads = 4\nmemory = \"8G\"\n\n"
                '[[rules]]\nname = "a"\noutput = ["x.txt"]\n\n'
                '[[rules]]\nname = "b"\nthreads = 8\noutput = ["y.txt"]\n',
                encoding="utf-8",
            )
            _, rules, err = runner.load_generated(str(path))
            self.assertIsNone(err)
            self.assertEqual(rules[0]["threads"], 4)
            self.assertEqual(rules[0]["memory"], "8G")
            self.assertEqual(rules[1]["threads"], 8)  # rule value wins over defaults

    def test_metric_fields_unions_across_rows(self):
        results = [
            {"id": "a", "trial": 1, "name_match": 1.0},
            {"id": "b", "trial": 1, "no_hallucination": 0.0},
        ]
        self.assertEqual(runner.metric_fields(results), ["name_match", "no_hallucination"])


class DotEdgeParsingTests(unittest.TestCase):
    def test_parse_dot_edges_maps_labels(self):
        dot = 'digraph {\n    0 [ label = "fastp"]\n    1 [ label = "multiqc"]\n    0 -> 1 [ ]\n}'
        self.assertEqual(runner.parse_dot_edges(dot), {("fastp", "multiqc")})

    def test_parse_dot_edges_ignores_dangling_references(self):
        dot = 'digraph {\n    0 [ label = "a"]\n    0 -> 9 [ ]\n}'
        self.assertEqual(runner.parse_dot_edges(dot), set())

    def test_parse_dot_edges_empty_graph(self):
        self.assertEqual(runner.parse_dot_edges("digraph {\n}"), set())


class NegativeSampleJudgingTests(unittest.TestCase):
    """Regression tests for #172: a correct rejection of a fabricated tool has
    to be earnable — the answer necessarily echoes the queried name — while a
    hallucinated fallback suggestion must still fail."""

    def _judge(self, query, answer, known_names):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": answer, "error": ""})
            gold_rows = [
                {
                    "id": "tool-900",
                    "query": query,
                    "expected_tool": "",
                    "expected_version": "",
                    "negative_sample": "1",
                }
            ]
            with mock.patch("common.known_tool_names", return_value=known_names):
                results = runner.judge_tool(gold_rows, str(captures))
            return results[0]["no_hallucination"]

    def test_correct_rejection_naming_the_tool_and_databases_scores(self):
        # `seq_shampooer` embeds the real tool name `shampoo`-style substrings;
        # the queried fake name must be excluded before scanning, and short KB
        # names (`bio`, `fc`) must not match inside longer words (`bioconda`,
        # `nf-core`).
        score = self._judge(
            "what is seq_shampooer",
            "I could not find any tool named seq_shampooer. "
            "It does not exist in bioconda or nf-core.",
            {"bio", "bioc", "fc", "shampoo", "shampoos"},
        )
        self.assertEqual(score, 1.0)

    def test_recognition_failure_is_a_rejection(self):
        score = self._judge(
            "what is seq_polisher_pro",
            "I do not recognize seq_polisher_pro in Bioconda.",
            set(),
        )
        self.assertEqual(score, 1.0)

    def test_spaced_echo_of_the_queried_name_is_excluded(self):
        score = self._judge(
            "what is variant_valkyrie",
            "No tool called variant valkyrie exists in bioconda.",
            {"variantvalkyrie", "bwa"},
        )
        self.assertEqual(score, 1.0)

    def test_rejection_suggesting_a_real_fallback_still_fails(self):
        score = self._judge(
            "what is seq_shampooer",
            "No tool named seq_shampooer — maybe you meant shampoos?",
            {"shampoo", "shampoos"},
        )
        self.assertEqual(score, 0.0)

    def test_unrelated_tool_mention_without_a_cue_is_not_a_suggestion(self):
        # "not related to bwameth" rejects the fake without suggesting bwameth.
        score = self._judge(
            "what does aligninator do",
            "I could not find aligninator; it is not related to bwameth.",
            {"bwameth"},
        )
        self.assertEqual(score, 1.0)

    def test_cue_scoped_suggestion_still_fails(self):
        score = self._judge(
            "what does aligninator do",
            "I could not find aligninator — did you mean bwameth?",
            {"bwameth"},
        )
        self.assertEqual(score, 0.0)

    def test_format_word_mention_in_a_rejection_passes(self):
        # `sam`/`bam` are real KB entries; a rejection mentioning the file
        # format without a cue must not be read as a fallback suggestion.
        score = self._judge(
            "what is seq_polisher_pro",
            "No tool named seq_polisher_pro exists; it is not a SAM/BAM format tool.",
            {"sam", "bam"},
        )
        self.assertEqual(score, 1.0)

    def test_hyphen_spellings_of_real_tools_are_still_caught(self):
        self.assertEqual(
            self._judge(
                "what is seq_polisher_pro",
                "No tool named seq_polisher_pro — maybe you meant fast-qc?",
                {"fastqc"},
            ),
            0.0,
        )
        self.assertEqual(
            self._judge(
                "what is seq_polisher_pro",
                "No tool named seq_polisher_pro — perhaps multi-qc covers this?",
                {"multiqc"},
            ),
            0.0,
        )

    def test_short_names_do_not_match_inside_longer_words(self):
        self.assertFalse(
            runner.answer_mentions_known_tool(
                "It is not in bioconda or nf-core.", {"bio", "fc"}
            )
        )
        self.assertTrue(
            runner.answer_mentions_known_tool("Not found; use bwa instead.", {"bwa"})
        )


class ValidityWarningTests(unittest.TestCase):
    def test_summary_warns_on_same_family_and_preview_mode(self):
        summary = {"n_items": 2, "by_difficulty": {"easy": {"n": 2}}, "by_query_type": {"alias": {"n": 2}}}
        gold_rows = [{"gold_draft_by": "claude"}, {"gold_draft_by": "claude"}]
        manifest = {"provider": {"kind": "claude"}, "include_unreviewed": True}
        warnings = runner.build_validity_warnings(summary, gold_rows, manifest)
        self.assertTrue(any("same-family" in w or "gold drafter" in w for w in warnings))
        self.assertTrue(any("preview-only" in w for w in warnings))

    def test_partial_availability_and_missing_first_trial_are_flagged(self):
        summary = {
            "n_items": 2,
            "capture_availability": {"capture_success_rate": 0.5},
            "by_difficulty": {},
            "by_query_type": {},
            "per_item": [
                {"id": "wf-001", "first_trial_available": 0.0},
                {"id": "wf-002", "first_trial_available": 1.0},
            ],
        }
        warnings = runner.build_validity_warnings(summary, [], {})
        self.assertTrue(any("Capture availability is 0.500" in w for w in warnings))
        self.assertTrue(any("wf-001" in w and "pass@1" in w for w in warnings))


class AIJudgeTests(unittest.TestCase):
    def test_provider_resolution_uses_environment_not_cli_secrets(self):
        env = {
            "AI_JUDGE_PROVIDER": "openai",
            "AI_JUDGE_BASE_URL": "http://example.test/v1",
            "AI_JUDGE_API_KEY": "secret",
            "AI_JUDGE_MODEL": "judge-model",
        }
        with mock.patch.dict(os.environ, env, clear=True):
            provider = ai_judge.resolve_provider()
        self.assertEqual(
            provider,
            {
                "kind": "openai",
                "api_url": "http://example.test/v1",
                "api_key": "secret",
                "model": "judge-model",
            },
        )
        self.assertNotIn("api_key", ai_judge.public_provider(provider))

    def test_provider_rejects_credentialed_or_parameterized_endpoints(self):
        env = {
            "AI_JUDGE_PROVIDER": "openai",
            "AI_JUDGE_MODEL": "judge-model",
        }
        for endpoint in (
            "http://user@example.test/v1",
            "http://example.test/v1?token=secret",
            "http://example.test/v1#fragment",
        ):
            with mock.patch.dict(os.environ, {**env, "AI_JUDGE_BASE_URL": endpoint}, clear=True):
                with self.assertRaises(ValueError):
                    ai_judge.resolve_provider()

    def test_normalize_review_recomputes_overall_and_requires_rationales(self):
        review = ai_judge.normalize_review(
            {
                "scientific_correctness": {"score": 8, "rationale": "sound"},
                "completeness": {"score": 6, "rationale": "one output missing"},
                "engine_conformance": {"score": 9, "rationale": "valid TOML"},
                "usability": {"score": 7, "rationale": "runnable"},
                "confidence": "high",
                "major_risks": ["missing output"],
            }
        )
        self.assertEqual(review["ai_judge_overall"], 7.5)
        self.assertIn("missing output", review["major_risks_json"])

    def test_advisory_failure_is_recorded_without_fabricated_scores(self):
        with tempfile.TemporaryDirectory() as td:
            captures = Path(td) / "answers.csv"
            with open(captures, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "trial", "answer", "error"])
                writer.writeheader()
                writer.writerow({"id": "tool-900", "trial": 1, "answer": "fastp", "error": ""})
            results = [{"id": "tool-900", "trial": 1}]
            gold = [{"id": "tool-900", "query": "what is fastp"}]
            provider = {"kind": "openai", "api_url": "http://test", "api_key": "x", "model": "m"}
            with mock.patch("common.chat", side_effect=RuntimeError("HTTP 429")):
                rows = ai_judge.evaluate_results(results, "tool", gold, str(captures), provider)
        self.assertEqual(rows[0]["judge_error"], "HTTP 429")
        self.assertEqual(rows[0]["ai_judge_overall"], "")


if __name__ == "__main__":
    unittest.main()
