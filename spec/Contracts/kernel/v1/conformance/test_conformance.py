"""Conformance tests for Allternit Kernel ABI Package 1.0.0 (frozen 2026-09-30).

Run: uvx --with jsonschema --with referencing pytest spec/Contracts/kernel/v1/conformance -q
"""
import copy
import glob
import json
import os
import re

import pytest
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HERE = os.path.dirname(os.path.abspath(__file__))
BASE = "https://schemas.allternit.com/kernel/1.0.0/"


def load(path):
    with open(path) as f:
        return json.load(f)


SCHEMAS = {os.path.basename(p): load(p) for p in sorted(glob.glob(f"{ROOT}/schemas/*.json"))}
REGISTRY = Registry().with_resources((s["$id"], Resource.from_contents(s)) for s in SCHEMAS.values())
MANIFEST = load(f"{ROOT}/MANIFEST.json")
CONTRACTS = [(c["file"], c["def"]) for c in MANIFEST["contracts"]]
VALID = f"{HERE}/examples/valid"
INVALID = f"{HERE}/examples/invalid"


def validator(file, name):
    return Draft202012Validator(
        {"$ref": f"{BASE}{file}#/$defs/{name}"},
        registry=REGISTRY,
        format_checker=Draft202012Validator.FORMAT_CHECKER,
    )


def is_valid(file, name, inst):
    return not list(validator(file, name).iter_errors(inst))


def def_file(name):
    return next(f for f, d in CONTRACTS if d == name)


# --- package shape ---------------------------------------------------------

def test_manifest_frozen():
    assert MANIFEST["package_version"] == "1.0.0"
    assert MANIFEST["status"] == "FROZEN"
    assert len(SCHEMAS) == 21
    assert len(CONTRACTS) == 65
    assert len(set(CONTRACTS)) == 65
    for f, d in CONTRACTS:
        assert d in SCHEMAS[f]["$defs"], (f, d)


@pytest.mark.parametrize("name", sorted(SCHEMAS))
def test_schema_is_valid_2020_12(name):
    Draft202012Validator.check_schema(SCHEMAS[name])
    assert SCHEMAS[name]["$id"] == BASE + name
    assert "draft" not in SCHEMAS[name]["title"].lower()


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


def test_closed_objects_carry_extensions():
    """09 §E Q3: closed contracts + x-* extensions map."""
    for f, s in SCHEMAS.items():
        for k, v in s["$defs"].items():
            if k == "ExtensionMap" or not isinstance(v, dict) or "properties" not in v:
                continue
            assert v.get("additionalProperties") is False, f"{f}#{k} is open"
            assert "extensions" in v["properties"], f"{f}#{k} lacks extensions"


# --- examples --------------------------------------------------------------

@pytest.mark.parametrize("file,name", CONTRACTS)
def test_valid_example_passes(file, name):
    inst = load(f"{VALID}/{name}.json")
    errs = [e.message for e in validator(file, name).iter_errors(inst)]
    assert not errs, errs[:3]
    # x-* extensions are preserved/accepted; non-x keys are not
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
    """09 §C #5: adding enum members is non-breaking only because receivers fail closed."""
    inst = load(f"{VALID}/{name}.json")
    for k in _enum_props(file, name):
        if k in inst:
            bad = copy.deepcopy(inst)
            bad[k] = "ZZ_UNKNOWN_FUTURE_VALUE"
            assert not is_valid(file, name, bad), k


def test_decision_request_unknown_op_fails_closed():
    inst = load(f"{VALID}/DecisionRequestV1.json")
    ops = [k for k in _enum_props("decision.schema.json", "DecisionRequestV1")]
    assert ops, "DecisionRequestV1 has no enum-typed op field"
    for k in ops:
        bad = copy.deepcopy(inst)
        bad[k] = "TENTH_OP"
        assert not is_valid("decision.schema.json", "DecisionRequestV1", bad)


@pytest.mark.parametrize("file,name", CONTRACTS)
def test_unknown_field_fails_closed(file, name):
    inst = load(f"{VALID}/{name}.json")
    inst["future_required_field"] = True
    assert not is_valid(file, name, inst)
    if isinstance(inst.get("envelope"), dict):
        env = load(f"{VALID}/{name}.json")
        env["envelope"]["future_required_field"] = True
        assert not is_valid(file, name, env)


