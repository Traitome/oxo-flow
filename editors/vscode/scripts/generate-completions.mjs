#!/usr/bin/env node
// Derives the extension's completion/hover data from the canonical workflow
// JSON Schema (docs/schema/oxoflow-v1.schema.json). CI enforces
// `make schema-drift` (CLI-embedded schema == docs copy), so the docs copy is
// the authoritative source — this generator consumes it instead of hand-copying
// keys, and the committed output is drift-gated via `npm run generate:check`.
//
//   node scripts/generate-completions.mjs          rewrite src/generated/oxoflow-completions.json
//   node scripts/generate-completions.mjs --check  fail if the committed file is stale
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const extDir = dirname(scriptDir);
const schemaPath = join(extDir, "..", "..", "docs", "schema", "oxoflow-v1.schema.json");
const outPath = join(extDir, "src", "generated", "oxoflow-completions.json");

const schemaBytes = readFileSync(schemaPath);
const schema = JSON.parse(schemaBytes.toString("utf8"));
const schemaSha256 = createHash("sha256").update(schemaBytes).digest("hex");

function resolveRef(node) {
  if (node && typeof node === "object" && typeof node.$ref === "string") {
    if (!node.$ref.startsWith("#/")) {
      throw new Error(`unsupported external $ref: ${node.$ref}`);
    }
    let cur = schema;
    for (const part of node.$ref.slice(2).split("/")) {
      cur = cur?.[part.replaceAll("~1", "/").replaceAll("~0", "~")];
    }
    if (cur === undefined) throw new Error(`unresolvable $ref: ${node.$ref}`);
    return resolveRef(cur);
  }
  return node;
}

// Extract the completion-relevant view of a schema property.
function propInfo(key, rawProp) {
  const prop = resolveRef(rawProp);
  const info = { key, description: prop.description ?? "" };
  if (typeof prop.type === "string") info.type = prop.type;
  else if (prop.type === "array" && prop.items) {
    const item = resolveRef(prop.items);
    info.type = "array";
    if (item.enum) info.enums = [...item.enum].sort();
    else if (typeof item.type === "string") info.itemsType = item.type;
  } else if (rawProp && typeof rawProp === "object" && rawProp.$ref) {
    info.type = info.type ?? "object";
  }
  if (prop.enum) {
    info.enums = [...prop.enum].sort();
    delete info.itemsType;
  }
  if (prop.$ref === undefined && rawProp && typeof rawProp === "object" && rawProp.$ref) {
    info.ref = rawProp.$ref;
  }
  return info;
}

function contextFromProperties(props) {
  const keys = Object.keys(props)
    .sort()
    .map((k) => propInfo(k, props[k]));
  if (keys.length === 0) return null;
  return { keys };
}

const contexts = {};

// 1. The document root: every top-level table / value.
contexts.root = contextFromProperties(schema.properties);

// 2. Each top-level table with concrete properties becomes its own context
//    ([workflow], [cluster], [ai], [config.*], ...).
for (const [key, rawProp] of Object.entries(schema.properties)) {
  const prop = resolveRef(rawProp);
  if (prop && typeof prop === "object" && prop.properties) {
    const ctx = contextFromProperties(prop.properties);
    if (ctx) contexts[key] = ctx;
  }
}

// 3. Rules: the $defs/rule table drives [[rules]] completion.
const ruleDef = resolveRef(schema.properties.rules?.items ?? { $ref: "#/$defs/rule" });
if (ruleDef?.properties) contexts.rules = contextFromProperties(ruleDef.properties);

// 4. Rule sub-objects ($ref'ed or inline) become `rules.<prop>` contexts, so
//    `environment = { ... }` and friends complete their inner keys.
for (const [key, rawProp] of Object.entries(ruleDef?.properties ?? {})) {
  let sub = null;
  if (rawProp && typeof rawProp === "object" && typeof rawProp.$ref === "string") {
    sub = resolveRef(rawProp);
  } else if (rawProp && typeof rawProp === "object" && rawProp.type === "object" && rawProp.properties) {
    sub = rawProp;
  } else if (rawProp && typeof rawProp === "object" && rawProp.type === "array" && rawProp.items) {
    const item = resolveRef(rawProp.items);
    if (item?.properties) sub = item;
  }
  if (sub?.properties) {
    const ctx = contextFromProperties(sub.properties);
    if (ctx) contexts[`rules.${key}`] = ctx;
  }
}

const doc = {
  version: 1,
  source: "docs/schema/oxoflow-v1.schema.json",
  schemaSha256,
  contexts,
};

const serialized = JSON.stringify(doc, null, 2) + "\n";

if (process.argv.includes("--check")) {
  let existing = null;
  try {
    existing = readFileSync(outPath, "utf8");
  } catch {
    console.error(`generate:check: ${outPath} is missing — run "npm run generate"`);
    process.exit(1);
  }
  if (existing !== serialized) {
    console.error(
      "generate:check: generated completion data is stale — " +
        'run "npm run generate" and commit the result (schema changed?)'
    );
    process.exit(1);
  }
  console.log(`generate:check OK (${Object.keys(contexts).length} contexts, schema ${schemaSha256.slice(0, 12)}…)`);
  process.exit(0);
}

writeFileSync(outPath, serialized);
const sizeKb = (Buffer.byteLength(serialized) / 1024).toFixed(1);
console.log(
  `generated ${Object.keys(contexts).length} contexts ` +
    `(${Object.entries(contexts).map(([k, v]) => `${k}:${v.keys.length}`).join(", ")}) → ${outPath} (${sizeKb} KB)`
);
