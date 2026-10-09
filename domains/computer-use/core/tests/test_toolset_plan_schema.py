"""The planner's action schema is generated from the toolset contract."""

from core.toolset_executor import plan_json_schema
from contracts.toolset_v1 import member_spec


def test_plan_schema_is_generated_from_the_contract():
    variants = plan_json_schema()["properties"]["immediate_action"]["anyOf"]
    left = next(v for v in variants if v["properties"]["member"]["const"] == "left_click"
                and v["properties"]["toolset"]["const"] == "computer")
    assert left["properties"]["input"] == member_spec("computer", "left_click")["input_schema"]
