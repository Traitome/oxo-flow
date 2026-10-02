import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Registry, type IGrammar, type IToken, parseRawGrammar } from "vscode-textmate";
import { createOnigScanner, createOnigString, loadWASM } from "vscode-oniguruma";

const extRoot = join(__dirname, "..", "..", "..");

/**
 * Behavioral grammar test: tokenize a representative pipeline with the real
 * TextMate engine (vscode-textmate + oniguruma, the same machinery VS Code
 * uses) and assert the oxo-flow specific scopes light up where a user would
 * expect them. Catches scope bugs that a JSON well-formedness check cannot.
 *
 * tokenizeLine is line-based: positions are (line, column) pairs.
 */

interface PositionedToken {
  line: number;
  token: IToken;
}

async function loadGrammar(): Promise<IGrammar> {
  const wasm = readFileSync(
    join(extRoot, "node_modules", "vscode-oniguruma", "release", "onig.wasm")
  );
  await loadWASM(wasm.buffer as ArrayBuffer);
  const grammarText = readFileSync(
    join(extRoot, "syntaxes", "oxoflow.tmLanguage.json"),
    "utf8"
  );
  const registry = new Registry({
    onigLib: Promise.resolve({ createOnigScanner, createOnigString }),
    loadGrammar: async (scopeName) =>
      scopeName === "source.oxoflow"
        ? parseRawGrammar(grammarText, "oxoflow.tmLanguage.json")
        : null,
  });
  const grammar = await registry.loadGrammar("source.oxoflow");
  assert.ok(grammar, "grammar failed to load");
  return grammar!;
}

function tokenize(grammar: IGrammar, text: string): PositionedToken[] {
  const positioned: PositionedToken[] = [];
  let ruleStack = null;
  const lines = text.split("\n");
  for (let line = 0; line < lines.length; line++) {
    const result = grammar.tokenizeLine(lines[line], ruleStack);
    for (const token of result.tokens) positioned.push({ line, token });
    ruleStack = result.ruleStack;
  }
  return positioned;
}

/** Deepest scope at (line, column), or "". */
function scopeAt(tokens: PositionedToken[], text: string, needle: string): string {
  const lines = text.split("\n");
  const line = lines.findIndex((l) => l.includes(needle));
  assert.ok(line >= 0, `fixture line containing ${needle} not found`);
  const col = lines[line].indexOf(needle);
  for (const { line: l, token } of tokens) {
    if (l === line && token.startIndex <= col && col < token.endIndex) {
      return token.scopes[token.scopes.length - 1] ?? "";
    }
  }
  return "";
}

const SAMPLE = [
  "# pipeline comment",
  "[workflow]",
  'name = "demo"',
  "",
  "[[rules]]",
  'name = "fastqc"',
  'input = ["data/{sample}.fastq.gz"]',
  "threads = 8",
  'memory = "4GB"',
  "checkpoint = true",
  'environment = { conda = "envs/fastp.yaml" }',
  'shell = """',
  "fastp -i {input[0]} {params.extra} > {output[0]}",
  '"""',
].join("\n");

test("grammar tokenizes comments, tables, keys and values", async () => {
  const grammar = await loadGrammar();
  const tokens = tokenize(grammar, SAMPLE);

  assert.equal(
    scopeAt(tokens, SAMPLE, "# pipeline comment"),
    "comment.line.number-sign.oxoflow",
    "comment"
  );
  assert.equal(
    scopeAt(tokens, SAMPLE, "workflow"),
    "entity.name.section.oxoflow",
    "table header name"
  );
  assert.equal(
    scopeAt(tokens, SAMPLE, "rules"),
    "entity.name.section.rules.oxoflow",
    "array-of-tables header"
  );
  // Rule `name` keys get their own emphasis; the value is a plain string.
  assert.equal(scopeAt(tokens, SAMPLE, 'name = "fastqc"'.slice(0, 4)), "keyword.other.name.oxoflow", "rule name key");
  assert.match(
    scopeAt(tokens, SAMPLE, "8"),
    /^constant\.numeric/,
    "threads = 8 value"
  );
  assert.match(
    scopeAt(tokens, SAMPLE, "true"),
    /^constant\.language\.boolean/,
    "checkpoint = true"
  );
  assert.equal(
    scopeAt(tokens, SAMPLE, "{sample}"),
    "variable.other.oxoflow.wildcard",
    "{sample} wildcard"
  );
  assert.equal(
    scopeAt(tokens, SAMPLE, "{input[0]}"),
    "variable.other.oxoflow.placeholder",
    "{input[0]} placeholder"
  );
  assert.equal(
    scopeAt(tokens, SAMPLE, "conda"),
    "support.type.environment-backend.oxoflow",
    "conda backend key in inline table"
  );
  assert.match(
    scopeAt(tokens, SAMPLE, "fastp -i"),
    /^string\./,
    "shell script body is a string"
  );
});
