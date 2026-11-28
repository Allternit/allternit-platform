#!/usr/bin/env python3
"""Bundle the os/v1 schemas plus the frozen ABI defs they reach into one codegen-only schema (not normative).

Same approach as spec/Contracts/kernel/v1/scripts/bundle_for_codegen.py:
  * os defs plus the transitive closure of referenced ABI defs go into one `definitions` map;
  * `$defs/<Name>` refs become `#/definitions/<Name>`; deeper pointers into an ABI def
    (e.g. LeaseV1/properties/reclaim_policy) are inlined;
  * conditional-only keywords (if/then/not/propertyNames, $comment) and `format` are dropped.
Conditional rules are enforced by schema validation (conformance tests), never by types.
The normative contract is always schemas/*.json.
"""
import glob, json, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ABI = os.path.normpath(os.path.join(ROOT, "..", "..", "kernel", "v1"))
DROP = {"$comment", "not", "propertyNames", "format", "if", "then", "else", "$schema", "$id", "title"}

docs = {}
for base in (ROOT, ABI):
    for p in glob.glob(f"{base}/schemas/*.json"):
        d = json.load(open(p))
        docs[d["$id"]] = d


def split(ref, cur):
    uri, _, ptr = ref.partition("#")
    if not uri:
        uri = cur
    elif not uri.startswith("http"):
        uri = cur.rsplit("/", 1)[0] + "/" + uri
    return uri, [p for p in ptr.split("/") if p]


defs, todo = {}, []


def clean(node, cur):
    if isinstance(node, list):
        return [clean(x, cur) for x in node]
    if not isinstance(node, dict):
        return node
    if "$ref" in node:
        uri, parts = split(node["$ref"], cur)
        if len(parts) == 2:
            todo.append((uri, parts[1]))
            return {"$ref": "#/definitions/" + parts[1]}
        target = docs[uri]
        for p in parts:
            target = target[p]
        return clean(target, uri)
    out = {}
    for k, v in node.items():
        if k in DROP:
            continue
        if k == "allOf":
            kept = [clean(x, cur) for x in v if not (isinstance(x, dict) and ("if" in x or "not" in x))]
            kept = [x for x in kept if x]
            if kept:
                out[k] = kept
            continue
        out[k] = clean(v, cur)
    return out


for uri, d in docs.items():
    if uri.startswith("https://schemas.allternit.com/os/"):
        todo += [(uri, n) for n in d["$defs"]]
while todo:
    uri, name = todo.pop()
    if name in defs:
        continue
    defs[name] = None
    defs[name] = clean(docs[uri]["$defs"][name], uri)

bundle = {
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "AllternitOsContracts",
    "description": "Codegen bundle of AllternitOS contracts os/v1 plus the ABI defs they reference. Not normative; see schemas/.",
    "definitions": defs,
}
json.dump(bundle, open(sys.argv[1], "w"), indent=2)
print(f"bundled {len(defs)} defs -> {sys.argv[1]}")
