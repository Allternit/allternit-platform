"""/v1/computer alias: the old Claude payload becomes one contract call."""

from core.toolset_executor import alias_v1_computer, plan_json_schema
from contracts.toolset_v1 import member_spec


def test_old_claude_payloads_map_to_contract_members():
    cases = [
        ({"action": "left_click", "coordinate": [10, 20]}, ("computer", "left_click", {"coordinate": [10.0, 20.0]})),
        ({"action": "click", "coordinate": [1, 2]}, ("computer", "left_click", {"coordinate": [1.0, 2.0]})),
        ({"action": "type", "text": "hi"}, ("computer", "type", {"text": "hi"})),
        ({"action": "key", "key": "cmd+a"}, ("computer", "key", {"text": "cmd+a"})),
        ({"action": "scroll", "coordinate": [5, 5], "delta": [0, -300]},
         ("computer", "scroll", {"coordinate": [5.0, 5.0], "scroll_direction": "up", "scroll_amount": 3})),
        ({"action": "screenshot"}, ("computer", "screenshot", {})),
        ({"action": "navigate", "url": "https://example.com"}, ("browser", "navigate", {"url": "https://example.com"})),
        ({"action": "goto", "url": "https://example.com"}, ("browser", "navigate", {"url": "https://example.com"})),
        ({"action": "made_up_verb"}, ("computer", "screenshot", {})),
    ]
    for payload, expected in cases:
        got = alias_v1_computer(payload)
        assert got == expected, (payload, got)
        assert member_spec(got[0], got[1]) is not None


def test_plan_schema_is_generated_from_the_contract():
    variants = plan_json_schema()["properties"]["immediate_action"]["anyOf"]
    left = next(v for v in variants if v["properties"]["member"]["const"] == "left_click"
                and v["properties"]["toolset"]["const"] == "computer")
    assert left["properties"]["input"] == member_spec("computer", "left_click")["input_schema"]
