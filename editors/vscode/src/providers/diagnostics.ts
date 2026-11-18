import * as vscode from "vscode";
import { DEFAULT_EXECUTABLE, runCli, type CliResult } from "../core/exec";
import {
  dedupeFindings,
  findingsFromLint,
  findingsFromValidate,
  parseReport,
  type Finding,
} from "../core/jsonReport";
import { indexToml } from "../core/tomlIndex";
import { lintArgs, validateArgs } from "../core/cliArgs";
import { withContentSnapshot } from "../core/contentSnapshot";

const DEBOUNCE_MS = 600;

/**
 * Background diagnostics driven by the CLI: `validate --json` always, plus
 * `lint --json` when enabled. Findings are anchored to the `name = "..."`
 * line of the failing rule when one exists, and deduplicated across the two
 * commands (lint intentionally re-runs the format checks).
 */
export class OxoflowDiagnostics implements vscode.Disposable {
  private readonly collection = vscode.languages.createDiagnosticCollection("oxo-flow");
  private readonly timers = new Map<string, NodeJS.Timeout>();
  private readonly generations = new Map<string, number>();
  private readonly killCurrent = new Map<string, (() => void) | undefined>();
  private readonly subscriptions: vscode.Disposable[] = [this.collection];

  constructor(private readonly output: vscode.OutputChannel) {}

  /** Wire the document/config listeners. */
  activate(context: vscode.ExtensionContext): void {
    this.subscriptions.push(
      vscode.workspace.onDidOpenTextDocument((doc) => this.maybeDiagnose(doc, "open")),
      vscode.workspace.onDidChangeTextDocument((e) => this.maybeDiagnose(e.document, "change")),
      vscode.workspace.onDidSaveTextDocument((doc) => this.maybeDiagnose(doc, "save")),
      vscode.workspace.onDidCloseTextDocument((doc) => {
        this.cancel(doc.uri);
        this.collection.delete(doc.uri);
      }),
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("oxo-flow")) {
          for (const doc of vscode.workspace.textDocuments) this.maybeDiagnose(doc, "config");
        }
      })
    );
    for (const doc of vscode.workspace.textDocuments) this.maybeDiagnose(doc, "open");
    context.subscriptions.push(this);
  }

  /** Entry point used by both the listeners and the Validate/Lint commands. */
  async diagnose(doc: vscode.TextDocument): Promise<void> {
    if (doc.languageId !== "oxoflow" || doc.uri.scheme !== "file") return;
    const uriKey = doc.uri.toString();
    const generation = (this.generations.get(uriKey) ?? 0) + 1;
    this.generations.set(uriKey, generation);
    this.killCurrent.get(uriKey)?.();
    this.killCurrent.set(uriKey, undefined);

    const cfg = vscode.workspace.getConfiguration("oxo-flow", doc.uri);
    const executable = cfg.get<string>("executablePath", DEFAULT_EXECUTABLE);
    const includeLint = cfg.get<boolean>("enableLintDiagnostics", true);
    const trace = cfg.get<boolean>("trace", false);
    const cwd = dirname(doc.uri);
    const text = doc.getText();

    const log = (msg: string) => {
      if (trace) this.output.appendLine(msg);
    };

    const onSpawn = (kill: () => void) => {
      if (this.generations.get(uriKey) === generation) this.killCurrent.set(uriKey, kill);
    };

    /** Run both checks against ONE path (the saved file or a buffer snapshot). */
    const runChecks = async (file: string): Promise<Finding[]> => {
      log(`[diagnostics] ${executable} validate ${file}${includeLint ? " + lint" : ""}`);
      const [validateRes, lintRes] = await Promise.allSettled([
        runCli(executable, validateArgs(file), { cwd, onSpawn }),
        includeLint ? runCli(executable, lintArgs(file), { cwd, onSpawn }) : Promise.resolve(null),
      ]);
      return collectFindings(validateRes, lintRes, executable, log);
    };

    let findings: Finding[];
    if (doc.isDirty) {
      // The editor shows content the saved file does not have. Analyze the
      // BUFFER (staged as a sibling `.oxoflow` snapshot, so relative
      // [[include]] resolution is unchanged); never fall back to the stale
      // disk content. If staging fails, refuse loudly instead.
      try {
        findings = await withContentSnapshot(doc.uri.fsPath, text, (snapshotPath) => {
          log(`[diagnostics] unsaved buffer — analyzing snapshot ${snapshotPath}`);
          return runChecks(snapshotPath);
        });
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        log(`[diagnostics] cannot stage unsaved buffer: ${message}`);
        findings = [saveFirstFinding(message)];
      }
    } else {
      findings = await runChecks(doc.uri.fsPath);
    }

    // A newer run superseded this one — drop the result.
    if (this.generations.get(uriKey) !== generation) return;

    this.collection.set(doc.uri, toDiagnostics(text, dedupeFindings(findings)));
  }

  dispose(): void {
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
    for (const kill of this.killCurrent.values()) kill?.();
    for (const d of this.subscriptions) d.dispose();
  }

  private maybeDiagnose(doc: vscode.TextDocument, trigger: "open" | "change" | "save" | "config"): void {
    if (doc.languageId !== "oxoflow" || doc.uri.scheme !== "file") return;
    const mode = vscode.workspace.getConfiguration("oxo-flow", doc.uri).get<string>("diagnosticMode", "save");
    if (mode === "off") {
      this.collection.delete(doc.uri);
      return;
    }
    const uriKey = doc.uri.toString();
    if (trigger === "change" && mode !== "type") return;
    if (trigger === "save" && mode === "type") return; // already ran while typing
    if (
      trigger === "open" ||
      trigger === "config" ||
      mode === "save" ||
      (trigger === "change" && mode === "type")
    ) {
      // open/config/save run immediately; a change in "type" mode runs
      // debounced below.
      if (mode === "type" && trigger === "change") {
        const prev = this.timers.get(uriKey);
        if (prev) clearTimeout(prev);
        this.timers.set(
          uriKey,
          setTimeout(() => {
            this.timers.delete(uriKey);
            void this.diagnose(doc);
          }, DEBOUNCE_MS)
        );
        return;
      }
      void this.diagnose(doc);
    }
  }

  private cancel(uri: vscode.Uri): void {
    const key = uri.toString();
    const t = this.timers.get(key);
    if (t) clearTimeout(t);
    this.timers.delete(key);
    this.killCurrent.get(key)?.();
    this.generations.delete(key);
  }
}

