#!/usr/bin/env python3
"""Bundle the 21 frozen schemas into one codegen-only schema (not normative).

Codegen tools (json-schema-to-typescript, typify) do not model cross-file $refs or
conditional if/then/not rules. This bundle:
  * merges every $defs entry into one `definitions` map (def names are unique);
  * rewrites refs to `#/definitions/<Name>`;
  * drops conditional-only keywords (allOf of if/then, not, propertyNames, $comment) and
    `format` (date-time becomes a plain String, still validated by the schema; keeps the crate free of chrono).
Conditional rules are enforced by schema validation (conformance tests), never by types.
The normative contract is always schemas/*.json.
"""
import glob, json, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DROP = {"$comment", "not", "propertyNames", "format", "if", "then", "else", "$schema", "$id", "title"}


def clean(node):
    if isinstance(node, list):
        return [clean(x) for x in node]
    if not isinstance(node, dict):
        return node
    out = {}
    for k, v in node.items():
        if k in DROP:
            continue
        if k == "$ref":
            out[k] = "#/definitions/" + v.split("/")[-1]
            continue
        if k == "allOf":
            kept = [clean(x) for x in v if not (isinstance(x, dict) and ("if" in x or "not" in x))]
            kept = [x for x in kept if x]
            if kept:
                out[k] = kept
            continue
        out[k] = clean(v)
    return out


defs = {}
for path in sorted(glob.glob(f"{ROOT}/schemas/*.json")):
    for name, node in json.load(open(path))["$defs"].items():
        if name in defs:
            sys.exit(f"duplicate def name {name} in {path}")
        defs[name] = clean(node)

manifest = json.load(open(f"{ROOT}/MANIFEST.json"))
contracts = [c["def"] for c in manifest["contracts"]]
bundle = {
    "$schema": "http://json-schema.org/draft-07/schema#",
    "title": "AllternitKernelAbi",
    "description": "Codegen bundle of Allternit Kernel ABI 1.0.0. Not normative; see schemas/.",
    "definitions": defs,
}
out = sys.argv[1]
json.dump(bundle, open(out, "w"), indent=2)
print(f"bundled {len(defs)} defs ({len(contracts)} contracts) -> {out}")
