import importlib.util
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "acu_vision_providers",
    Path(__file__).resolve().parents[1] / "core" / "vision_providers.py",
)
vp = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
_spec.loader.exec_module(vp)


def test_gizzi_runtime_base_strips_v1():
    assert vp.gizzi_runtime_base("http://127.0.0.1:4096/v1") == "http://127.0.0.1:4096"
    assert vp.gizzi_runtime_base("http://127.0.0.1:4096") == "http://127.0.0.1:4096"


def test_parse_platform_model():
    assert vp.parse_platform_model("local-mlx/qwen3.6-35b-a3b-4bit") == (
        "local-mlx",
        "qwen3.6-35b-a3b-4bit",
    )
    assert vp.parse_platform_model("claude-cli/claude-sonnet-4-6") == (
        "claude-cli",
        "claude-sonnet-4-6",
    )


def test_parse_platform_model_rejects_bare_id():
    try:
        vp.parse_platform_model("gpt-4o")
        assert False, "expected VisionConfigError"
    except vp.VisionConfigError as err:
        assert "provider/model" in str(err)


def test_plan_from_gizzi_structured():
    plan = vp.plan_from_gizzi_message(
        {
            "info": {
                "structured": {
                    "reasoning": "screen is visible",
                    "plan_steps": [],
                    "immediate_action": {"type": "screenshot", "target": "screen", "reason": "done"},
                    "done": True,
                    "confidence": 0.9,
                }
            },
            "parts": [],
        }
    )
    assert plan.done is True
    assert plan.immediate_action.type == "screenshot"


def test_plan_from_gizzi_text_parts():
    plan = vp.plan_from_gizzi_message(
        {
            "info": {},
            "parts": [
                {
                    "type": "text",
                    "text": '{"reasoning":"ok","immediate_action":{"type":"click","target":"ok","reason":"go"},"done":false}',
                }
            ],
        }
    )
    assert plan.done is False
    assert plan.immediate_action.type == "left_click"
    assert plan.immediate_action.target == "ok"
