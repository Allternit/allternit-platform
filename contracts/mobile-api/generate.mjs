#!/usr/bin/env node
// Generates the native Android app's API client from the OpenAPI specs listed in
// operations.json (native plan step 6, handoffs/android-native-2026-11-18).
//
//   node contracts/mobile-api/generate.mjs           write the Kotlin files
//   node contracts/mobile-api/generate.mjs --check   exit 1 when they differ from the specs
//
// Node 20+, no dependencies. The YAML reader below handles the subset our specs use
// (block mappings/sequences, flow { } and [ ], quoted and plain scalars, `|` blocks).
// Output (package com.allternit.core.api.gen):
//   Models.gen.kt  @Serializable data classes + enums (unknown values decode to Unknown)
//   Api.gen.kt     AllternitApi: one suspend fun per operation returning ApiResult<T>;
//                  `x-stream: sse` operations get only a request builder (AgentChatStream reads them).
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const outDir = join(repo, "surfaces/allternit-mobile/android/core/api/src/main/java/com/allternit/core/api/gen");
const check = process.argv.includes("--check");

// ── YAML subset ──────────────────────────────────────────────────────────────
function stripComment(line) {
  let q = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (q) { if (c === q) q = null; continue; }
    if (c === '"' || c === "'") { if (i === 0 || /[\s:\[{,]/.test(line[i - 1])) q = c; continue; }
    if (c === "#" && (i === 0 || /\s/.test(line[i - 1]))) return line.slice(0, i);
  }
  return line;
}

function parseScalar(s) {
  s = s.trim();
  if (s === "" || s === "~" || s === "null") return null;
  if (s === "true") return true;
  if (s === "false") return false;
  if (s.startsWith('"')) return JSON.parse(s);
  if (s.startsWith("'")) return s.slice(1, -1).replace(/''/g, "'");
  if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
  if (s.startsWith("{") || s.startsWith("[")) return parseFlow(s);
  return s;
}

function parseFlow(src) {
  let i = 0;
  const ws = () => { while (i < src.length && /\s/.test(src[i])) i++; };
  const quoted = () => {
    const q = src[i++];
    let out = "";
    while (i < src.length && src[i] !== q) {
      if (q === '"' && src[i] === "\\") { out += JSON.parse(`"${src[i]}${src[i + 1]}"`); i += 2; continue; }
      if (q === "'" && src[i] === "'" && src[i + 1] === "'") { out += "'"; i += 2; continue; }
      out += src[i++];
    }
    i++;
    return out;
  };
  const plain = (stops) => {
    const start = i;
    while (i < src.length && !stops.includes(src[i])) i++;
    return src.slice(start, i).trim();
  };
  const value = () => {
    ws();
    if (src[i] === "{") return map();
    if (src[i] === "[") return seq();
    if (src[i] === '"' || src[i] === "'") return quoted();
    return parseScalar(plain(",}]"));
  };
  const map = () => {
    i++;
    const out = {};
    for (;;) {
      ws();
      if (src[i] === "}") { i++; return out; }
      let key;
      if (src[i] === '"' || src[i] === "'") key = quoted();
      else { const start = i; while (i < src.length && !(src[i] === ":" && /[\s]/.test(src[i + 1] ?? " "))) i++; key = src.slice(start, i).trim(); }
      ws();
      if (src[i] !== ":") throw new Error(`flow map: expected ':' in ${src}`);
      i++;
      out[key] = value();
      ws();
      if (src[i] === ",") i++;
    }
  };
  const seq = () => {
    i++;
    const out = [];
    for (;;) {
      ws();
      if (src[i] === "]") { i++; return out; }
      out.push(value());
      ws();
      if (src[i] === ",") i++;
    }
  };
  const v = value();
  ws();
  if (i !== src.length) throw new Error(`flow: trailing text in ${src}`);
  return v;
}

function parseYaml(text) {
  const raw = text.split(/\r?\n/);
  const lines = [];
  for (let n = 0; n < raw.length; n++) {
    const stripped = stripComment(raw[n]).replace(/\s+$/, "");
    if (stripped.trim() === "") { lines.push({ n, blank: true, raw: raw[n] }); continue; }
    lines.push({ n, indent: stripped.match(/^ */)[0].length, text: stripped.trim(), raw: raw[n] });
  }
  let p = 0;
  const skipBlank = () => { while (p < lines.length && lines[p].blank) p++; };

  const splitKey = (t) => {
    let key, rest;
    if (t.startsWith('"') || t.startsWith("'")) {
      const end = t.indexOf(t[0], 1);
      key = t.slice(1, end);
      rest = t.slice(end + 1);
      if (!rest.startsWith(":")) return null;
      rest = rest.slice(1);
    } else {
      const m = t.match(/^([^:]+?):(\s|$)/);
      if (!m) return null;
      key = m[1];
      rest = t.slice(m[1].length + 1);
    }
    return [key, rest.trim()];
  };

  const blockScalar = (parentIndent) => {
    const out = [];
    let ind = null;
    while (p < lines.length) {
      const l = lines[p];
      if (!l.blank && l.indent <= parentIndent) break;
      if (l.blank) { out.push(""); p++; continue; }
      if (ind === null) ind = l.indent;
      out.push(l.raw.slice(ind));
      p++;
    }
    while (out.length && out[out.length - 1] === "") out.pop();
    return out.join("\n") + "\n";
  };

  const valueAfter = (rest, indent) => {
    if (rest === "|" || rest === "|-") { const s = blockScalar(indent); return rest === "|-" ? s.replace(/\n$/, "") : s; }
    if (rest !== "") return parseScalar(rest);
    skipBlank();
    if (p >= lines.length) return null;
    const next = lines[p];
    if (next.indent > indent) return block(next.indent);
    if (next.indent === indent && next.text.startsWith("- ")) return block(indent);
    return null;
  };

  const block = (indent) => {
    skipBlank();
    const first = lines[p];
    if (first.text.startsWith("- ") || first.text === "-") {
      const out = [];
      while (p < lines.length) {
        skipBlank();
        const l = lines[p];
        if (!l || l.indent !== indent || !(l.text.startsWith("- ") || l.text === "-")) break;
        const item = l.text.slice(1).trim();
        const kv = item && !item.startsWith("{") && !item.startsWith("[") ? splitKey(item) : null;
        if (kv) {
          // `- key: value` starts a mapping at indent + 2.
          lines[p] = { ...l, indent: indent + 2, text: item };
          out.push(block(indent + 2));
        } else {
          p++;
          out.push(item === "" ? valueAfter("", indent) : parseScalar(item));
        }
      }
      return out;
    }
    const out = {};
    while (p < lines.length) {
      skipBlank();
      const l = lines[p];
      if (!l || l.indent < indent) break;
      if (l.indent > indent) throw new Error(`yaml line ${l.n + 1}: unexpected indent`);
      if (l.text.startsWith("- ")) break;
      const kv = splitKey(l.text);
      if (!kv) throw new Error(`yaml line ${l.n + 1}: expected key: ${l.text}`);
      p++;
      out[kv[0]] = valueAfter(kv[1], indent);
    }
    return out;
  };
  return block(0);
}

// ── Load specs + operations ──────────────────────────────────────────────────
const ops = JSON.parse(readFileSync(join(here, "operations.json"), "utf8"));
const wanted = ops.phases.flatMap((ph) => ph.operations);
const schemas = new Map(); // name -> { schema, spec }
const found = new Map(); // operationId -> op
for (const rel of ops.specs) {
  const spec = parseYaml(readFileSync(join(repo, rel), "utf8"));
  for (const [name, schema] of Object.entries(spec.components?.schemas ?? {})) {
    const prev = schemas.get(name);
    if (prev && JSON.stringify(prev.schema) !== JSON.stringify(schema)) throw new Error(`schema ${name} differs between ${prev.spec} and ${rel}`);
    schemas.set(name, { schema, spec: rel });
  }
  for (const [path, item] of Object.entries(spec.paths ?? {})) {
    for (const method of ["get", "post", "put", "patch", "delete"]) {
      const op = item[method];
      if (!op) continue;
      if (found.has(op.operationId)) throw new Error(`duplicate operationId ${op.operationId}`);
      found.set(op.operationId, { ...op, method: method.toUpperCase(), path, spec: rel, params: [...(item.parameters ?? []), ...(op.parameters ?? [])] });
    }
  }
}
for (const id of wanted) if (!found.has(id)) throw new Error(`operations.json lists ${id}, which no spec defines`);

const refName = (ref) => ref.replace("#/components/schemas/", "");
const used = new Set();
const visit = (s) => {
  if (!s || typeof s !== "object") return;
  if (s.$ref) {
    const n = refName(s.$ref);
    if (!schemas.has(n)) throw new Error(`unknown schema ${n}`);
    if (!used.has(n)) { used.add(n); visit(schemas.get(n).schema); }
    return;
  }
  for (const v of Object.values(s)) if (typeof v === "object") visit(v);
};
const jsonSchema = (content) => content?.["application/json"]?.schema;
for (const id of wanted) {
  const op = found.get(id);
  visit(jsonSchema(op.requestBody?.content));
  for (const [code, r] of Object.entries(op.responses ?? {})) if (/^2/.test(code)) visit(jsonSchema(r.content));
}

// ── Kotlin ───────────────────────────────────────────────────────────────────
const KEYWORDS = new Set(["as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface", "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias", "typeof", "val", "var", "when", "while"]);
const camel = (s) => s.replace(/[-_ ]+([a-zA-Z0-9])/g, (_, c) => c.toUpperCase()).replace(/^[A-Z]/, (c) => c.toLowerCase());
const ident = (s) => (KEYWORDS.has(s) ? `\`${s}\`` : s);
const pascal = (s) => { const c = camel(s); return c.charAt(0).toUpperCase() + c.slice(1); };
const kdoc = (text, indent = "") => {
  if (!text) return "";
  const lines = String(text).trim().split("\n").map((l) => l.replace(/\*\//g, "* /"));
  return lines.length === 1 ? `${indent}/** ${lines[0]} */\n` : `${indent}/**\n${lines.map((l) => `${indent} * ${l}`.trimEnd()).join("\n")}\n${indent} */\n`;
};

function ktType(s) {
  if (!s) return { t: "JsonElement", nullable: true };
  if (s["x-kotlin-raw"]) return { t: "JsonElement", nullable: true };
  if (s.$ref) return { t: refName(s.$ref), nullable: false };
  let type = s.type;
  let nullable = false;
  if (Array.isArray(type)) { nullable = type.includes("null"); type = type.find((x) => x !== "null"); }
  switch (type) {
    case "string": return { t: "String", nullable };
    case "integer": return { t: s.format === "int32" ? "Int" : "Long", nullable };
    case "number": return { t: "Double", nullable };
    case "boolean": return { t: "Boolean", nullable };
    case "array": return { t: `List<${ktType(s.items).t}>`, nullable };
    case "object":
      if (s.additionalProperties && !s.properties) return { t: `Map<String, ${ktType(s.additionalProperties).t}>`, nullable };
      throw new Error(`inline object schema: give it a name under components/schemas (${JSON.stringify(s).slice(0, 80)})`);
    default: throw new Error(`unsupported schema ${JSON.stringify(s).slice(0, 80)}`);
  }
}

function emitEnum(name, s) {
  const cases = s.enum.map((v) => `    ${pascal(String(v))}("${v}"),`).join("\n");
  return `${kdoc(s.description)}@Serializable(with = ${name}.Serializer::class)
enum class ${name}(val wire: String) {
${cases}
    /** A value this build does not know yet. */
    Unknown("");

    companion object {
        fun fromWire(value: String): ${name} = entries.firstOrNull { it != Unknown && it.wire == value } ?: Unknown
    }

    object Serializer : KSerializer<${name}> {
        override val descriptor: SerialDescriptor =
            PrimitiveSerialDescriptor("com.allternit.core.api.gen.${name}", PrimitiveKind.STRING)
        override fun serialize(encoder: Encoder, value: ${name}) = encoder.encodeString(value.wire)
        override fun deserialize(decoder: Decoder): ${name} = fromWire(decoder.decodeString())
    }
}
`;
}

function emitClass(name, s) {
  const required = new Set(s.required ?? []);
  const props = Object.entries(s.properties ?? {});
  if (!props.length) return `${kdoc(s.description)}@Serializable\nclass ${name}\n`;
  const fields = props.map(([wire, ps]) => {
    const { t, nullable } = ktType(ps);
    const opt = !required.has(wire) || nullable || t === "JsonElement";
    const decl = `    @SerialName("${wire}") val ${ident(camel(wire))}: ${t}${opt ? "? = null" : ""},`;
    return kdoc(ps.description, "    ") + decl;
  });
  return `${kdoc(s.description)}@Serializable\ndata class ${name}(\n${fields.join("\n")}\n)\n`;
}

const specList = ops.specs.join(", ");
const header = (what) => `// GENERATED by contracts/mobile-api/generate.mjs from ${specList}
// (operations: contracts/mobile-api/operations.json). ${what} Do not edit; change the spec and regenerate.
@file:Suppress("unused", "RedundantVisibilityModifier")

package com.allternit.core.api.gen
`;

const modelNames = [...used].sort();
const models = `${header("Wire models.")}
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonElement

${modelNames.map((n) => { const s = schemas.get(n).schema; return s.enum ? emitEnum(n, s) : emitClass(n, s); }).join("\n")}`;

function serializerOf(s) {
  if (s.$ref) return `${refName(s.$ref)}.serializer()`;
  if (s.type === "array") return `ListSerializer(${serializerOf(s.items)})`;
  throw new Error(`no serializer for ${JSON.stringify(s)}`);
}

function emitOp(op) {
  const base = op["x-base"];
  if (!["api", "cloud", "instance"].includes(base)) throw new Error(`${op.operationId}: x-base must be api, cloud or instance`);
  const args = [];
  if (base === "instance") args.push("instanceBaseUrl: HttpUrl");
  const pathParams = op.params.filter((p) => p.in === "path");
  const queryParams = op.params.filter((p) => p.in === "query");
  for (const p of pathParams) args.push(`${ident(camel(p.name))}: String`);
  const bodySchema = jsonSchema(op.requestBody?.content);
  if (bodySchema) args.push(`body: ${ktType(bodySchema).t}`);
  for (const p of queryParams) args.push(`${ident(camel(p.name))}: ${ktType(p.schema).t}? = null`);

  const baseExpr = { api: "config.apiBaseUrl", cloud: "config.cloudBaseUrl", instance: "instanceBaseUrl" }[base];
  const segs = op.path.split("/").filter(Boolean).map((seg) => {
    const m = seg.match(/^\{(.+)\}$/);
    return m ? `.addPathSegment(${ident(camel(m[1]))})` : `.addPathSegment("${seg}")`;
  });
  const query = queryParams.map((p) => `.apply { ${ident(camel(p.name))}?.let { addQueryParameter("${p.name}", it.toString()) } }`);
  const url = `${baseExpr}.newBuilder()${[...segs, ...query].join("")}.build()`;
  const reqBody = bodySchema
    ? `json.encodeToString(${ktType(bodySchema).t}.serializer(), body)`
    : ["POST", "PUT", "PATCH"].includes(op.method) ? `"{}"` : "null";
  const doc = kdoc(`${op.method} ${op.path} (${base}): ${op.summary ?? ""}${op.description ? "\n\n" + op.description.trim() : ""}\nHandler: ${op["x-handler"] ?? "?"}`, "    ");

  if (op["x-stream"] === "sse") {
    return `${doc}    fun ${op.operationId}Request(${args.join(", ")}): Request =
        ApiCall.request(${op.method === "GET" ? "\"GET\"" : `"${op.method}"`}, ${url}, ${reqBody}, accept = "text/event-stream")
`;
  }
  const ok = Object.entries(op.responses ?? {}).find(([c]) => /^2/.test(c));
  const okSchema = ok && jsonSchema(ok[1].content);
  const result = okSchema ? ktType(okSchema).t : "Unit";
  const call = okSchema
    ? `ApiCall.execute(client, json, request, ${serializerOf(okSchema)})`
    : `ApiCall.executeUnit(client, json, request)`;
  return `${doc}    suspend fun ${op.operationId}(${args.join(", ")}): ApiResult<${result}> {
        val request = ApiCall.request("${op.method}", ${url}, ${reqBody})
        return ${call}
    }
`;
}

const api = `${header("Typed client.")}
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request

/**
 * The Allternit app API. Every call returns an [ApiResult]; nothing throws to the caller
 * (cancellation aside). Build [client] with \`ApiClients.okHttp\` so the bearer is attached.
 */
class AllternitApi(
    private val client: OkHttpClient,
    private val config: ApiConfig,
    private val json: Json = ApiJson.default,
) {
${ops.phases.map((ph) => `    // ── ${ph.name} ──\n\n${ph.operations.map((id) => emitOp(found.get(id))).join("\n")}`).join("\n")}}
`;

const outputs = { "Models.gen.kt": models, "Api.gen.kt": api };
let stale = 0;
for (const [file, text] of Object.entries(outputs)) {
  const path = join(outDir, file);
  if (check) {
    let cur = null;
    try { cur = readFileSync(path, "utf8"); } catch {}
    if (cur !== text) { console.error(`stale: ${path}`); stale++; }
  } else {
    mkdirSync(outDir, { recursive: true });
    writeFileSync(path, text);
  }
}
const summary = `${wanted.length} operations (${wanted.filter((id) => found.get(id)["x-stream"]).length} streaming), ${modelNames.length} models`;
if (check) {
  if (stale) { console.error(`mobile-api: generated Kotlin is out of date; run node contracts/mobile-api/generate.mjs`); process.exit(1); }
  console.log(`mobile-api: up to date (${summary})`);
} else {
  console.log(`mobile-api: wrote ${Object.keys(outputs).length} files to ${outDir} (${summary})`);
}
