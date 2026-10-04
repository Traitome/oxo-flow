import * as os from "node:os";

/** Everything the report embeds about the running environment. */
export interface ReportEnvironment {
  readonly extensionVersion: string;
  readonly vscodeVersion: string;
  readonly appName: string;
  readonly appHost: string;
  readonly remoteName: string | undefined;
  readonly platform: string;
  readonly arch: string;
  readonly cliExecutable: string;
  /** `undefined` when the CLI probe failed (the binary may be missing). */
  readonly cliVersion: string | undefined;
  readonly settings: Record<string, unknown>;
}

/** Optional failure context captured at the moment the user hit a problem. */
export interface ReportContext {
  readonly lastError?: string;
  readonly outputTail?: string;
}

const MAX_TAIL_CHARS = 4000;
export const ISSUE_NEW_URL = "https://github.com/Traitome/oxo-flow/issues/new";
/** GitHub silently drops very long query strings; stay well under the limit. */
const MAX_URL_CHARS = 8000;

/** Replace every occurrence of the user's home directory with `~`. */
export function maskHomeDirs(text: string, home: string = os.homedir()): string {
  if (!home) return text;
  return text.split(home).join("~");
}

/** Keep the last `maxChars` characters — log tails matter most at the end. */
export function truncateTail(text: string, maxChars: number = MAX_TAIL_CHARS): string {
  const trimmed = text.trimEnd();
  if (trimmed.length <= maxChars) return trimmed;
  return `…(truncated)\n${trimmed.slice(-maxChars)}`;
}

/**
 * Render the markdown issue body. Pure (no VS Code imports) so the masking
 * and section layout stay unit-tested; `home` is injectable for tests.
 */
export function buildIssueBody(
  env: ReportEnvironment,
  context: ReportContext = {},
  home: string = os.homedir()
): string {
  const settings = Object.entries(env.settings)
    .map(([key, value]) => `- \`${key}\`: \`${maskHomeDirs(JSON.stringify(value), home)}\``)
    .join("\n");
  const lines = [
    "### Environment",
    "",
    `- Extension: oxo-flow v${env.extensionVersion}`,
    `- VS Code: ${env.vscodeVersion} — ${env.appName} (host: ${env.appHost})`,
    `- Platform: ${env.platform}/${env.arch}${env.remoteName ? ` — remote: ${env.remoteName}` : ""}`,
    `- CLI: \`${maskHomeDirs(env.cliExecutable, home)}\`${env.cliVersion ? ` — v${env.cliVersion}` : " — not runnable"}`,
    "",
    "### Settings",
    "",
    settings || "- (all defaults)",
  ];
  if (context.lastError) {
    lines.push("", "### Error at report time", "", "```", maskHomeDirs(context.lastError, home), "```");
  }
  if (context.outputTail) {
    lines.push("", "### Output channel tail", "", "```", maskHomeDirs(truncateTail(context.outputTail), home), "```");
  }
  lines.push(
    "",
    "### Steps to reproduce",
    "",
    "1. ",
    "",
    "**Expected behavior:** ",
    "",
    "**Actual behavior:** ",
    "",
    "<!-- Prefilled by `oxo-flow: Report Issue`. Please review everything above before submitting — it contains machine and path details. -->"
  );
  return lines.join("\n");
}

/**
 * Issue title + body as a ready-to-open GitHub URL. Returns null when the
 * body is too long for a query string (callers fall back to the clipboard).
 *
 * The result must be handed to `env.openExternal` as a plain string — it is
 * already fully encoded, and `Uri.parse` would re-decode the query before the
 * open pipeline re-encodes it (corrupting `###` to `%23%23%23` and cutting
 * the value at the first `&`).
 */
export function buildIssueUrl(body: string, title = "[vscode] "): string | null {
  const url = `${ISSUE_NEW_URL}?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
  return url.length > MAX_URL_CHARS ? null : url;
}
