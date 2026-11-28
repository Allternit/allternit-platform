"""Conformance tests for the AllternitOS contract package os/v1 (DRAFT, gate G0).

Sibling of the frozen Kernel ABI 1.0.0 (spec/Contracts/kernel/v1). ABI types are referenced
by $ref and resolved from the ABI's own schema files; nothing here redefines them.

Run: uvx --with jsonschema --with referencing --with rfc3339-validator pytest spec/Contracts/os/v1/conformance -q
"""
import copy
import glob
import hashlib
import json
import os

import pytest
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HERE = os.path.dirname(os.path.abspath(__file__))
ABI_ROOT = os.path.normpath(os.path.join(ROOT, "..", "..", "kernel", "v1"))
BASE = "https://schemas.allternit.com/os/1.0.0/"
ABI_BASE = "https://schemas.allternit.com/kernel/1.0.0/"


def load(path):
    with open(path) as f:
        return json.load(f)


SCHEMAS = {os.path.basename(p): load(p) for p in sorted(glob.glob(f"{ROOT}/schemas/*.json"))}
ABI_SCHEMAS = {os.path.basename(p): load(p) for p in sorted(glob.glob(f"{ABI_ROOT}/schemas/*.json"))}
REGISTRY = Registry().with_resources(
    (s["$id"], Resource.from_contents(s)) for s in list(SCHEMAS.values()) + list(ABI_SCHEMAS.values())
)
MANIFEST = load(f"{ROOT}/MANIFEST.json")
CONTRACTS = [(c["file"], c["def"]) for c in MANIFEST["contracts"]]
KIND = {c["def"]: c["kind"] for c in MANIFEST["contracts"]}
VALID = f"{HERE}/examples/valid"
INVALID = f"{HERE}/examples/invalid"


def validator(file, name, base=BASE):
    return Draft202012Validator(
        {"$ref": f"{base}{file}#/$defs/{name}"},
        registry=REGISTRY,
        format_checker=Draft202012Validator.FORMAT_CHECKER,
    )


def is_valid(file, name, inst, base=BASE):
    return not list(validator(file, name, base).iter_errors(inst))


def enum_of(file, name):
    return set(SCHEMAS[file]["$defs"][name]["enum"])


# --- package shape ---------------------------------------------------------

def test_manifest_shape():
    assert MANIFEST["package"] == "allternit-os-contracts"
    assert MANIFEST["package_version"] == "1.0.0"
    assert MANIFEST["status"] in ("DRAFT", "FROZEN")
    assert sorted(SCHEMAS) == ["authority.schema.json", "inbox.schema.json", "shell.schema.json"]
    assert len(set(CONTRACTS)) == len(CONTRACTS)
    for f, d in CONTRACTS:
        assert d in SCHEMAS[f]["$defs"], (f, d)
    # every titled def is a listed contract, and the reverse
    titled = {(f, k) for f, s in SCHEMAS.items() for k, v in s["$defs"].items() if "title" in v}
    assert titled == set(CONTRACTS)


@pytest.mark.parametrize("name", sorted(SCHEMAS))
def test_schema_is_valid_2020_12(name):
    Draft202012Validator.check_schema(SCHEMAS[name])
    assert SCHEMAS[name]["$id"] == BASE + name


def _refs(node):
    if isinstance(node, dict):
        if isinstance(node.get("$ref"), str):
            yield node["$ref"]
        for v in node.values():
            yield from _refs(v)
    elif isinstance(node, list):
        for v in node:
            yield from _refs(v)


@pytest.mark.parametrize("name", sorted(SCHEMAS))
def test_all_refs_resolve(name):
    resolver = REGISTRY.resolver(base_uri=SCHEMAS[name]["$id"])
    for ref in _refs(SCHEMAS[name]):
        resolver.lookup(ref)  # raises Unresolvable


def test_abi_types_are_referenced_not_redefined():
    abi_defs = {k for s in ABI_SCHEMAS.values() for k in s["$defs"]}
    ours = {k for s in SCHEMAS.values() for k in s["$defs"]}
    assert not (abi_defs & ours), abi_defs & ours
    refs = [r for s in SCHEMAS.values() for r in _refs(s)]
    for t in ("PolicyCheckV1", "TaskIRV1", "ExtensionMap", "Id", "Timestamp"):
        assert any(r.startswith(ABI_BASE) and r.endswith(t) for r in refs), t


def test_frozen_abi_untouched():
    """This package must never edit a frozen ABI file: the ABI MANIFEST hashes still match."""
    abi = load(f"{ABI_ROOT}/MANIFEST.json")
    assert abi["status"] == "FROZEN"
    for rel, digest in abi["files"].items():
        with open(os.path.join(ABI_ROOT, rel), "rb") as f:
            assert "sha256:" + hashlib.sha256(f.read()).hexdigest() == digest, rel


def test_manifest_file_hashes():
    for rel, digest in MANIFEST["files"].items():
        with open(os.path.join(ROOT, rel), "rb") as f:
            assert "sha256:" + hashlib.sha256(f.read()).hexdigest() == digest, rel
    for p in glob.glob(f"{ROOT}/schemas/*.json"):
        assert os.path.relpath(p, ROOT) in MANIFEST["files"], p


