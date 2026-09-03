import csv
import os
import sys
import tempfile
import unittest
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


if __name__ == "__main__":
    unittest.main()