# --- encodings (Q4) --------------------------------------------------------

def test_encodings():
    c = "common.schema.json"
    ok_hash = "sha256:" + "0" * 64
    hv = Draft202012Validator({"$ref": f"{BASE}{c}#/$defs/ContentHash"}, registry=REGISTRY)
    assert hv.is_valid(ok_hash)
    assert not hv.is_valid("0" * 64)  # seed bare hex rejected
    tv = Draft202012Validator(
        {"$ref": f"{BASE}{c}#/$defs/Timestamp"}, registry=REGISTRY,
        format_checker=Draft202012Validator.FORMAT_CHECKER)
    assert tv.is_valid("2026-09-30T00:00:00Z")
    assert not tv.is_valid(1727654400.5)  # seed unix float rejected
    av = Draft202012Validator({"$ref": f"{BASE}{c}#/$defs/AbiVersion"}, registry=REGISTRY)
    assert av.is_valid("1.0.0") and not av.is_valid("1.0.0-draft")


# --- vendor neutrality -----------------------------------------------------

VENDOR = ["jev", "anyjev", "gliner", "clm", "raven", "openai", "anthropic", "claude",
          "codex", "qwen", "gpt", "gemini", "llama", "mistral", "deepseek", "sonnet"]


@pytest.mark.parametrize("path", sorted(
    glob.glob(f"{ROOT}/schemas/*.json") + glob.glob(f"{ROOT}/registry/*.json")
    + glob.glob(f"{ROOT}/data/*.json")))
def test_no_vendor_or_model_names(path):
    text = open(path).read().lower()
    hits = [v for v in VENDOR if v in text]
    assert not hits, (os.path.basename(path), hits)


# --- data + registry -------------------------------------------------------

def test_bug_fix_completion_policy():
    pol = load(f"{ROOT}/data/completion_policy.bug_fix.v1.json")
    assert is_valid("completion.schema.json", "CompletionPolicyV1", pol)
    assert pol["allow_partial"] is False
    assert len(pol["require"]) == 5 and all(r["blocking"] for r in pol["require"])
    crit = {c["criterion_id"] for c in load(f"{ROOT}/data/completion_criteria.builtin.v1.json")}
    assert {r["criterion_id"] for r in pol["require"]} <= crit


def test_builtin_criteria_valid():
    for c in load(f"{ROOT}/data/completion_criteria.builtin.v1.json"):
        assert is_valid("completion.schema.json", "CompletionCriterionV1", c), c["criterion_id"]


def test_primitive_registry():
    reg = load(f"{ROOT}/registry/primitives.json")
    assert is_valid("primitive.schema.json", "PrimitiveRegistryV1", reg)
    ids = [p["id"] for p in reg["primitives"]]
    assert reg["count"] == len(ids) == len(set(ids)) == 191
    legend = {k for k in reg["class_legend"] if not k.startswith("_")}
    assert legend == set("DSGVPMR")
    for p in reg["primitives"]:
        assert p["class"] is None or set(p["class"]) <= legend, p
        assert not re.match(r"^(gen|vcs)\.", p["id"]), p["id"]  # Q2


# --- AgentState (Q2/Q5) ----------------------------------------------------

def test_agent_state_split():
    props = SCHEMAS["agent_state.schema.json"]["$defs"]["AgentStateV1"]["properties"]
    for k in props:
        if k == "user_visible_status":
            continue  # display-only, non-authoritative (Q5)
        for bad in ("lifecycle", "lease", "campaign", "lock", "status", "wake"):
            assert bad not in k, k
    assert "budgets" in props and "work_ref" in props


# --- ported / inverted v1.4 reference tests --------------------------------

