import { defineConfig } from "@vscode/test-cli";

// Pinned test host: VS Code >= ~1.13x renamed the macOS bundle executable
// from `Electron` to `Code`, while @vscode/test-electron 2.5.x still spawns
// `.../MacOS/Electron` (ENOENT on launch). Pin the newest host whose layout
// matches the runner; lift the pin when vscode-test catches up.
export default defineConfig({
  version: "1.99.3",
  files: "out/test/integration/**/*.test.js",
  extensionDevelopmentPath: import.meta.dirname,
  // The tests update workspace-scoped settings and rely on a workspace for
  // pipeline lookups — open the extension folder itself.
  workspaceFolder: import.meta.dirname,
  // The runner's mocha defaults to the TDD UI (suite/test); the tests are
  // written BDD-style (describe/it) and rely on the globals injected by this
  // very mocha instance — importing our own copy would break suite binding.
  mocha: { ui: "bdd", timeout: 60_000 },
  // Pass OXOFLOW_TEST_BIN and friends through to the test host.
  extensionTestsEnv: { ...process.env },
});