function dirname(uri: vscode.Uri): string {
  return uri.with({ path: uri.path.replace(/\/[^/]*$/, "") }).fsPath;
}

/** Turn the two CLI results into findings (shared by file and snapshot runs). */
function collectFindings(
  validateRes: PromiseSettledResult<CliResult>,
  lintRes: PromiseSettledResult<CliResult | null>,
  executable: string,
  log: (msg: string) => void
): Finding[] {
  const findings: Finding[] = [];
  if (validateRes.status === "fulfilled") {
    const res = validateRes.value;
    if (res.spawnError) {
      log(`[diagnostics] cannot run ${executable}: ${res.spawnError}`);
    } else {
      try {
        const report = parseReport(res.stdout);
        if (report.command === "validate") findings.push(...findingsFromValidate(report));
      } catch (e) {
        log(`[diagnostics] validate parse failure: ${e instanceof Error ? e.message : String(e)}`);
        findings.push(unparseable("validate", res));
      }
    }
  } else {
    log(`[diagnostics] validate failed: ${String(validateRes.reason)}`);
    findings.push(unparseable("validate", null));
  }

  if (lintRes.status === "fulfilled" && lintRes.value) {
    const res = lintRes.value;
    if (res.spawnError) {
      log(`[diagnostics] cannot run ${executable} (lint): ${res.spawnError}`);
    } else {
      try {
        const report = parseReport(res.stdout);
        if (report.command === "lint") findings.push(...findingsFromLint(report));
      } catch (e) {
        log(`[diagnostics] lint parse failure: ${e instanceof Error ? e.message : String(e)}`);
        findings.push(unparseable("lint", res));
      }
    }
  }
  return findings;
}

/**
 * Refusal diagnostic for an unsaved buffer that could not be staged. It is
 * deliberately NOT a stale result: nothing is read from the saved file.
 */
function saveFirstFinding(reason: string): Finding {
  return {
    severity: "warning",
    code: "UNSAVED",
    message: `Unsaved changes — oxo-flow could not analyze the in-editor buffer (${reason}) and did NOT fall back to the saved file. Save the document (Cmd/Ctrl+S) to refresh diagnostics against what you see.`,
    rule: null,
    suggestion: "Save the document, then re-run validation.",
    source: "validate",
  };
}

/** Human-output fallback when the JSON document cannot be recovered. */
function unparseable(command: string, res: { stderr: string; exitCode: number | null } | null): Finding {
  const detail = res?.stderr.split("\n").filter(Boolean).slice(-3).join(" ") ?? "CLI invocation failed";
  return {
    severity: "error",
    code: "CLI_OUTPUT",
    message: `could not parse oxo-flow ${command} output (exit ${res?.exitCode ?? "?"}): ${detail}`,
    rule: null,
    suggestion: `Run \`oxo-flow ${command} --json\` in a terminal to see the raw output; also check oxo-flow.executablePath.`,
    source: "validate",
  };
}

function toDiagnostics(text: string, findings: Finding[]): vscode.Diagnostic[] {
  const index = indexToml(text);
  const docLines = text.split(/\r\n|\n|\r/);
  return findings.map((f) => {
    let line = 0;
    if (f.rule) {
      const ruleLine = index.lineOfRuleName(f.rule);
      if (ruleLine !== null) line = ruleLine;
    }
    const range = new vscode.Range(line, 0, line, Math.max(0, (docLines[line] ?? "").length));
    const message = f.suggestion ? `${f.code}: ${f.message}\n\n${f.suggestion}` : `${f.code}: ${f.message}`;
    const diag = new vscode.Diagnostic(range, message, toSeverity(f.severity));
    diag.source = "oxo-flow";
    diag.code = f.code;
    return diag;
  });
}

function toSeverity(s: Finding["severity"]): vscode.DiagnosticSeverity {
  switch (s) {
    case "error":
      return vscode.DiagnosticSeverity.Error;
    case "warning":
      return vscode.DiagnosticSeverity.Warning;
    default:
      return vscode.DiagnosticSeverity.Information;
  }
}
