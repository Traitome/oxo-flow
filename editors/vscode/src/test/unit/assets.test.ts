import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const extRoot = join(__dirname, "..", "..", "..");
const repoRoot = join(extRoot, "..", "..");

function readJson(rel: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(extRoot, rel), "utf8")) as Record<string, unknown>;
}

/** Compile every TextMate regex to catch Oniguruma-only syntax that JS rejects. */
function compilePatterns(node: unknown, path: string, errors: string[]): void {
  if (Array.isArray(node)) {
    node.forEach((child, i) => compilePatterns(child, `${path}[${i}]`, errors));
    return;
  }
  if (node && typeof node === "object") {
    for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
      if ((key === "match" || key === "begin" || key === "end") && typeof value === "string") {
        try {
          new RegExp(value);
        } catch (e) {
          errors.push(`${path}.${key}: ${value} (${String(e)})`);
        }
      } else {
        compilePatterns(value, `${path}.${key}`, errors);
      }
    }
  }
}

test("TextMate grammar parses and every pattern compiles as a JS regex", () => {
  const grammar = readJson("syntaxes/oxoflow.tmLanguage.json");
  assert.equal(grammar.scopeName, "source.oxoflow");
  const errors: string[] = [];
  compilePatterns(grammar, "grammar", errors);
  assert.deepEqual(errors, []);
});

test("grammar covers the oxo-flow specific scopes", () => {
  const raw = readFileSync(join(extRoot, "syntaxes", "oxoflow.tmLanguage.json"), "utf8");
  for (const scope of [
    "variable.other.oxoflow.placeholder",
    "variable.other.oxoflow.wildcard",
    "entity.name.section.rules.oxoflow",
    "support.type.environment-backend.oxoflow",
  ]) {
    assert.ok(raw.includes(scope), `grammar missing scope ${scope}`);
  }
});

test("snippets and language configuration parse", () => {
  const snippets = readJson("snippets/oxoflow.json");
  assert.ok(Object.keys(snippets).length >= 5);
  for (const [name, snip] of Object.entries(snippets)) {
    const s = snip as { prefix?: unknown; body?: unknown };
    assert.ok(typeof s.prefix === "string", `snippet ${name} missing prefix`);
    assert.ok(Array.isArray(s.body), `snippet ${name} body must be an array`);
  }
  const langCfg = readJson("language-configuration.json");
  assert.ok(langCfg.comments);
  assert.ok(langCfg.brackets);
});

test("package manifest is publishable and in lockstep", () => {
  const pkg = readJson("package.json") as {
    name: string;
    version: string;
    publisher: string;
    engines: { vscode: string };
    devDependencies: Record<string, string>;
  };
  assert.equal(pkg.name, "oxo-flow");
  assert.ok(pkg.publisher.length > 0, "publisher is required by vsce");
  assert.match(pkg.version, /^\d+\.\d+\.\d+$/);
  // @types/vscode must not exceed engines.vscode (vsce packaging rule).
  const types = pkg.devDependencies["@types/vscode"];
  assert.equal(types, "1.85.0");
  assert.ok(pkg.engines.vscode.startsWith("^1.85"));

  // Lockstep with the Rust workspace when this checkout is the repository.
  const cargoPath = join(repoRoot, "Cargo.toml");
  if (existsSync(cargoPath)) {
    const cargo = readFileSync(cargoPath, "utf8");
    const ws = /\[workspace\.package\][\s\S]*?version = "([^"]+)"/.exec(cargo);
    assert.ok(ws, "root Cargo.toml has [workspace.package]");
    assert.equal(pkg.version, ws![1], "extension version must match the workspace version");
  }

  for (const asset of ["icon.png", "LICENSE", "README.md", "CHANGELOG.md", "language-configuration.json"]) {
    assert.ok(existsSync(join(extRoot, asset)), `missing packaging asset ${asset}`);
  }
  const icon = readFileSync(join(extRoot, "icon.png"));
  assert.deepEqual([...icon.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], "icon is a PNG");
});

test("generated completion data is committed and loadable", () => {
  const data = readJson("src/generated/oxoflow-completions.json") as {
    version: number;
    schemaSha256: string;
    contexts: Record<string, unknown>;
  };
  assert.equal(data.version, 1);
  assert.match(data.schemaSha256, /^[0-9a-f]{64}$/);
  assert.ok(Object.keys(data.contexts).length >= 10);
});