def test_seed_bug_fix_graph_is_rejected():
    """Inverts v1.4 test_bugfix_graph_static_conformance: the seed graph must NOT pass."""
    g = load(f"{HERE}/fixtures/seed_v1_4_bug_fix.graph.json")
    assert not is_valid("graph.schema.json", "ComputeGraphIRV1", g)
    pid = Draft202012Validator({"$ref": f"{BASE}common.schema.json#/$defs/PrimitiveId"}, registry=REGISTRY)
    gen = [n["primitive_id"] for n in g["nodes"] if n["primitive_id"].startswith("gen.")]
    assert gen and not any(pid.is_valid(x) for x in gen)


def test_capability_ids_are_cap_namespaced():
    """Ports test_cognitive_execution_integration capability check, with the 02 §4 form."""
    cv = Draft202012Validator({"$ref": f"{BASE}common.schema.json#/$defs/CapabilityId"}, registry=REGISTRY)
    assert cv.is_valid("cap.decision.tool_selection")
    assert not cv.is_valid("decision.tool_selection")


def test_completion_proposal_has_no_authority():
    """S1 may propose completion; only SYSTEM decides (rejects seed dec.assess_completion edge)."""
    props = SCHEMAS["completion.schema.json"]["$defs"]["CompletionProposalV1"]["properties"]
    assert not {"outcome", "decided_by", "decision"} & set(props)
    dec = SCHEMAS["completion.schema.json"]["$defs"]["CompletionDecisionV1"]["properties"]
    assert dec["decided_by"] == {"const": "SYSTEM"}


# --- freeze gates (Eoj, 2026-09-30) ----------------------------------------

EXPECTED_IDS = {name: BASE + name for name in SCHEMAS}


def test_stable_ids_pinned():
    """Gate 5: every $id is https://schemas.allternit.com/kernel/1.0.0/<file>."""
    assert len(EXPECTED_IDS) == 21
    for name, s in SCHEMAS.items():
        assert s["$id"] == f"https://schemas.allternit.com/kernel/1.0.0/{name}"


def _keys_and_refs(node):
    if isinstance(node, dict):
        for k, v in node.items():
            if k in ("$comment", "description"):
                continue
            yield k
            if k == "$ref":
                yield v
            yield from _keys_and_refs(v)
    elif isinstance(node, list):
        for v in node:
            yield from _keys_and_refs(v)


def test_compute_graph_ir_is_the_only_workflow_definition():
    """Gate 2: no WIH DAG type anywhere; ComputeGraphIRV1 is the only nodes+edges workflow."""
    for name, s in SCHEMAS.items():
        for k in _keys_and_refs(s):
            assert "wih" not in k.lower(), (name, k)
    workflows = [(f, d) for f, s in SCHEMAS.items() for d, v in s["$defs"].items()
                 if isinstance(v, dict) and {"nodes", "edges"} <= set(v.get("properties", {}))]
    assert workflows == [("graph.schema.json", "ComputeGraphIRV1")]


def test_completion_decision_producer_is_system_or_verifier():
    """Gate 3: S1/S2 (decision backends, model components) cannot produce a CompletionDecision."""
    base = load(f"{VALID}/CompletionDecisionV1.json")
    for kind, ok in [("RUNTIME", True), ("VERIFIER", True), ("DECISION_BACKEND", False),
                     ("MODEL_COMPONENT", False), ("PRIMITIVE", False), ("ADAPTER", False)]:
        inst = copy.deepcopy(base)
        inst["envelope"]["producer"]["kind"] = kind
        assert is_valid("completion.schema.json", "CompletionDecisionV1", inst) is ok, kind


