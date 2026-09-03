import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common


class ResolveProviderTests(unittest.TestCase):
    def test_openai_env_resolution_uses_provider_defaults(self):
        env = {
            'OXO_FLOW_AI_PROVIDER': 'openai',
            'OPENAI_API_KEY': 'sk-test',
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch('os.path.exists', return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(provider, {
            'kind': 'openai',
            'api_url': common.DEFAULT_OPENAI_URL,
            'api_key': 'sk-test',
            'model': common.DEFAULT_OPENAI_MODEL,
        })

    def test_claude_env_resolution_prefers_anthropic_variables(self):
        env = {
            'OXO_FLOW_AI_PROVIDER': 'claude',
            'ANTHROPIC_AUTH_TOKEN': 'sk-ant-test',
            'ANTHROPIC_BASE_URL': 'https://api.anthropic.com',
            'ANTHROPIC_MODEL': 'claude-test',
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch('os.path.exists', return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(provider, {
            'kind': 'claude',
            'api_url': 'https://api.anthropic.com',
            'api_key': 'sk-ant-test',
            'model': 'claude-test',
        })

    def test_ollama_does_not_require_api_key(self):
        env = {
            'OXO_FLOW_AI_PROVIDER': 'ollama',
            'OLLAMA_HOST': 'http://127.0.0.1:11434',
        }
        with mock.patch.dict(os.environ, env, clear=True), mock.patch('os.path.exists', return_value=False):
            provider = common.resolve_provider()
        self.assertEqual(provider, {
            'kind': 'ollama',
            'api_url': 'http://127.0.0.1:11434',
            'api_key': '',
            'model': common.DEFAULT_OLLAMA_MODEL,
        })


class UrlHelperTests(unittest.TestCase):
    def test_openai_url_helper_accepts_base_or_full_endpoint(self):
        self.assertEqual(
            common._ensure_openai_chat_url('https://api.openai.com/v1'),
            'https://api.openai.com/v1/chat/completions',
        )
        self.assertEqual(
            common._ensure_openai_chat_url('https://api.deepseek.com'),
            'https://api.deepseek.com/v1/chat/completions',
        )
        self.assertEqual(
            common._ensure_openai_chat_url('https://example.com/chat/completions'),
            'https://example.com/chat/completions',
        )

    def test_claude_url_helper_accepts_base_or_full_endpoint(self):
        self.assertEqual(
            common._ensure_claude_messages_url('https://api.anthropic.com'),
            'https://api.anthropic.com/v1/messages',
        )
        self.assertEqual(
            common._ensure_claude_messages_url('https://api.anthropic.com/v1'),
            'https://api.anthropic.com/v1/messages',
        )
        self.assertEqual(
            common._ensure_claude_messages_url('https://api.anthropic.com/v1/messages'),
            'https://api.anthropic.com/v1/messages',
        )

    def test_claude_payload_moves_system_prompt_to_top_level(self):
        payload = common._claude_payload(
            [
                {'role': 'system', 'content': 'sys'},
                {'role': 'user', 'content': 'hello'},
                {'role': 'assistant', 'content': 'hi'},
            ],
            'claude-test',
            256,
            0.1,
        )
        self.assertEqual(payload['system'], 'sys')
        self.assertEqual(payload['messages'], [
            {'role': 'user', 'content': 'hello'},
            {'role': 'assistant', 'content': 'hi'},
        ])


if __name__ == '__main__':
    unittest.main()
