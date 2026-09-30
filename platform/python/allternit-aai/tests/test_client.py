import json

import httpx
import pytest

from allternit_aai import (AllternitAgents, ApprovalRequiredError, ConflictError, HumanIntentRequiredError, RateLimitedError)


def make(responses):
    calls = []

    def handler(req: httpx.Request) -> httpx.Response:
        calls.append(req)
        status, body = responses.pop(0) if responses else (200, {})
        return httpx.Response(status, json=body)

    return calls, AllternitAgents("http://x/", token="t", transport=httpx.MockTransport(handler))


def test_create_account_body_and_auth():
    calls, c = make([(201, {"account": {"id": "a1"}})])
    assert c.create_account("grok", "api_key", display_name="G")["account"]["id"] == "a1"
    assert str(calls[0].url) == "http://x/api/v1/gateway/provider-accounts"
    assert json.loads(calls[0].content) == {"vendor": "grok", "authType": "api_key", "displayName": "G"}
    assert calls[0].headers["authorization"] == "Bearer t"


def test_typed_errors():
    _, c = make([(428, {"error": "n", "approvalId": "ap1"}), (409, {"error": "x", "code": "REMOTE_CLOSED"}), (429, {"error": "s", "retryAfterMs": 500})])
    with pytest.raises(ApprovalRequiredError) as e1:
        c.send_turn("s", "hi")
    assert e1.value.approval_id == "ap1"
    with pytest.raises(ConflictError) as e2:
        c.send_turn("s", "hi")
    assert e2.value.code == "REMOTE_CLOSED"
    with pytest.raises(RateLimitedError) as e3:
        c.send_turn("s", "hi")
    assert e3.value.retry_after_ms == 500


def test_respond_requires_human_intent():
    calls, c = make([(200, {"state": "approved"})])
    with pytest.raises(HumanIntentRequiredError):
        c.respond_approval("ap1", "approve")
    c.respond_approval("ap1", "approve", human_intent=True)
    assert json.loads(calls[0].content) == {"decision": "approve", "actor": {"type": "user"}}


def test_stream_events_cursor_and_backoff():
    ev = lambda n: {"id": f"e{n}", "sequence": n, "type": "t"}
    calls, c = make([(200, {"events": [ev(1), ev(2)]}), (502, {"error": "bad"}), (200, {"events": [ev(3)]}), (200, {"events": []})])
    sleeps = []
    got = [e["sequence"] for e in c.stream_events("th", max_idle_polls=1, sleep=sleeps.append)]
    assert got == [1, 2, 3]
    assert [r.url.params.get("after") for r in calls] == ["0", "2", "2", "3"]
    assert len(sleeps) == 1


def test_stream_surfaces_4xx():
    _, c = make([(404, {"error": "nf"})])
    with pytest.raises(Exception) as e:
        list(c.stream_events("th"))
    assert e.value.status == 404