def test_decision_gate_cannot_masquerade_as_policy():
    """Gate 4: decision outputs share no values with policy verdicts, and are never accepted as one."""
    d = SCHEMAS["decision.schema.json"]["$defs"]["DecisionResultV1"]["properties"]
    p = SCHEMAS["policy.schema.json"]["$defs"]["PolicyDecisionV1"]["properties"]["decision"]["enum"]
    assert not set(d["threshold_action"]["enum"]) & set(p)
    res = load(f"{VALID}/DecisionResultV1.json")
    gate = copy.deepcopy(res)
    gate["operation"] = "GATE"
    for verdict in p:
        gate["answer"] = verdict
        assert not is_valid("decision.schema.json", "DecisionResultV1", gate), verdict
    gate["answer"] = {"transition": "advance"}
    assert is_valid("decision.schema.json", "DecisionResultV1", gate)
    for f, name in [("policy.schema.json", "PolicyDecisionV1"), ("receipts.schema.json", "PolicyReceiptV1"),
                    ("policy.schema.json", "PolicyCheckV1")]:
        assert not is_valid(f, name, res)
    # nothing that refs PolicyDecisionV1 also admits DecisionResultV1
    def alts(node):
        if isinstance(node, dict):
            for k in ("anyOf", "oneOf"):
                if k in node:
                    refs = {b.get("$ref", "").split("/")[-1] for b in node[k] if isinstance(b, dict)}
                    assert not {"PolicyDecisionV1", "DecisionResultV1"} <= refs
            for v in node.values():
                alts(v)
        elif isinstance(node, list):
            for v in node:
                alts(v)
    alts(SCHEMAS)


RUN_CONTRACTS = ["AgentStateV1", "WorkNodeLifecycleV1", "LeaseV1", "NodeOutputV1", "CampaignV1",
                 "WorkRunRecordV1", "ActionReceiptV1", "PolicyReceiptV1", "RunReceiptV1"]


def test_q2_single_owner_per_mutable_field():
    """Gate 1: a representative Agency run spanning AgentState + Work Runtime + receipts
    round-trips, and no mutable field has two authoritative owners (MANIFEST `ownership`)."""
    run = {name: load(f"{VALID}/{name}.json") for name in RUN_CONTRACTS}
    back = json.loads(json.dumps(run, sort_keys=True))
    assert back == run
    for name, inst in back.items():
        assert is_valid(def_file(name), name, inst), name
    own = {k: v for k, v in MANIFEST["ownership"].items() if not k.startswith("_")}
    owners = {}
    for concept, o in own.items():
        props = SCHEMAS[def_file(o["owner"])]["$defs"][o["owner"]]["properties"]
        for fld in o["fields"]:
            assert fld in props, (o["owner"], fld)
            path = f"{o['owner']}.{fld}"
            assert path not in owners, (path, owners.get(path), concept)
            owners[path] = concept
    # the same concept is never owned by two contracts
    assert len({o["owner"] for o in own.values() if o["owner"] != "AgentStateV1"}) == len(own) - 2
    # owned work-ledger fields never reappear on AgentState; receipts own nothing mutable
    agent = set(SCHEMAS["agent_state.schema.json"]["$defs"]["AgentStateV1"]["properties"])
    for o in own.values():
        if o["owner"] != "AgentStateV1":
            assert not agent & set(o["fields"]) - {"objective"}, (o["owner"], agent & set(o["fields"]))
    assert not any(k.startswith(("ActionReceipt", "PolicyReceipt", "RunReceipt")) for k in owners)
    # cross-store links are refs, never embedded copies
    common = SCHEMAS["common.schema.json"]["$defs"]
    ref_types = {"Id", "StateVersionRef", "ContentHash"}
    for name in RUN_CONTRACTS:
        props = SCHEMAS[def_file(name)]["$defs"][name]["properties"]
        for k, v in props.items():
            if k.endswith("_ref") or (k.endswith("_id") and k != "schema_id"):
                refs = {r.split("/")[-1] for r in _refs(v)}
                scalar = "properties" not in json.dumps(v) and '"object"' not in json.dumps(v)
                assert (refs & ref_types) or (not refs and scalar), (name, k, v)
    assert common["StateVersionRef"]


def test_manifest_hashes_match_files():
    """Gate 6: MANIFEST lists sha256 of every schema/registry/data/generated file, and they match."""
    import hashlib
    files = MANIFEST["files"]
    expect = sorted(
        os.path.relpath(p, ROOT) for pat in ("schemas/*.json", "registry/*.json", "data/*.json",
                                             "generated/ts/*.d.ts", "generated/rust/Cargo.toml",
                                             "generated/rust/src/*.rs")
        for p in glob.glob(os.path.join(ROOT, pat)))
    assert sorted(files) == expect
    for rel, h in files.items():
        assert h == "sha256:" + hashlib.sha256(open(os.path.join(ROOT, rel), "rb").read()).hexdigest(), rel
