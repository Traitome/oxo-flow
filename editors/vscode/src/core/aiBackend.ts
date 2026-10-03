import { completionData } from "./completionData";

export type AiBackend = "cli" | "ide";

export interface BackendProbe {
  /** `oxo-flow ai` reports a usable provider (the CLI generation path works). */
  readonly cliAiConfigured: boolean;
  /** `vscode.lm.selectChatModels` exists and returned at least one model. */
  readonly ideModelsAvailable: boolean;
}

export type BackendResolution =
  | { readonly kind: "ok"; readonly backend: AiBackend; readonly reason: string }
  | { readonly kind: "unavailable"; readonly reason: string };

/**
 * Priority chain for `oxo-flow.ai.backend = "auto"`: the CLI engine path
 * first (generation is paired with schema-validated repair rounds and is the
 * eval-tuned pipeline), then the editor language-model API (zero-config when
 * a Copilot-style model is signed in), then a loud failure — never a silent
 * no-op.
 */
export function resolveBackend(
  configured: "auto" | "cli" | "ide",
  probe: BackendProbe
): BackendResolution {
  if (configured === "cli") {
    return probe.cliAiConfigured
      ? { kind: "ok", backend: "cli", reason: "CLI AI provider configured" }
      : {
          kind: "unavailable",
          reason:
            "AI backend is forced to 'cli' but no CLI provider is configured. Run `oxo-flow ai` to inspect; set OXO_FLOW_AI_PROVIDER or ~/.oxo-flow/ai_config.json.",
        };
  }
  if (configured === "ide") {
    return probe.ideModelsAvailable
      ? { kind: "ok", backend: "ide", reason: "editor language model" }
      : {
          kind: "unavailable",
          reason:
            "AI backend is forced to 'ide' but this editor exposes no language models. GitHub Copilot signed into stock VS Code provides them; VSCodium does not.",
        };
  }
  if (probe.cliAiConfigured) {
    return { kind: "ok", backend: "cli", reason: "auto: CLI AI provider configured" };
  }
  if (probe.ideModelsAvailable) {
    return { kind: "ok", backend: "ide", reason: "auto: CLI AI not configured — using the editor language model" };
  }
  return {
    kind: "unavailable",
    reason:
      "No AI backend available. Configure the CLI provider (`oxo-flow ai`; OXO_FLOW_AI_PROVIDER or ~/.oxo-flow/ai_config.json) or sign into an editor language model (GitHub Copilot in stock VS Code).",
  };
}

/**
 * Extract the TOML pipeline from a model response: a fenced ```toml block
 * wins; otherwise the raw text must carry the mandatory `[workflow]` table
 * (prose before it is stripped). Returns null when no pipeline is present.
 */
export function extractToml(response: string): string | null {
  const fenced = response.match(/```(?:toml)?\s*\n([\s\S]*?)```/i);
  const candidate = (fenced ? fenced[1] : response).trim();
  if (!candidate) return null;
  const start = candidate.search(/^\s*\[workflow\]/m);
  const toml = start > 0 ? candidate.slice(start) : candidate;
  return /^\s*\[workflow\]/m.test(toml) ? toml.trim() : null;
}

/**
 * Compact schema digest for the IDE generation prompt: key names, types and
 * enums per context, derived from the same generated artifact the completion
 * provider uses — it cannot drift from the engine schema.
 */
export function schemaDigest(): string {
  const contexts = completionData.contexts;
  return Object.keys(contexts)
    .sort()
    .map((name) => {
      const label = name === "root" ? "top level" : name;
      const keys = contexts[name].keys
        .map(
          (k) =>
            `  - ${k.key} (${k.type ?? "value"})${k.enums ? ` one of: ${k.enums.join("|")}` : ""}`
        )
        .join("\n");
      return `[${label}]\n${keys}`;
    })
    .join("\n\n");
}

export function buildGenerationPrompt(description: string): string {
  return [
    "You are generating an oxo-flow pipeline file (TOML dialect, schema oxoflow-v1).",
    `Pipeline request: ${description}`,
    "",
    "Rules:",
    "- Reply with ONE fenced ```toml block containing the complete pipeline and no prose.",
    "- Always include the [workflow] table with name and description.",
    "- Every [[rules]] entry needs a name plus input and/or output; shell commands use {input}/{output} placeholders.",
    "- Order rules with depends_on; declare environments in [env_groups.*] and reference them from rules.",
    "",
    "Schema digest (keys you may use):",
    "",
    schemaDigest(),
  ].join("\n");
}
