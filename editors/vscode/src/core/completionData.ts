/**
 * Schema-derived completion/hover data (see scripts/generate-completions.mjs).
 * Pure data access, no vscode imports, so tests can assert integrity without
 * an editor host.
 */
import data from "../generated/oxoflow-completions.json";

export interface PropInfo {
  key: string;
  description?: string;
  type?: string;
  /** Sorted allowed values when the schema constrains them. */
  enums?: string[];
  itemsType?: string;
  ref?: string;
}

export interface ContextData {
  keys: PropInfo[];
}

export interface CompletionData {
  version: number;
  source: string;
  schemaSha256: string;
  contexts: Record<string, ContextData>;
}

export const completionData = data as CompletionData;

export function contextNames(): string[] {
  return Object.keys(completionData.contexts).sort();
}

export function contextKeys(name: string): PropInfo[] {
  return completionData.contexts[name]?.keys ?? [];
}

export function propInfoFor(context: string, key: string): PropInfo | undefined {
  return contextKeys(context).find((k) => k.key === key);
}

export function enumFor(context: string, key: string): string[] | null {
  const info = propInfoFor(context, key);
  return info?.enums && info.enums.length > 0 ? info.enums : null;
}

/**
 * TOML type of a property for insertion templates: tables become `[name]`,
 * arrays of tables become `[[name]]`, values become `key = <placeholder>`.
 */
export function insertionFor(info: PropInfo): string {
  if (info.key === "rules") return "[[rules]]";
  if (info.type === "object" || info.ref) return `[${info.key}]`;
  if (info.type === "array") return `${info.key} = [\n\t"$1"\n]`;
  if (info.type === "boolean") return `${info.key} = ${info.enums?.length === 2 ? "false" : "false"}`;
  if (info.type === "number" || info.type === "integer") return `${info.key} = $1`;
  return `${info.key} = "$1"`;
}
