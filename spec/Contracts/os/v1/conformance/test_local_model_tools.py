"""On-device model tool-call conformance: can a local model call `shell` and `inbox`?

Two parts:
  * Fixture checks (always run, no model): every expected call validates against the os/v1
    schemas, and an `approve` against `inbox` or `shell` does NOT validate (the contract has no
    approve action, so a model that emits one is wrong).
  * Live check (manual, Desktop-local): sends each prompt to the Desktop local engine and
    validates the tool call it returns. Needs a downloaded model, so CI skips it.

Live run (Allternit Desktop running with a tool-capable model loaded):
  ALLTERNIT_LOCAL_MODEL=<model id> [LOCAL_ENGINE_URL=http://127.0.0.1:3015] \
  uvx --with jsonschema --with referencing --with rfc3339-validator \
      pytest spec/Contracts/os/v1/conformance/test_local_model_tools.py -q -k live
"""
import json
import os
import urllib.request

import pytest

from test_conformance import HERE, SCHEMAS, is_valid, load

FIXTURE = load(f"{HERE}/local_model/fixture.json")
CASES = {c["id"]: c for c in FIXTURE["cases"]}
SHELL_FILE, INBOX_FILE = "shell.schema.json", "inbox.schema.json"


def shell_command(args):
    """The harness fills the envelope fields a model should not have to invent."""
    return {"schema_id": "allternit.os.ShellCommandV1", "schema_version": "1.0.0", "command_id": "cmd_conf", **args}


def call_validates(tool, args):
    if tool == "shell":
        return is_valid(SHELL_FILE, "ShellCommandV1", shell_command(args))
    if tool == "inbox":
        return is_valid(INBOX_FILE, "InboxToolInputV1", args)
    return False


def matches(args, include):
    return all(args.get(k) == v for k, v in include.items())


def judge(case, calls):
    """Return a list of failures for the tool calls a model made on one case."""
    exp = case["expect"]
    if exp.get("refusal"):
        bad = [c for c in calls if any(c["tool"] == f["tool"] and matches(c["arguments"], f["args_include"]) for f in exp["forbid"])]
        return [f"emitted forbidden call {c}" for c in bad]
    if not calls:
        return ["no tool call"]
    c = calls[0]
    if c["tool"] != exp["tool"]:
        return [f"expected tool {exp['tool']}, got {c['tool']}"]
    if not call_validates(c["tool"], c["arguments"]):
        return [f"arguments do not validate as {exp['schema']}: {c['arguments']}"]
    if not matches(c["arguments"], exp["args_include"]):
        return [f"arguments missing {exp['args_include']}: {c['arguments']}"]
    return []


def tool_defs():
    return [
        {"type": "function", "function": {
            "name": "shell", "description": "Open, close, focus or arrange views in the Allternit shell.",
            "parameters": {"type": "object", "required": ["action"], "additionalProperties": False, "properties": {
                "action": {"type": "string", "enum": SCHEMAS[SHELL_FILE]["$defs"]["ShellAction"]["enum"]},
                "view_id": {"type": "string"}, "params": {"type": "object"}}}}},
        {"type": "function", "function": {
            "name": "inbox", "description": "Read the owner's inbox. There is no approve action.",
            "parameters": {"type": "object", "required": ["action"], "properties": {
                "action": {"type": "string", "enum": ["list", "summarise", "snooze", "prepare"]},
                "scope": {"type": "string", "enum": ["needs_you", "snoozed", "all"]}, "limit": {"type": "integer"},
                "ids": {"type": "array", "items": {"type": "string"}}, "id": {"type": "string"},
                "until": {"type": "string"}, "draft": {"type": "object"}}}}},
    ]


# --- fixture checks (no model) ----------------------------------------------

def test_fixture_cases_present():
    assert set(CASES) == {"shell-open-notes", "inbox-list-needs-you", "inbox-no-approve"}


@pytest.mark.parametrize("cid", ["shell-open-notes", "inbox-list-needs-you"])
def test_sample_calls_pass_the_judge(cid):
    case = CASES[cid]
    assert judge(case, [case["sample_call"]]) == []


def test_inbox_list_is_valid_and_approve_is_not():
    assert call_validates("inbox", {"action": "list", "scope": "needs_you"})
    assert not call_validates("inbox", {"action": "approve", "id": "apr_01"})
    assert not call_validates("shell", {"action": "approve"})
    assert "approve" not in SCHEMAS[SHELL_FILE]["$defs"]["ShellAction"]["enum"]


def test_refusal_case_fails_a_model_that_approves():
    case = CASES["inbox-no-approve"]
    assert judge(case, []) == []
    assert judge(case, [{"tool": "inbox", "arguments": {"action": "approve", "id": "apr_01"}}])
    assert judge(case, [{"tool": "inbox", "arguments": {"action": "list", "scope": "all"}}]) == []


def test_wrong_tool_and_bad_args_are_caught():
    case = CASES["shell-open-notes"]
    assert judge(case, [])
    assert judge(case, [{"tool": "inbox", "arguments": {"action": "list", "scope": "all"}}])
    assert judge(case, [{"tool": "shell", "arguments": {"action": "teleport"}}])


# --- live check (manual: needs Desktop local engine + a model) ----------------

def ask_engine(model, prompt):
    url = os.environ.get("LOCAL_ENGINE_URL", "http://127.0.0.1:3015").rstrip("/") + "/v1/chat/completions"
    body = {"model": model, "temperature": 0, "tools": tool_defs(),
            "messages": [{"role": "system", "content": FIXTURE["system"]}, {"role": "user", "content": prompt}]}
    req = urllib.request.Request(url, json.dumps(body).encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        msg = json.load(r)["choices"][0]["message"]
    calls = []
    for tc in msg.get("tool_calls") or []:
        fn = tc["function"]
        args = fn["arguments"]
        calls.append({"tool": fn["name"], "arguments": json.loads(args) if isinstance(args, str) else args})
    return calls


@pytest.mark.skipif(not os.environ.get("ALLTERNIT_LOCAL_MODEL"),
                    reason="manual: set ALLTERNIT_LOCAL_MODEL (Desktop local engine + downloaded model)")
@pytest.mark.parametrize("cid", [c["id"] for c in FIXTURE["cases"]])
def test_live_local_model(cid):
    case = CASES[cid]
    calls = ask_engine(os.environ["ALLTERNIT_LOCAL_MODEL"], case["prompt"])
    assert judge(case, calls) == [], f"calls: {calls}"
