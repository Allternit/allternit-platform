"""Drive Allternit hosted computers from Claude, OpenAI computer use or Gemini computer use.

The Anthropic toolsets live in ``allternit_computer_driver.anthropic`` so importing this package
never needs the optional ``anthropic`` dependency."""

from .client import (
    DEFAULT_BASE_URL,
    AllternitApiError,
    AllternitComputers,
    ApprovalRequiredError,
    ComputerBusyError,
    ComputerConflictError,
    SandboxRequiredError,
    result_image,
    result_text,
)
from .gemini import gemini_computer_v2_declaration, gemini_keys, gemini_to_calls, run_gemini_call, run_gemini_v2_call
from .openai import openai_computer_v2_tool, openai_keys, openai_to_calls, run_openai_action, run_openai_v2_call
from .v2 import (
    COMPUTER_V2_MEMBER_NAMES,
    COMPUTER_V2_STRUCTURED_MEMBER_NAMES,
    COMPUTER_V2_TOOL_NAME,
    V2_TOOL_MARKER,
    ComputerV2Driver,
    ComputerV2Error,
    computer_v2_parameters,
    computer_v2_tool,
    computer_v2_tool_description,
    run_computer_v2_member,
)

__version__ = "0.2.0"

__all__ = [
    "AllternitComputers", "AllternitApiError", "ApprovalRequiredError", "ComputerBusyError",
    "ComputerConflictError", "SandboxRequiredError", "DEFAULT_BASE_URL", "result_image", "result_text",
    "gemini_keys", "gemini_to_calls", "run_gemini_call", "gemini_computer_v2_declaration", "run_gemini_v2_call",
    "openai_keys", "openai_to_calls", "run_openai_action", "openai_computer_v2_tool", "run_openai_v2_call",
    "COMPUTER_V2_MEMBER_NAMES", "COMPUTER_V2_STRUCTURED_MEMBER_NAMES", "COMPUTER_V2_TOOL_NAME", "V2_TOOL_MARKER",
    "ComputerV2Driver", "ComputerV2Error", "computer_v2_parameters", "computer_v2_tool",
    "computer_v2_tool_description", "run_computer_v2_member",
]