def _closed_objects(node, path):
    if isinstance(node, dict):
        if node.get("type") == "object" and "properties" in node:
            yield path, node
        for k, v in node.items():
            yield from _closed_objects(v, f"{path}/{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            yield from _closed_objects(v, f"{path}/{i}")


def test_objects_are_closed_and_types_carry_extensions():
    """ABI Q3: closed objects (unknown fields fail closed). CONTRACT.md 3: every type carries extensions."""
    for f, s in SCHEMAS.items():
        for path, obj in _closed_objects(s["$defs"], f):
            if "/then/" in path or "/if/" in path:
                continue
            assert obj.get("additionalProperties") is False, f"{path} is open"
    for f, d in CONTRACTS:
        if KIND[d] == "type":
            node = SCHEMAS[f]["$defs"][d]
            assert "extensions" in node["properties"], d
            assert node["properties"]["schema_id"] == {"const": f"allternit.os.{d}"}


# --- examples --------------------------------------------------------------

@pytest.mark.parametrize("file,name", CONTRACTS)
def test_valid_examples_pass(file, name):
    paths = [f"{VALID}/{name}.json"] + glob.glob(f"{VALID}/{name}.*.json")
    for p in paths:
        errs = [e.message for e in validator(file, name).iter_errors(load(p))]
        assert not errs, (p, errs[:3])
    if KIND[name] == "type":
        inst = load(paths[0])
        ok = copy.deepcopy(inst)
        ok["extensions"] = {"x-vendor-note": {"any": 1}}
        assert is_valid(file, name, ok)
        bad = copy.deepcopy(inst)
        bad["extensions"] = {"note": 1}
        assert not is_valid(file, name, bad)


@pytest.mark.parametrize("file,name", CONTRACTS)
def test_invalid_examples_fail(file, name):
    paths = glob.glob(f"{INVALID}/{name}.*.json")
    assert paths, f"no invalid example for {name}"
    for p in paths:
        assert not is_valid(file, name, load(p)), p


def _enum_props(file, name):
    node = SCHEMAS[file]["$defs"][name]
    resolver = REGISTRY.resolver(base_uri=BASE + file)
    for k, v in node.get("properties", {}).items():
        if "$ref" in v:
            v = resolver.lookup(v["$ref"]).contents
        if isinstance(v, dict) and "enum" in v:
            yield k


@pytest.mark.parametrize("file,name", CONTRACTS)
def test_unknown_enum_value_fails_closed(file, name):
    inst = load(f"{VALID}/{name}.json")
    if not isinstance(inst, dict):
        return
    for k in _enum_props(file, name):
        if k in inst:
            bad = copy.deepcopy(inst)
            bad[k] = "zz_unknown_future_value"
            assert not is_valid(file, name, bad), k


# --- contract rules (CONTRACT.md) -------------------------------------------

def test_action_classes_and_human_only():
    a = "authority.schema.json"
    assert enum_of(a, "ActionClass") == {
        "read", "ui_layout", "run_control", "file_write", "network", "app_install", "inbox_prepare",
        "money", "client_message", "deploy", "secret_new_host", "irreversible"}
    assert enum_of(a, "HumanOnlyActionClass") == {"money", "client_message", "deploy", "secret_new_host", "irreversible"}
    assert enum_of(a, "AutonomyLevel") == {"ask_first", "ask_risky", "full"}


def test_inbox_tool_has_no_approve():
    """CONTRACT.md 7.2: the model's inbox tool can never approve, deny or resolve."""
    assert enum_of("inbox.schema.json", "InboxAction") == {"list", "summarise", "snooze", "prepare"}
    for verb in ("approve", "deny", "resolve"):
        assert not is_valid("inbox.schema.json", "InboxToolInputV1", {"action": verb, "id": "apr_01"})


@pytest.mark.parametrize("kind", ["run", "service"])
def test_resolve_only_from_a_human_caller(kind):
    """CONTRACT.md 4.3: approval.resolve accepts caller.kind == human only."""
    req = load(f"{VALID}/KernelRequestV1.json")
    assert req["call"] == "approval.resolve" and is_valid("authority.schema.json", "KernelRequestV1", req)
    req["caller"]["kind"] = kind
    assert not is_valid("authority.schema.json", "KernelRequestV1", req)


def test_token_is_bound_to_a_payload_hash_and_single_use():
    tok = load(f"{VALID}/ApprovalTokenV1.json")
    for k in ("action_hash", "signature", "key_id", "expires_at"):
        bad = copy.deepcopy(tok)
        bad.pop(k)
        assert not is_valid("authority.schema.json", "ApprovalTokenV1", bad), k


def test_protected_and_app_views():
    vd = load(f"{VALID}/ViewDescriptorV1.json")
    assert vd["kind"] == "app" and vd["view_id"].startswith("app:")
    builtin = {k: v for k, v in vd.items() if k not in ("app_id", "app_version", "entry")}
    builtin.update(view_id="chat", kind="builtin")
    assert is_valid("shell.schema.json", "ViewDescriptorV1", builtin)


def test_open_conflict_os_v1_extension_key():
    """OPEN CONFLICT for G0 (README "Open conflicts" 1): CONTRACT.md 2.1/2.2/2.9 put os fields in
    extensions["os.v1"], but the frozen ABI ExtensionMap only accepts ^x- keys. This test pins
    today's fact so nobody wires it by accident; update it when G0 decides."""
    abi = load(f"{ABI_ROOT}/conformance/examples/valid/PolicyCheckV1.json")
    assert is_valid("policy.schema.json", "PolicyCheckV1", abi, ABI_BASE)
    abi["extensions"] = {"os.v1": load(f"{VALID}/CheckExtensionV1.json")}
    assert not is_valid("policy.schema.json", "PolicyCheckV1", abi, ABI_BASE)
