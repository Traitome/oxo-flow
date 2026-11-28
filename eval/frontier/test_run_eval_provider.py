"""Provider-native tests for the governed ablation harness (issue #1395).

No network: every provider call is mocked and every other function under
test is pure. These tests pin the protocol adaptation frozen by the
campaign pre-registration: Arm A's bare prompt can run over the
OpenAI-compatible chat-completions protocol, and Arms B/C pin the resolved
provider without inheriting ambient agent credentials.
"""

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import run_eval


class OpenAiUrlTests(unittest.TestCase):
    def test_v1_base_appends_chat_completions(self):
        self.assertEqual(
            run_eval.openai_chat_url("http://proxy.example:1234/csuapi/v1"),
            "http://proxy.example:1234/csuapi/v1/chat/completions",
        )

    def test_full_endpoint_passes_through(self):
        url = "https://api.example.com/v1/chat/completions"
        self.assertEqual(run_eval.openai_chat_url(url), url)

    def test_versioned_non_v1_base_appends_suffix(self):
        # Mirrors the Rust normalizer's GLM /v4 case.
        self.assertEqual(
            run_eval.openai_chat_url("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions",
        )

    def test_bare_base_gains_v1(self):
        self.assertEqual(
            run_eval.openai_chat_url("https://api.example.com"),
            "https://api.example.com/v1/chat/completions",
        )

    def test_trailing_slash_is_normalized(self):
        self.assertEqual(
            run_eval.openai_chat_url("https://api.example.com/v1/"),
            "https://api.example.com/v1/chat/completions",
        )


class ArmAProtocolTests(unittest.TestCase):
    def test_auto_follows_the_resolved_provider(self):
        self.assertEqual(run_eval.resolve_arm_a_protocol("auto", "claude"), "anthropic")
        self.assertEqual(run_eval.resolve_arm_a_protocol("auto", "openai"), "openai")
        self.assertEqual(run_eval.resolve_arm_a_protocol("auto", "deepseek"), "openai")

    def test_explicit_pin_wins(self):
        self.assertEqual(run_eval.resolve_arm_a_protocol("anthropic", "openai"), "anthropic")
        self.assertEqual(run_eval.resolve_arm_a_protocol("openai", "claude"), "openai")


class ChildEnvTests(unittest.TestCase):
    def test_child_env_pins_provider_and_scrubs_ambient_credentials(self):
        creds = {
            "provider": "openai",
            "api_key": "resolved-key",
            "api_url": "http://proxy.example/v1",
            "model": "DeepSeek",
        }
        ambient = {
            "ANTHROPIC_AUTH_TOKEN": "ambient-agent-token",
            "ANTHROPIC_BASE_URL": "http://ambient.example",
            "OPENAI_API_KEY": "ambient-openai-key",
            "DEEPSEEK_API_KEY": "ambient-deepseek-key",
            "OXO_FLOW_AI_API_KEY": "ambient-generic-key",
            "PATH": os.environ.get("PATH", ""),
        }
        with mock.patch.dict(os.environ, ambient, clear=True):
            env = run_eval.build_child_env(creds)

        self.assertEqual(env["OXO_FLOW_AI_PROVIDER"], "openai")
        self.assertEqual(env["OXO_FLOW_AI_API_KEY"], "resolved-key")
        self.assertEqual(env["OXO_FLOW_AI_API_URL"], "http://proxy.example/v1")
        self.assertEqual(env["OXO_FLOW_AI_MODEL"], "DeepSeek")
        for key in run_eval.AMBIENT_CREDENTIAL_VARS:
            # The generic OXO_FLOW_AI_* trio is the resolved carrier by
            # design; every provider-specific ambient variable must be gone.
            if key in ("OXO_FLOW_AI_API_KEY", "OXO_FLOW_AI_API_URL",
                       "OXO_FLOW_AI_MODEL"):
                continue
            self.assertNotIn(key, env, key)
        self.assertNotIn("ambient", json.dumps(env))
        self.assertEqual(env.get("PATH"), ambient["PATH"])


class TemplateCommandTests(unittest.TestCase):
    class Args:
        profile = "compact"
        attempts = 1
        max_retries = None

    def _command(self, variant):
        return run_eval.template_command(
            variant, "quality control with fastp", Path("/tmp/run"), "/bin/oxo-flow",
            self.Args(),
        )

    def test_arm_b_omits_and_arm_c_adds_contract_feedback(self):
        arm_b = self._command("syntax-only")
        arm_c = self._command("full-contract")
        self.assertNotIn("--ai-contract-feedback", arm_b)
        self.assertIn("--ai-contract-feedback", arm_c)
        # The toggle is the ONLY difference between the two argv vectors.
        self.assertEqual(arm_c, arm_b + ["--ai-contract-feedback"])

    def test_both_engine_arms_use_the_template_ai_path(self):
        for variant in ("syntax-only", "full-contract"):
            cmd = self._command(variant)
            self.assertEqual(cmd[1:3], ["template", "--ai"])
            self.assertNotIn("--ai-team-profile", cmd)  # compact is not Arm C


class CredentialSourceTests(unittest.TestCase):
    CREDS = {
        "provider": "openai",
        "api_key": "config-key",
        "api_url": "http://proxy.example/v1",
        "model": "DeepSeek",
    }

    def _write_config(self, directory, payload):
        path = Path(directory) / "ai_config.json"
        path.write_text(json.dumps(payload))
        return path

    def test_config_source_reads_the_operator_provider(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write_config(tmp, self.CREDS)
            with mock.patch.object(run_eval, "CFG_PATH", path):
                creds = run_eval.load_credentials("config")
        self.assertEqual(creds, self.CREDS)

    def test_config_source_rejects_an_unknown_provider(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write_config(tmp, {**self.CREDS, "provider": "mystery"})
            with mock.patch.object(run_eval, "CFG_PATH", path):
                with self.assertRaisesRegex(SystemExit, "not one of"):
                    run_eval.load_credentials("config")

    def test_config_source_fails_closed_on_missing_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write_config(tmp, {**self.CREDS, "api_key": ""})
            with mock.patch.object(run_eval, "CFG_PATH", path):
                with self.assertRaisesRegex(SystemExit, "missing api_key"):
                    run_eval.load_credentials("config")

    def test_config_source_fails_closed_when_the_file_is_absent(self):
        missing = Path(tempfile.gettempdir()) / "definitely-not-an-oxo-config.json"
        with mock.patch.object(run_eval, "CFG_PATH", missing):
            with self.assertRaisesRegex(SystemExit, "does not exist"):
                run_eval.load_credentials("config")

    def test_env_source_uses_the_anthropic_tier(self):
        env = {
            "ANTHROPIC_AUTH_TOKEN": "env-key",
            "ANTHROPIC_BASE_URL": "http://env.example",
            "ANTHROPIC_MODEL": "claude-test",
        }
        with mock.patch.dict(os.environ, env, clear=True):
            creds = run_eval.load_credentials("env")
        self.assertEqual(creds, {
            "provider": "claude",
            "api_key": "env-key",
            "api_url": "http://env.example",
            "model": "claude-test",
        })

    def test_auto_prefers_a_complete_env_tier_then_falls_back(self):
        full_env = {
            "ANTHROPIC_AUTH_TOKEN": "env-key",
            "ANTHROPIC_BASE_URL": "http://env.example",
            "ANTHROPIC_MODEL": "claude-test",
        }
        with mock.patch.dict(os.environ, full_env, clear=True):
            self.assertEqual(run_eval.load_credentials("auto")["provider"], "claude")
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write_config(tmp, self.CREDS)
            with mock.patch.dict(os.environ, {}, clear=True):
                with mock.patch.object(run_eval, "CFG_PATH", path):
                    self.assertEqual(run_eval.load_credentials("auto")["provider"], "openai")


class _FakeResponse:
    def __init__(self, payload):
        self._payload = json.dumps(payload).encode()

    def read(self):
        return self._payload

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class OpenAiChatTests(unittest.TestCase):
    CREDS = {
        "provider": "openai",
        "api_key": "test-key",
        "api_url": "http://proxy.example/v1",
        "model": "DeepSeek",
    }

    def test_parses_content_usage_and_reported_model(self):
        payload = {
            "model": "DeepSeek-V4-Backend",
            "choices": [{"message": {"content": "hello"}}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4},
        }
        captured = {}

        def fake_urlopen(request, timeout):
            captured["url"] = request.full_url
            captured["authorization"] = request.headers.get("Authorization")
            return _FakeResponse(payload)

        with mock.patch.object(run_eval.urllib.request, "urlopen", fake_urlopen):
            text, usage, error = run_eval.openai_chat(
                self.CREDS, "DeepSeek", "system", "user", max_tokens=10, timeout=5,
            )

        self.assertIsNone(error)
        self.assertEqual(text, "hello")
        self.assertEqual(usage["input"], 3)
        self.assertEqual(usage["output"], 4)
        self.assertEqual(usage["reported_model"], "DeepSeek-V4-Backend")
        self.assertEqual(captured["url"], "http://proxy.example/v1/chat/completions")
        self.assertEqual(captured["authorization"], "Bearer test-key")

    def test_error_payload_is_reported_without_the_key(self):
        payload = {"error": {"message": "model not found"}}

        def fake_urlopen(request, timeout):
            return _FakeResponse(payload)

        with mock.patch.object(run_eval.urllib.request, "urlopen", fake_urlopen):
            text, usage, error = run_eval.openai_chat(
                self.CREDS, "DeepSeek", "system", "user", max_tokens=10, timeout=5,
            )
        self.assertIsNone(text)
        self.assertEqual(usage, {})
        self.assertEqual(error, "model not found")
        self.assertNotIn("test-key", error)

    def test_empty_content_is_not_a_generation(self):
        payload = {"model": "m", "choices": [{"message": {"content": ""}}], "usage": {}}

        def fake_urlopen(request, timeout):
            return _FakeResponse(payload)

        with mock.patch.object(run_eval.urllib.request, "urlopen", fake_urlopen):
            text, _usage, error = run_eval.openai_chat(
                self.CREDS, "m", "system", "user", max_tokens=10, timeout=5,
            )
        self.assertIsNone(text)
        self.assertIsNone(error)


class ManifestProvenanceTests(unittest.TestCase):
    def test_sha256_file_is_stable_and_prefixed(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "blob"
            path.write_bytes(b"oxo-flow")
            first = run_eval.sha256_file(path)
            second = run_eval.sha256_file(path)
        self.assertEqual(first, second)
        self.assertTrue(first.startswith("sha256:"))
        self.assertEqual(len(first), len("sha256:") + 64)

    def test_sha256_file_returns_none_for_a_missing_file(self):
        missing = Path(tempfile.gettempdir()) / "definitely-not-a-blob"
        self.assertIsNone(run_eval.sha256_file(missing))


if __name__ == "__main__":
    unittest.main()
