import { buildGenerationPrompt, extractToml } from "./aiBackend";
import { validateArgs } from "./cliArgs";
import { withContentSnapshot } from "./contentSnapshot";
import { runCli, type CliResult } from "./exec";
import { findingsFromValidate, parseReport, type Finding } from "./jsonReport";

/**
 * Validate-before-apply guard for the editor-model AI generation path.
 *
 * The pipeline returned by the language model is NEVER written to the chosen
 * destination first: it is staged as a sibling `.oxoflow` snapshot, judged by
 * `oxo-flow validate` (the same CLI-backed path diagnostics use), and only a
 * PASSING draft reaches `apply`. One repair round is allowed; a repaired
 * draft is re-validated under the same rule. A draft that still fails is
 * surfaced to the caller with the CLI findings and is not applied, so an
 * invalid generation can never land on disk (or overwrite an existing file).
 *
 * Infrastructure failures (missing binary, timeout, staging I/O) are NOT
 * validation failures: they are reported as `unavailable`, name the real
 * cause, and do not burn the repair round.
 */

export interface DraftChatTurn {
  readonly role: "user" | "assistant";
  readonly text: string;
}

/** CLI invoker seam so the guard's mechanics are unit-testable. */
export type DraftRunner = (
  executable: string,
  args: string[],
  opts: { cwd?: string; timeoutMs?: number }
) => Promise<CliResult>;

export interface DraftGenerationDeps {
  /** Destination path the draft would occupy; anchors the validation snapshot. */
  readonly targetPath: string;
  readonly executable: string;
  readonly cwd: string;
  /** One model round-trip over the supplied conversation. */
  readonly chat: (turns: readonly DraftChatTurn[]) => Promise<string>;
  /** Receives the pipeline ONLY after validation passes. */
  readonly apply: (toml: string) => Promise<void>;
  readonly timeoutMs?: number;
  readonly log?: (line: string) => void;
  /** Test seam; defaults to the real `runCli`. */
  readonly run?: DraftRunner;
}

export interface DraftVerdict {
  readonly ok: boolean;
  /** The CLI could not judge the draft at all (spawn/timeout/staging), so the
   * draft is neither valid nor invalid — never consume the repair round. */
  readonly unavailable: boolean;
  /** Structured findings (code/rule/message/hint) for the repair prompt and
   * the user report; raw CLI output when the report cannot be parsed. */
  readonly detail: string;
}

export type DraftOutcome =
  | { readonly status: "applied"; readonly toml: string; readonly attempts: 1 | 2 }
  | { readonly status: "no-pipeline"; readonly response: string }
  | {
      readonly status: "rejected";
      readonly attempts: 1 | 2;
      /** Last complete candidate rejected by validation (never written). */
      readonly toml: string;
      readonly detail: string;
    }
  | {
      readonly status: "unavailable";
      readonly attempts: 1 | 2;
      /** The real infrastructure cause (cannot run, timed out, staging I/O). */
      readonly detail: string;
    };

const MAX_DETAIL_CHARS = 4000;

/** Build the single repair-round prompt from the failed validation output. */
export function repairPrompt(detail: string): string {
  return `That pipeline failed \`oxo-flow validate\`:\n\n${detail}\n\nReturn the corrected COMPLETE pipeline as one fenced toml block.`;
}

/** Render findings so the model (and the user report) sees code, rule,
 * message and hint — not the CLI's stderr bail summary. */
export function formatFindings(findings: readonly Finding[]): string {
  return findings
    .map((f) => {
      const rule = f.rule ? ` (rule: ${f.rule})` : "";
      const hint = f.suggestion ? `\n  hint: ${f.suggestion}` : "";
      return `${f.code}: ${f.message}${rule}${hint}`;
    })
    .join("\n")
    .slice(0, MAX_DETAIL_CHARS);
}

/**
 * Extract actionable findings from `validate --json` stdout. `--json` sends
 * findings to stdout while stderr only carries a bail summary, so preferring
 * stderr would hand the model nothing to fix. Returns null when stdout is not
 * a usable validate report.
 */
function structuredFindings(res: CliResult): string | null {
  try {
    const report = parseReport(res.stdout);
    if (report.command !== "validate") return null;
    const findings = findingsFromValidate(report);
    return findings.length > 0 ? formatFindings(findings) : null;
  } catch {
    return null;
  }
}

/**
 * Stage `toml` beside `targetPath` and run `oxo-flow validate` on it. The
 * destination itself is never written here.
 */
export async function validateDraftContent(
  deps: Pick<DraftGenerationDeps, "targetPath" | "executable" | "cwd" | "timeoutMs" | "run" | "log">,
  toml: string
): Promise<DraftVerdict> {
  const run = deps.run ?? runCli;
  try {
    return await withContentSnapshot(deps.targetPath, toml, async (snapshotPath) => {
      deps.log?.(`[ai:ide] validating draft snapshot ${snapshotPath}`);
      const res = await run(deps.executable, validateArgs(snapshotPath), {
        cwd: deps.cwd,
        timeoutMs: deps.timeoutMs,
      });
      if (res.spawnError) {
        return {
          ok: false,
          unavailable: true,
          detail: `cannot run ${deps.executable}: ${res.spawnError}`,
        };
      }
      if (res.timedOut) {
        return { ok: false, unavailable: true, detail: "oxo-flow validate timed out" };
      }
      if (res.exitCode === 0) return { ok: true, unavailable: false, detail: "" };
      const raw = (res.stderr.trim() || res.stdout.trim()).slice(0, MAX_DETAIL_CHARS);
      return { ok: false, unavailable: false, detail: structuredFindings(res) ?? raw };
    });
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return {
      ok: false,
      unavailable: true,
      detail: `cannot stage the draft for validation: ${message}`,
    };
  }
}

/**
 * Generate, validate, optionally repair once, re-validate, and apply only a
 * passing pipeline. See the module comment for the ordering guarantee.
 */
export async function generateValidatedDraft(
  description: string,
  deps: DraftGenerationDeps
): Promise<DraftOutcome> {
  const turns: DraftChatTurn[] = [{ role: "user", text: buildGenerationPrompt(description) }];
  const response = await deps.chat([...turns]);
  const draft = extractToml(response);
  if (!draft) return { status: "no-pipeline", response };

  const first = await validateDraftContent(deps, draft);
  if (first.unavailable) {
    return { status: "unavailable", attempts: 1, detail: first.detail };
  }
  if (first.ok) {
    await deps.apply(draft);
    return { status: "applied", toml: draft, attempts: 1 };
  }

  deps.log?.("[ai:ide] draft failed validation — one repair round");
  turns.push(
    { role: "assistant", text: response },
    { role: "user", text: repairPrompt(first.detail) }
  );
  const repaired = await deps.chat([...turns]);
  const fixed = extractToml(repaired);
  if (!fixed) {
    return {
      status: "rejected",
      attempts: 2,
      toml: draft,
      detail: `${first.detail}\n\nThe repair round returned no pipeline.`,
    };
  }

  const second = await validateDraftContent(deps, fixed);
  if (second.unavailable) {
    return { status: "unavailable", attempts: 2, detail: second.detail };
  }
  if (!second.ok) {
    return { status: "rejected", attempts: 2, toml: fixed, detail: second.detail };
  }
  await deps.apply(fixed);
  return { status: "applied", toml: fixed, attempts: 2 };
}
