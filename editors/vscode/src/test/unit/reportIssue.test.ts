import test from "node:test";
import assert from "node:assert/strict";
import {
  buildIssueBody,
  buildIssueUrl,
  maskHomeDirs,
  truncateTail,
} from "../../core/reportIssue";

const HOME = "/Users/demo";

const ENV = {
  extensionVersion: "0.22.0",
  vscodeVersion: "1.99.0",
  appName: "Visual Studio Code",
  appHost: "desktop",
  remoteName: "ssh-remote",
  platform: "darwin",
  arch: "arm64",
  cliExecutable: "/Users/demo/.cargo/bin/oxo-flow",
  cliVersion: "0.22.0",
  settings: { "oxo-flow.executablePath": "/Users/demo/.cargo/bin/oxo-flow" },
};

test("maskHomeDirs replaces every home occurrence with ~", () => {
  assert.equal(maskHomeDirs("see /Users/demo/x and /Users/demo/y", HOME), "see ~/x and ~/y");
  assert.equal(maskHomeDirs("no match here", HOME), "no match here");
});

test("truncateTail keeps the tail and marks the cut", () => {
  const short = "line1\nline2";
  assert.equal(truncateTail(short, 100), short);
  const long = "a".repeat(50) + "TAIL";
  const cut = truncateTail(long, 10);
  assert.ok(cut.startsWith("…(truncated)"), cut);
  assert.ok(cut.endsWith("TAIL"));
});

test("buildIssueBody embeds environment, masks paths, and marks the tail", () => {
  const body = buildIssueBody(
    ENV,
    {
      lastError: "Cannot run /Users/demo/.cargo/bin/oxo-flow (ENOENT).",
      outputTail: "$ oxo-flow validate\nE007: boom\n" + "x".repeat(5000),
    },
    HOME
  );
  assert.ok(body.includes("oxo-flow v0.22.0"), body);
  assert.ok(body.includes("ssh-remote"));
  assert.ok(body.includes("`~/.cargo/bin/oxo-flow`"), "home must be masked");
  assert.ok(!body.includes("/Users/demo"), "raw home path must never survive");
  assert.ok(body.includes("### Error at report time"));
  assert.ok(body.includes("…(truncated)"));
  assert.ok(body.includes("### Steps to reproduce"));
});

test("buildIssueBody omits empty sections and lists settings", () => {
  const body = buildIssueBody({ ...ENV, remoteName: undefined }, {}, HOME);
  assert.ok(body.includes("`oxo-flow.executablePath`"), body);
  assert.ok(body.includes('"~/.cargo/bin/oxo-flow"'), "setting values must be masked too");
  assert.ok(!body.includes("Error at report time"));
  assert.ok(!body.includes("Output channel tail"));
});

test("buildIssueUrl encodes newlines and signals oversize with null", () => {
  const url = buildIssueUrl("line1\nline2");
  assert.ok(url!.startsWith("https://github.com/Traitome/oxo-flow/issues/new?title="));
  assert.ok(url!.includes("line1%0Aline2"));
  assert.equal(buildIssueUrl("x".repeat(9000)), null);
});
