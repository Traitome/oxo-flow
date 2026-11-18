import { buildGenerationPrompt, extractToml } from "./aiBackend";
import { validateArgs } from "./cliArgs";
import { withContentSnapshot } from "./contentSnapshot";
import { runCli, type CliResult } from "./exec";

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
  /** CLI output (stderr, else stdout) for the repair prompt / user report. */
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
    };

const MAX_DETAIL_CHARS = 4000;

/** Build the single repair-round prompt from the failed validation output. */
export function repairPrompt(detail: string): string {
  return `That pipeline failed \`oxo-flow validate\`:\n\n${detail}\n\nReturn the corrected COMPLETE pipeline as one fenced toml block.`;
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
      const output = (res.stderr.trim() || res.stdout.trim()).slice(0, MAX_DETAIL_CHARS);
      if (res.spawnError) {
        return { ok: false, detail: `cannot run ${deps.executable}: ${res.spawnError}` };
      }
      if (res.timedOut) {
        return { ok: false, detail: `oxo-flow validate timed out${output ? `\n${output}` : ""}` };
      }
      return { ok: res.exitCode === 0, detail: output };
    });
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return { ok: false, detail: `cannot stage the draft for validation: ${message}` };
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
  if (!second.ok) {
    return { status: "rejected", attempts: 2, toml: fixed, detail: second.detail };
  }
  await deps.apply(fixed);
  return { status: "applied", toml: fixed, attempts: 2 };
}
