"""A 423 from the executor means nobody took control: the run must stop, not retry."""
from core.toolset_executor import ComputerControlRequired, ToolsetReply


def test_423_reply_needs_control():
    reply = ToolsetReply(423, {"is_error": True, "content": [{"type": "text", "text": "Take control of this computer first."}]})
    assert reply.needs_control
    assert not reply.ok
    assert reply.text().startswith("Take control")


def test_other_failures_do_not_need_control():
    assert not ToolsetReply(500, {"is_error": True}).needs_control
    assert not ToolsetReply(200, {}).needs_control


def test_control_required_is_a_runtime_error():
    assert issubclass(ComputerControlRequired, RuntimeError)
