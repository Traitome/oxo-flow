/**
 * Pure builders for `oxo-flow` CLI argument vectors.
 *
 * Callers execute these with `child_process.execFile` / `ShellExecution`, so
 * no shell quoting happens here — arguments are passed verbatim as one
 * element each. Keeping this logic in a vscode-free module makes it directly
 * unit-testable.
 */

export interface RunOptions {
  file: string;
  jobs?: number;
  keepGoing?: boolean;
  targets?: string[];
  extraArgs?: string[];
}

/** Global flags every subcommand accepts (clap `global = true`). */
function jsonFlag(): string[] {
  return ["--json"];
}

export function validateArgs(file: string, opts?: { asInclude?: boolean; json?: boolean }): string[] {
  const args = ["validate", file];
  if (opts?.asInclude) args.push("--as-include");
  if (opts?.json !== false) args.push(...jsonFlag());
  return args;
}

export function lintArgs(file: string, opts?: { strict?: boolean; json?: boolean }): string[] {
  const args = ["lint", file];
  if (opts?.strict) args.push("--strict");
  if (opts?.json !== false) args.push(...jsonFlag());
  return args;
}

export function runArgs(opts: RunOptions): string[] {
  const args = ["run", opts.file];
  if (opts.jobs !== undefined) args.push("-j", String(opts.jobs));
  if (opts.keepGoing) args.push("-k");
  for (const t of opts.targets ?? []) args.push("-t", t);
  args.push(...(opts.extraArgs ?? []));
  return args;
}

export function dryRunArgs(opts: RunOptions): string[] {
  // Dry-run shares the run surface; the terminal view wants human output.
  const args = ["dry-run", opts.file];
  if (opts.jobs !== undefined) args.push("-j", String(opts.jobs));
  for (const t of opts.targets ?? []) args.push("-t", t);
  args.push(...(opts.extraArgs ?? []));
  return args;
}

/** Formats accepted by `oxo-flow graph -f` (clap `GraphFormat`). */
export type GraphFormat = "ascii" | "dot" | "dot-clustered" | "tree" | "mermaid" | "metro";

export function graphArgs(file: string, format?: GraphFormat): string[] {
  const args = ["graph", file];
  if (format && format !== "ascii") args.push("-f", format);
  return args;
}

export function resumeArgs(checkpoint: string, extraArgs?: string[]): string[] {
  return ["resume", checkpoint, ...(extraArgs ?? [])];
}

/** `oxo-flow clean <workflow> [-n|--force] [--orphans] [-d workdir]` */
export function cleanArgs(
  file: string,
  opts?: { dryRun?: boolean; force?: boolean; orphans?: boolean }
): string[] {
  const args = ["clean", file];
  if (opts?.dryRun) args.push("-n");
  if (opts?.force) args.push("--force");
  if (opts?.orphans) args.push("--orphans");
  return args;
}

/** `oxo-flow status [checkpoint] [--timing]` */
export function statusArgs(checkpoint?: string, opts?: { timing?: boolean }): string[] {
  const args = checkpoint ? ["status", checkpoint] : ["status"];
  if (opts?.timing) args.push("--timing");
  return args;
}

export function templateArgs(description: string, output?: string): string[] {
  const args = ["template", description, "--ai"];
  if (output) args.push("-o", output);
  return args;
}

export function formatArgs(file: string, output?: string): string[] {
  const args = ["format", file];
  if (output) args.push("-o", output);
  return args;
}

export function schemaArgs(): string[] {
  return ["schema"];
}

export function aiStatusArgs(): string[] {
  return ["ai", "--json"];
}

export function versionArgs(): string[] {
  return ["--version"];
}
