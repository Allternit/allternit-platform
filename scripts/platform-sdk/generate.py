#!/usr/bin/env python3
"""Generate the Allternit Platform SDKs' typed layer from the OpenAPI file.

Source of truth: cmd/allternit-cloud-api/openapi/platform-v1.yaml.

Writes:
  sdk/platform-ts/src/generated/types.ts       (schema + request/response types)
  sdk/platform-ts/src/generated/operations.ts  (one resource class per tag)
  sdk/platform-python/allternit_platform/_generated.py

Everything is derived generically from the spec (paths, path/query params,
JSON request bodies, response schemas, $ref/allOf/nullable/enum/const), so
new routes added to the yaml show up after a re-run with no generator change.

Usage:
  python3 scripts/platform-sdk/generate.py           # write files
  python3 scripts/platform-sdk/generate.py --check   # exit 1 if files are stale

Needs PyYAML (`pip3 install pyyaml`).
"""

from __future__ import annotations

import keyword
import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("generate.py needs PyYAML: pip3 install pyyaml")

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "cmd/allternit-cloud-api/openapi/platform-v1.yaml"
TS_TYPES = ROOT / "sdk/platform-ts/src/generated/types.ts"
TS_OPS = ROOT / "sdk/platform-ts/src/generated/operations.ts"
PY_GEN = ROOT / "sdk/platform-python/allternit_platform/_generated.py"

HEADER = "Generated from cmd/allternit-cloud-api/openapi/platform-v1.yaml by scripts/platform-sdk/generate.py. Do not edit."
METHODS = ["get", "post", "put", "patch", "delete"]


# ---------------------------------------------------------------- helpers

def pascal(s: str) -> str:
    parts = re.split(r"[^A-Za-z0-9]+", s)
    out = "".join(p[:1].upper() + p[1:] for p in parts if p)
    return out if out and not out[0].isdigit() else "T" + out


def camel(s: str) -> str:
    p = pascal(s)
    return p[:1].lower() + p[1:]


def snake(s: str) -> str:
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", s)
    return re.sub(r"[^A-Za-z0-9]+", "_", s).strip("_").lower()


def ts_key(k: str) -> str:
    return k if re.fullmatch(r"[A-Za-z_$][A-Za-z0-9_$]*", k) else repr_ts(k)


