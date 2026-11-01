import csv
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common


class ResolveProviderTests(unittest.TestCase):
    def test_openai_env_resolution_uses_provider_defaults(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "openai",
            "OPENAI_API_KEY": "sk-test",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(
            provider,
            {
                "kind": "openai",
                "api_url": common.DEFAULT_OPENAI_URL,
                "api_key": "sk-test",
                "model": common.DEFAULT_OPENAI_MODEL,
            },
        )

    def test_claude_env_resolution_prefers_anthropic_variables(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "claude",
            "ANTHROPIC_AUTH_TOKEN": "sk-ant-test",
            "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
            "ANTHROPIC_MODEL": "claude-test",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(
            provider,
            {
                "kind": "claude",
                "api_url": "https://api.anthropic.com",
                "api_key": "sk-ant-test",
                "model": "claude-test",
            },
        )

    def test_ollama_does_not_require_api_key(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "ollama",
            "OLLAMA_HOST": "http://127.0.0.1:11434",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(
            provider,
            {
                "kind": "ollama",
                "api_url": "http://127.0.0.1:11434",
                "api_key": "",
                "model": common.DEFAULT_OLLAMA_MODEL,
            },
        )

    def test_specific_provider_key_beats_generic_fallback(self):
        # #948 P2-3: the generic OXO_FLOW_AI_API_KEY used to sit FIRST and
        # shadow the per-provider variable (the #645 bug shape).
        env = {
            "OXO_FLOW_AI_PROVIDER": "openai",
            "OPENAI_API_KEY": "sk-specific",
            "OXO_FLOW_AI_API_KEY": "sk-generic",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(provider["api_key"], "sk-specific")

    def test_generic_key_used_when_specific_missing(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "deepseek",
            "OXO_FLOW_AI_API_KEY": "sk-generic",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(provider["api_key"], "sk-generic")

    def test_whitespace_only_key_is_unconfigured(self):
        env = {
            "OXO_FLOW_AI_PROVIDER": "claude",
            "ANTHROPIC_AUTH_TOKEN": "   ",
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch("os.path.exists", return_value=False):
            self.assertIsNone(common.resolve_provider())


class UrlHelperTests(unittest.TestCase):
    def test_openai_url_helper_accepts_base_or_full_endpoint(self):
        self.assertEqual(
            common._ensure_openai_chat_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions",
        )
        self.assertEqual(
            common._ensure_openai_chat_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions",
        )
        self.assertEqual(
            common._ensure_openai_chat_url("https://example.com/chat/completions"),
            "https://example.com/chat/completions",
        )

    def test_claude_url_helper_accepts_base_or_full_endpoint(self):
        self.assertEqual(
            common._ensure_claude_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages",
        )
        self.assertEqual(
            common._ensure_claude_messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages",
        )
        self.assertEqual(
            common._ensure_claude_messages_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages",
        )

    def test_claude_payload_moves_system_prompt_to_top_level(self):
        payload = common._claude_payload(
            [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"},
            ],
            "claude-test",
            256,
            0.1,
        )
        self.assertEqual(payload["system"], "sys")
        self.assertEqual(
            payload["messages"],
            [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"},
            ],
        )


class GoldLoadingTests(unittest.TestCase):
    def test_load_gold_requires_approved_rows_by_default(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "tool.csv"
            with open(path, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "review_status"])
                writer.writeheader()
                writer.writerow({"id": "tool-001", "review_status": "draft"})
            with self.assertRaises(SystemExit):
                common.load_gold("tool", False, str(path))

    def test_load_gold_include_unreviewed_returns_rows(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "tool.csv"
            with open(path, "w", newline="", encoding="utf-8") as fh:
                writer = csv.DictWriter(fh, fieldnames=["id", "review_status"])
                writer.writeheader()
                writer.writerow({"id": "tool-001", "review_status": "draft"})
            rows = common.load_gold("tool", True, str(path))
            self.assertEqual(len(rows), 1)


class NamePresentTests(unittest.TestCase):
    def test_short_names_require_token_boundaries(self):
        self.assertFalse(common.name_present("tw", "a network of tools"))
        self.assertFalse(common.name_present("star", "starting point"))
        self.assertTrue(common.name_present("tw", "the tw CLI"))
        self.assertTrue(common.name_present("bwa", "bwa-mem2 alignment"))

    def test_short_alias_resolves_against_package_spellings(self):
        self.assertTrue(common.name_present("VEP", "Ensembl VEP annotates variants"))
        self.assertTrue(common.name_present("vep", "ensembl-vep was used"))

    def test_long_names_keep_separator_tolerant_substring(self):
        self.assertTrue(common.name_present("bwa-mem2", "ran bwa_mem2 on the reads"))


class StepMappingTests(unittest.TestCase):
    def test_map_expected_steps_prefers_exact_match(self):
        mapping = common.map_expected_steps(
            ["apply_vqsr_indels"], ["vqsr_indels", "apply_vqsr_indels"]
        )
        self.assertEqual(mapping["apply_vqsr_indels"], "apply_vqsr_indels")

    def test_map_expected_steps_falls_back_to_loose_match(self):
        mapping = common.map_expected_steps(["fastqc"], ["qc::fastqc"])
        self.assertEqual(mapping["fastqc"], "qc::fastqc")

    def test_map_expected_steps_leaves_unmatched_steps_out(self):
        self.assertEqual(common.map_expected_steps(["missing"], ["a", "b"]), {})

    def test_step_mapping_is_one_to_one(self):
        mapping = common.map_expected_steps(
            ["vqsr_snps", "apply_vqsr_snps"],
            ["vqsr_snps", "apply_vqsr_snps"],
        )
        self.assertEqual(
            mapping,
            {
                "vqsr_snps": "vqsr_snps",
                "apply_vqsr_snps": "apply_vqsr_snps",
            },
        )

    def test_bare_tool_rule_cannot_satisfy_an_unrelated_expected_step(self):
        # B1 fix: a `samtools` rule with environment `samtools` must not bind
        # an unmapped `fastqc` step just because both tools appear in expected_tools.
        rules = [{
            "name": "samtools",
            "environment": {"conda": "bioconda::samtools=1.24"},
        }]
        mapping = common.map_expected_steps(
            ["fastqc"],
            ["samtools"],
            ["fastqc", "samtools"],
            rules,
        )
        self.assertEqual(mapping, {})

    def test_semantic_rule_names_match_without_reusing_a_generic_helper(self):
        rules = [
            {"name": "write_greeting", "output": ["out/hello.txt"]},
            {"name": "fastp", "environment": {"conda": "bioconda::fastp=1.4.0"}},
        ]
        mapping = common.map_expected_steps(
            ["greet", "fastp_trim"],
            [rule["name"] for rule in rules],
            ["echo", "fastp"],
            rules,
        )
        self.assertEqual(mapping["greet"], "write_greeting")
        self.assertEqual(mapping["fastp_trim"], "fastp")

    def test_environment_identity_cannot_bind_the_wrong_indexer(self):
        rules = [
            {
                "name": "star_index",
                "environment": {"conda": "bioconda::star=2.7.11b"},
                "output": ["reference/star_index"],
            },
            {
                "name": "samtools_index",
                "environment": {"conda": "bioconda::samtools=1.24"},
                "output": ["aligned/{sample}.bam.bai"],
            },
        ]
        mapping = common.map_expected_steps(
            ["star_align", "index_bam"],
            [rule["name"] for rule in rules],
            ["STAR", "samtools"],
            rules,
        )
        # The fixture has no alignment rule, so star_index must remain free.
        self.assertNotIn("star_align", mapping)
        self.assertEqual(mapping["index_bam"], "samtools_index")


class MatchingTests(unittest.TestCase):
    def test_path_matches_allows_config_prefix_but_not_wrong_directory(self):
        self.assertTrue(
            common.path_matches(
                "results/multiqc_report.html",
                "{config.out_dir}/results/multiqc_report.html",
            )
        )
        self.assertFalse(
            common.path_matches(
                "aligned/sample.bam",
                "dedup/sample.bam",
            )
        )

    def test_step_match_handles_namespaced_rules(self):
        self.assertTrue(common.loose_step_match("qc::fastqc", "qc::fastqc"))
        self.assertTrue(common.loose_step_match("fastqc", "qc::fastqc"))
        self.assertFalse(common.loose_step_match("star", "starfish_alignment"))

    def test_agent_nouns_are_not_treated_as_morphology(self):
        # `counter`, `caller`, and `aligner` can be distinct tools; only
        # grammatical inflections are allowed to bridge rule-name variants.
        self.assertFalse(common._tokens_equivalent("count", "counter"))
        self.assertFalse(common._tokens_equivalent("call", "caller"))
        self.assertFalse(common._tokens_equivalent("align", "aligner"))
        self.assertTrue(common._tokens_equivalent("greet", "greeting"))

    def test_path_roles_match_across_layout_but_not_scientific_type(self):
        self.assertTrue(
            common.path_role_matches(
                "results/multiqc_report.html", "qc/multiqc_report.html"
            )
        )
        self.assertTrue(
            common.path_role_matches(
                "counts/{sample}.counts.txt", "counts/{sample}_gene_counts.txt"
            )
        )
        self.assertFalse(
            common.path_role_matches(
                "variants/{sample}.g.vcf.gz", "variants/{sample}.vcf.gz"
            )
        )
        self.assertFalse(
            common.path_role_matches(
                "results/multiqc_report.html", "results/summary.txt"
            )
        )
        # Contiguous stem check: a GVCF cannot satisfy a plain-VCF request.
        self.assertFalse(
            common.path_role_matches(
                "variants/{sample}.vcf.gz", "variants/{sample}.g.vcf.gz"
            )
        )


class JsonRequestRetryTests(unittest.TestCase):
    @staticmethod
    def _http_error(code, body):
        return urllib.error.HTTPError(
            url="http://test", code=code, msg="err", hdrs=None, fp=io.BytesIO(body.encode())
        )

    def test_transient_empty_502_is_replayed_then_succeeds(self):
        responses = [
            self._http_error(502, ""),
            {"content": [{"type": "text", "text": "ok"}]},
        ]

        def fake_urlopen(req, timeout):
            item = responses.pop(0)
            if isinstance(item, Exception):
                raise item
            return contextlib.nullcontext(io.BytesIO(json.dumps(item).encode()))

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen):
            data = common._json_request("http://test", {}, {})
        self.assertEqual(data["content"][0]["text"], "ok")

    def test_parseable_error_payload_surfaces_immediately(self):
        calls = []

        def fake_urlopen(req, timeout):
            calls.append(1)
            raise self._http_error(503, '{"error":"model_not_found"}')

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen):
            with self.assertRaisesRegex(RuntimeError, "model_not_found"):
                common._json_request("http://test", {}, {})
        self.assertEqual(len(calls), 1)

    def test_client_error_is_never_replayed(self):
        calls = []

        def fake_urlopen(req, timeout):
            calls.append(1)
            raise self._http_error(401, "")

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen):
            with self.assertRaisesRegex(RuntimeError, "HTTP 401"):
                common._json_request("http://test", {}, {})
        self.assertEqual(len(calls), 1)

    def test_rate_limit_with_html_body_is_replayed(self):
        responses = [
            self._http_error(429, "<html>429 Too Many Requests</html>"),
            {"content": [{"type": "text", "text": "ok"}]},
        ]

        def fake_urlopen(req, timeout):
            item = responses.pop(0)
            if isinstance(item, Exception):
                raise item
            return contextlib.nullcontext(io.BytesIO(json.dumps(item).encode()))

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen), mock.patch(
            "common.time.sleep"
        ):
            data = common._json_request("http://test", {}, {})
        self.assertEqual(data["content"][0]["text"], "ok")


if __name__ == "__main__":
    unittest.main()
