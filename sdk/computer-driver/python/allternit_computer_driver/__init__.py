"""Drive Allternit hosted computers from Claude, OpenAI computer use or Gemini computer use.

The Anthropic toolsets are in ``allternit_computer_driver.anthropic`` so importing this package
never needs the optional ``anthropic`` dependency."""

from .client import DEFAULT_BASE_URL, AllternitApiError, AllternitComputers, ApprovalRequiredError, result_image, result_text
from .gemini import gemini_keys, gemini_to_calls, run_gemini_call
from .openai import openai_keys, openai_to_calls, run_openai_action

__version__ = "0.1.0"

__all__ = [
    "AllternitComputers", "AllternitApiError", "ApprovalRequiredError", "DEFAULT_BASE_URL", "result_image", "result_text",
    "gemini_keys", "gemini_to_calls", "run_gemini_call", "openai_keys", "openai_to_calls", "run_openai_action",
]