def repr_ts(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if v is None:
        return "null"
    if isinstance(v, (int, float)):
        return str(v)
    return '"' + str(v).replace("\\", "\\\\").replace('"', '\\"') + '"'


def doc_ts(text: str | None, indent: str) -> str:
    if not text:
        return ""
    lines = [l.rstrip() for l in str(text).strip().splitlines()]
    body = "\n".join(f"{indent} * {l}".rstrip() for l in lines).replace("*/", "*\\/")
    return f"{indent}/**\n{body}\n{indent} */\n"


def py_name(k: str) -> str:
    n = snake(k) if not re.fullmatch(r"[a-z_][a-z0-9_]*", k) else k
    if keyword.iskeyword(n) or n in {"self", "options"}:
        n += "_"
    return n


class Spec:
    def __init__(self, raw: dict):
        self.raw = raw
        comps = raw.get("components", {}) or {}
        self.schemas = comps.get("schemas", {}) or {}
        self.parameters = comps.get("parameters", {}) or {}
        self.responses = comps.get("responses", {}) or {}
        self.request_bodies = comps.get("requestBodies", {}) or {}

    def deref(self, node):
        seen = 0
        while isinstance(node, dict) and "$ref" in node and seen < 20:
            ref = node["$ref"]
            _, _, kind, name = ref.split("/", 3)
            node = (self.raw["components"][kind])[name]
            seen += 1
        return node


def ref_name(node) -> str | None:
    if isinstance(node, dict) and "$ref" in node and node["$ref"].startswith("#/components/schemas/"):
        return node["$ref"].rsplit("/", 1)[1]
    return None


# ---------------------------------------------------------------- operations

class Param:
    def __init__(self, name, location, required, schema, description):
        self.name, self.location, self.required = name, location, required
        self.schema, self.description = schema or {}, description


class Op:
    pass


def collect_ops(spec: Spec) -> list[Op]:
    ops = []
    for path, item in (spec.raw.get("paths") or {}).items():
        shared = item.get("parameters", []) or []
        for method in METHODS:
            o = item.get(method)
            if not o:
                continue
            op = Op()
            op.path, op.method = path, method.upper()
            op.operation_id = o.get("operationId") or camel(f"{method} {path}")
            op.tag = (o.get("tags") or ["Default"])[0]
            op.summary, op.description = o.get("summary"), o.get("description")
            params = {}
            for p in shared + (o.get("parameters") or []):
                p = spec.deref(p)
                params[(p["name"], p["in"])] = Param(p["name"], p["in"], bool(p.get("required")), p.get("schema"), p.get("description"))
            order = [m.group(1) for m in re.finditer(r"\{([^}]+)\}", path)]
            op.path_params = [params[(n, "path")] for n in order if (n, "path") in params]
            op.query = [p for p in params.values() if p.location == "query"]
            op.idempotent_header = any(p.location == "header" and p.name.lower() == "idempotency-key" for p in params.values())
            rb = spec.deref(o.get("requestBody")) if o.get("requestBody") else None
            op.body_schema, op.body_required = None, False
            if rb:
                content = rb.get("content", {}) or {}
                if "application/json" in content:
                    op.body_schema = content["application/json"].get("schema") or {}
                    op.body_required = bool(rb.get("required"))
            op.response_schema, op.streams = None, False
            for code, resp in (o.get("responses") or {}).items():
                if not str(code).startswith("2"):
                    continue
                resp = spec.deref(resp)
                content = resp.get("content", {}) or {}
                if "text/event-stream" in content:
                    op.streams = True
                if op.response_schema is None and "application/json" in content:
                    op.response_schema = content["application/json"].get("schema") or {}
            op.paginated = any(p.name == "after" for p in op.query)
            ops.append(op)
    assign_method_names(ops)
    return ops


def assign_method_names(ops: list[Op]) -> None:
    """`createAgent` under tag Agents -> `create`; `sendConversationMessage`
    under Conversations -> `sendMessage`. Falls back to the operationId on a
    collision."""
    by_tag: dict[str, list[Op]] = {}
    for op in ops:
        by_tag.setdefault(op.tag, []).append(op)
    for tag, group in by_tag.items():
        plural = pascal(tag)
        singular = plural[:-1] if plural.endswith("s") else plural
        for op in group:
            m = re.match(r"([a-z]+)(.*)", op.operation_id)
            verb, rest = (m.group(1), m.group(2)) if m else (op.operation_id, "")
            name = op.operation_id
            if rest in (plural, singular):
                name = verb
            else:
                for noun in (plural, singular):
                    if rest.startswith(noun) and rest[len(noun):][:1].isupper():
                        name = verb + rest[len(noun):]
                        break
            op.method_name = name
        seen: dict[str, int] = {}
        for op in group:
            seen[op.method_name] = seen.get(op.method_name, 0) + 1
        for op in group:
            if seen[op.method_name] > 1 or op.method_name in {"constructor", "client"}:
                op.method_name = op.operation_id
            op.py_name = py_name(snake(op.method_name))


def page_item(spec: Spec, schema) -> object | None:
    """The `data[]` item schema of a list response, if any."""
    if not schema:
        return None
    s = spec.deref(schema)
    candidates = [s] + [spec.deref(x) for x in (s.get("allOf") or [])]
    for c in candidates:
        data = (c.get("properties") or {}).get("data")
        if data and (spec.deref(data).get("type") == "array"):
            items = spec.deref(data).get("items")
            if items:
                return data.get("items") if isinstance(data, dict) and "items" in data else items
    return None


# ---------------------------------------------------------------- TypeScript

TS_REF_PREFIX = ""  # "T." while writing operations.ts


def ts_type(spec: Spec, s, indent="") -> str:
    if s is None or s == {} or s is True:
        return "unknown"
    name = ref_name(s)
    if name:
        return TS_REF_PREFIX + pascal(name)
    if "$ref" in s:
        return ts_type(spec, spec.deref(s), indent)
    if "allOf" in s:
        return "(" + " & ".join(ts_type(spec, x, indent) for x in s["allOf"]) + ")"
    for key in ("oneOf", "anyOf"):
        if key in s:
            return "(" + " | ".join(ts_type(spec, x, indent) for x in s[key]) + ")"
    if "const" in s:
        return repr_ts(s["const"])
    t = s.get("type")
    types = t if isinstance(t, list) else ([t] if t else [])
    nullable = "null" in types or s.get("nullable") is True
    types = [x for x in types if x != "null"]
    if "enum" in s:
        base = " | ".join(repr_ts(v) for v in s["enum"] if v is not None)
        return f"({base} | null)" if nullable or None in s["enum"] else base
    parts = []
    if not types and ("properties" in s or "additionalProperties" in s):
        types = ["object"]
    for t in types:
        if t in ("integer", "number"):
            parts.append("number")
        elif t == "string":
            parts.append("string")
        elif t == "boolean":
            parts.append("boolean")
        elif t == "array":
            inner = ts_type(spec, s.get("items"), indent)
            parts.append(f"Array<{inner}>")
        elif t == "object":
            parts.append(ts_object(spec, s, indent))
    if not parts:
        parts.append("unknown")
    if nullable:
        parts.append("null")
    return parts[0] if len(parts) == 1 else "(" + " | ".join(parts) + ")"


def ts_object(spec: Spec, s, indent="") -> str:
    props = s.get("properties") or {}
    required = set(s.get("required") or [])
    extra = s.get("additionalProperties")
    if not props:
        if extra is False:
            return "Record<string, never>"
        return "Record<string, unknown>" if extra in (None, True, {}) else f"Record<string, {ts_type(spec, extra, indent)}>"
    inner = indent + "  "
    lines = ["{"]
    for k, v in props.items():
        desc = v.get("description") if isinstance(v, dict) else None
        lines.append(doc_ts(desc, inner).rstrip("\n")) if desc else None
        opt = "" if k in required else "?"
        lines.append(f"{inner}{ts_key(k)}{opt}: {ts_type(spec, v, inner)};")
    if extra not in (None, False):
        lines.append(f"{inner}[key: string]: unknown;")
    lines.append(indent + "}")
    return "\n".join(l for l in lines if l is not None)


def gen_ts(spec: Spec, ops: list[Op]) -> tuple[str, str]:
    out = [f"// {HEADER}", "/* eslint-disable */", ""]
    for name, s in spec.schemas.items():
        out.append(doc_ts(s.get("description"), "").rstrip("\n")) if s.get("description") else None
        out.append(f"export type {pascal(name)} = {ts_type(spec, s)};\n")
    for op in ops:
        base = pascal(op.operation_id)
        if op.query:
            fields = ["{"]
            for p in op.query:
                if p.description:
                    fields.append(doc_ts(p.description, "  ").rstrip("\n"))
                fields.append(f"  {ts_key(p.name)}{'' if p.required else '?'}: {ts_type(spec, p.schema, '  ')};")
            fields.append("}")
            out.append(f"export type {base}Query = " + "\n".join(fields) + ";\n")
        if op.body_schema is not None:
            out.append(f"export type {base}Request = {ts_type(spec, op.body_schema)};\n")
        if op.response_schema is not None:
            out.append(f"export type {base}Response = {ts_type(spec, op.response_schema)};\n")
        else:
            out.append(f"export type {base}Response = unknown;\n")
    types_ts = "\n".join(l for l in out if l is not None).rstrip() + "\n"

    o = [f"// {HEADER}", "/* eslint-disable */",
         'import type { RequestOptions, Transport, Page, SSEEvent } from "../core.ts";',
         'import type * as T from "./types.ts";', ""]
    tags: dict[str, list[Op]] = {}
    for op in ops:
        tags.setdefault(op.tag, []).append(op)
    for tag, group in tags.items():
        cls = pascal(tag) + "Resource"
        o.append(f"export class {cls} {{")
        o.append("  protected readonly _client: Transport;")
        o.append("  constructor(client: Transport) {\n    this._client = client;\n  }")
        for op in group:
            base = pascal(op.operation_id)
            args, call_path = [], op.path
            for p in op.path_params:
                v = camel(p.name)
                args.append(f"{v}: string")
                call_path = call_path.replace("{" + p.name + "}", "${encodeURIComponent(" + v + ")}")
            body_arg = query_expr = None
            if op.body_schema is not None:
                body_type = f"T.{base}Request"
                if op.streams:
                    body_type = f'Omit<T.{base}Request, "stream">'
                args.append(f"body{'' if op.body_required else '?'}: {body_type}")
                body_arg = "body"
                if op.query:
                    query_expr = "options?.query"
            elif op.query:
                req = any(p.required for p in op.query)
                args.append(f"query{'' if req else '?'}: T.{base}Query")
                query_expr = "query"
            args.append("options?: RequestOptions")
            resp = f"T.{base}Response"
            desc = op.summary or ""
            if op.description:
                desc = (desc + "\n\n" + op.description).strip()
            o.append("")
            o.append(doc_ts(f"{desc}\n\n`{op.method} {op.path}`", "  ").rstrip("\n"))
            req_obj = (
                f'{{ method: "{op.method}", path: `{call_path}`'
                + (f", query: {query_expr}" if query_expr else "")
                + (f", body: {body_arg}" if body_arg else "")
                + ", options }"
            )
            o.append(f"  {op.method_name}({', '.join(args)}): Promise<{resp}> {{")
            o.append(f"    return this._client.request<{resp}>({req_obj});")
            o.append("  }")
            if op.paginated:
                item = page_item(spec, op.response_schema)
                global TS_REF_PREFIX
                TS_REF_PREFIX = "T."
                item_t = ts_type(spec, item, "  ") if item is not None else "unknown"
                TS_REF_PREFIX = ""
                q_args = [a for a in args if not a.startswith("options")]
                fetch_args = [a.split(":")[0].rstrip("?") for a in q_args]
                qname = "query" if "query" in fetch_args else None
                o.append("")
                o.append(doc_ts(f"Every item of `{op.method_name}`, fetching pages as you iterate.", "  ").rstrip("\n"))
                o.append(f"  {op.method_name}All({', '.join(args)}): AsyncIterable<{item_t}> {{")
                call_args = ", ".join(
                    ([a for a in fetch_args if a != "query"])
                    + ([f"{{ ...({qname} ?? {{}}), after }} as T.{base}Query"] if qname else [])
                    + ["options"]
                )
                o.append(f"    return this._client.paginate<{item_t}>((after) => this.{op.method_name}({call_args}) as unknown as Promise<Page<{item_t}>>);")
                o.append("  }")
            if op.streams:
                o.append("")
                o.append(doc_ts(f"`{op.method_name}` with `stream: true`: yields the raw server-sent events.", "  ").rstrip("\n"))
                stream_body = "{ ...(body ?? {}), stream: true }" if body_arg else "{ stream: true }"
                req_obj = (
                    f'{{ method: "{op.method}", path: `{call_path}`'
                    + (f", query: {query_expr}" if query_expr else "")
                    + f", body: {stream_body}, options }}"
                )
                o.append(f"  {op.method_name}Stream({', '.join(args)}): AsyncIterable<SSEEvent> {{")
                o.append(f"    return this._client.streamRequest({req_obj});")
                o.append("  }")
        o.append("}\n")
    o.append("/** Every API area as a client property. `AllternitPlatform` extends this. */")
    o.append("export class GeneratedResources {")
    for tag in tags:
        o.append(f"  {camel(tag)}: {pascal(tag)}Resource;")
    o.append("  constructor(client: Transport) {")
    for tag in tags:
        o.append(f"    this.{camel(tag)} = new {pascal(tag)}Resource(client);")
    o.append("  }")
    o.append("}")
    return types_ts, "\n".join(l for l in o if l is not None).rstrip() + "\n"


# ---------------------------------------------------------------- Python

def py_type(spec: Spec, s) -> str:
    if s is None or s == {} or s is True:
        return "Any"
    name = ref_name(s)
    if name:
        return f'"{pascal(name)}"'
    if "$ref" in s:
        return py_type(spec, spec.deref(s))
    if "allOf" in s:
        refs = [x for x in s["allOf"] if ref_name(x)]
        return py_type(spec, refs[0]) if len(refs) == 1 and len(s["allOf"]) == 1 else "Dict[str, Any]"
    for key in ("oneOf", "anyOf"):
        if key in s:
            return "Union[" + ", ".join(py_type(spec, x) for x in s[key]) + "]"
    if "const" in s:
        return f"Literal[{s['const']!r}]"
    t = s.get("type")
    types = t if isinstance(t, list) else ([t] if t else [])
    nullable = "null" in types or s.get("nullable") is True
    types = [x for x in types if x != "null"]
    if "enum" in s:
        base = "Literal[" + ", ".join(repr(v) for v in s["enum"] if v is not None) + "]"
        return f"Optional[{base}]" if nullable else base
    parts = []
    if not types and ("properties" in s or "additionalProperties" in s):
        types = ["object"]
    for t in types:
        parts.append({"integer": "int", "number": "float", "string": "str", "boolean": "bool"}.get(t) or (
            f"List[{py_type(spec, s.get('items'))}]" if t == "array" else "Dict[str, Any]"))
    base = parts[0] if len(parts) == 1 else ("Union[" + ", ".join(parts) + "]" if parts else "Any")
    return f"Optional[{base}]" if nullable else base


def py_doc(text: str | None, indent: str) -> str:
    if not text:
        return ""
    t = str(text).strip().replace('"""', "'''").replace("\\", "\\\\")
    lines = t.splitlines()
    if len(lines) == 1:
        return f'{indent}"""{lines[0]}"""\n'
    return f'{indent}"""' + lines[0] + "\n" + "\n".join((indent + l).rstrip() for l in lines[1:]) + f"\n{indent}\"\"\"\n"


def gen_py(spec: Spec, ops: list[Op]) -> str:
    o = [f'"""{HEADER}"""', "# flake8: noqa", "# fmt: off", "",
         "from typing import Any, Dict, Iterator, List, Optional, Union",
         "",
         "try:",
         "    from typing import Literal, TypedDict",
         "except ImportError:  # pragma: no cover",
         "    from typing_extensions import Literal, TypedDict  # type: ignore",
         "",
         "from ._core import Page, SSEEvent, Transport",
         "",
         "",
         "class _Unset:",
         '    def __repr__(self) -> str:',
         '        return "NOT_GIVEN"',
         "",
         "",
         "NOT_GIVEN: Any = _Unset()",
         "",
         ""]
    for name, s in spec.schemas.items():
        s2 = spec.deref(s)
        props = s2.get("properties")
        if s2.get("type") == "object" and props:
            fields = ", ".join(f"{k!r}: {py_type(spec, v)}" for k, v in props.items())
            o.append(f"{pascal(name)} = TypedDict({pascal(name)!r}, {{{fields}}}, total=False)")
        else:
            t = py_type(spec, s2)
            o.append(f"{pascal(name)} = {t.strip(chr(34)) if t.startswith(chr(34)) else t}")
    o.append("")
    o.append("")

    tags: dict[str, list[Op]] = {}
    for op in ops:
        tags.setdefault(op.tag, []).append(op)
    for tag, group in tags.items():
        o.append(f"class {pascal(tag)}Resource:")
        o.append("    def __init__(self, client: Transport) -> None:")
        o.append("        self._client = client")
        for op in group:
            params, path_expr = ["self"], op.path
            for p in op.path_params:
                v = py_name(p.name)
                params.append(f"{v}: str")
                path_expr = path_expr.replace("{" + p.name + "}", "{_q(" + v + ")}")
            kw, body_fields, query_fields = [], [], []
            used = {py_name(p.name) for p in op.path_params}
            body = spec.deref(op.body_schema) if op.body_schema is not None else None
            free_body = False
            if body is not None:
                props = (body.get("properties") or {})
                req = set(body.get("required") or [])
                if props:
                    for k, v in props.items():
                        if op.streams and k == "stream":
                            continue
                        n = py_name(k)
                        while n in used:
                            n += "_"
                        used.add(n)
                        body_fields.append((k, n))
                        if k in req:
                            kw.append(f"{n}: {py_type(spec, v)}")
                        else:
                            kw.append(f"{n}: {py_type(spec, v)} = NOT_GIVEN")
                if not props or body.get("additionalProperties") not in (None, False):
                    free_body = "extra_body" if "body" in used else "body"
                    kw.append(f"{free_body}: Optional[Dict[str, Any]] = None")
            for p in op.query:
                n = py_name(p.name)
                while n in used:
                    n += "_"
                used.add(n)
                query_fields.append((p.name, n))
                kw.append(f"{n}: {py_type(spec, p.schema)}" + ("" if p.required else " = NOT_GIVEN"))
            # required keyword args first
            kw.sort(key=lambda a: "=" in a)
            post = op.method == "POST"
            kw += (["idempotency_key: Optional[str] = None"] if post else []) + ["timeout: Optional[float] = None", "extra_headers: Optional[Dict[str, str]] = None"]
            sig = ", ".join(params + ["*"] + kw)
            resp = op.response_schema
            ret = py_type(spec, resp) if resp is not None and ref_name(resp) else "Dict[str, Any]"

            def emit_body_lines(indent="        "):
                lines = []
                if body is not None:
                    lines.append(f"{indent}_body: Dict[str, Any] = dict({free_body} or {{}})" if free_body else f"{indent}_body: Dict[str, Any] = {{}}")
                    for k, n in body_fields:
                        lines.append(f"{indent}if {n} is not NOT_GIVEN:\n{indent}    _body[{k!r}] = {n}")
                if query_fields:
                    lines.append(f"{indent}_query: Dict[str, Any] = {{}}")
                    for k, n in query_fields:
                        lines.append(f"{indent}if {n} is not NOT_GIVEN:\n{indent}    _query[{k!r}] = {n}")
                return lines

            call_kw = (f'"{op.method}", f"{path_expr}"'
                       + (", query=_query" if query_fields else "")
                       + (", body=_body" if body is not None else "")
                       + (", idempotency_key=idempotency_key" if post else "")
                       + ", timeout=timeout, extra_headers=extra_headers")
            desc = (op.summary or "") + ("\n\n" + op.description if op.description else "")
            o.append("")
            o.append(f"    def {op.py_name}({sig}) -> {ret}:")
            o.append(py_doc(f"{desc.strip()}\n\n``{op.method} {op.path}``", "        ").rstrip("\n"))
            o.extend(emit_body_lines())
            o.append(f"        return self._client.request({call_kw})  # type: ignore[return-value]")
            if op.paginated:
                item = page_item(spec, resp)
                item_t = py_type(spec, item) if item is not None and ref_name(item) else "Dict[str, Any]"
                after_var = dict(query_fields).get("after", "after")
                o.append("")
                o.append(f"    def {op.py_name}_all({sig}) -> Iterator[{item_t}]:")
                o.append(py_doc(f"Every item of ``{op.py_name}``, fetching pages as you iterate.", "        ").rstrip("\n"))
                o.extend(emit_body_lines())
                o.append("        def _fetch(cursor: Optional[str]) -> Page:")
                o.append("            q = dict(_query)")
                o.append("            if cursor is not None:")
                o.append(f"                q['after'] = cursor")
                o.append(f'            return self._client.request("{op.method}", f"{path_expr}", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]')
                o.append(f"        return self._client.paginate(_fetch, {after_var} if {after_var} is not NOT_GIVEN else None)")
            if op.streams:
                o.append("")
                o.append(f"    def {op.py_name}_stream({sig}) -> Iterator[SSEEvent]:")
                o.append(py_doc(f"``{op.py_name}`` with ``stream: true``: yields the raw server-sent events.", "        ").rstrip("\n"))
                o.extend(emit_body_lines())
                if body is not None:
                    o.append("        _body['stream'] = True")
                o.append(f"        return self._client.stream_request({call_kw})")
        o.append("")
        o.append("")
    o.append("def _q(v: str) -> str:")
    o.append("    from urllib.parse import quote")
    o.append("    return quote(str(v), safe='')")
    o.append("")
    o.append("")
    o.append("class GeneratedResources:")
    o.append('    """Every API area as a client attribute. ``AllternitPlatform`` extends this."""')
    o.append("")
    o.append("    def __init__(self, client: Transport) -> None:")
    for tag in tags:
        o.append(f"        self.{snake(tag)} = {pascal(tag)}Resource(client)")
    return "\n".join(l for l in o if l is not None).rstrip() + "\n"


# ---------------------------------------------------------------- main

def main() -> int:
    check = "--check" in sys.argv[1:]
    spec = Spec(yaml.safe_load(SPEC.read_text()))
    ops = collect_ops(spec)
    types_ts, ops_ts = gen_ts(spec, ops)
    outputs = {TS_TYPES: types_ts, TS_OPS: ops_ts, PY_GEN: gen_py(spec, ops)}
    stale = []
    for path, text in outputs.items():
        current = path.read_text() if path.exists() else None
        if current != text:
            stale.append(path)
            if not check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
    rel = [str(p.relative_to(ROOT)) for p in stale]
    if check:
        if stale:
            print("SDK code is stale; run: python3 scripts/platform-sdk/generate.py\n  " + "\n  ".join(rel))
            return 1
        print(f"SDK code is up to date ({len(ops)} operations).")
        return 0
    print(f"Generated {len(ops)} operations." + (" Updated: " + ", ".join(rel) if rel else " No changes."))
    return 0


if __name__ == "__main__":
    sys.exit(main())
