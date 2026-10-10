"""
Allternit Computer Use — Core Package

Execution layer for computer use automation.
- Base adapter interface and action types
- Unified action executor (ComputerUseExecutor)
- Vision providers for non-Claude models (GPT-4o, Gemini, Qwen, etc.)
- Action recording and GIF generation
"""

from .base_adapter import (
    BaseAdapter,
    ActionRequest,
    Artifact,
    PolicyDecision,
    ResultEnvelope,
    Receipt,
    AdapterCapabilities,
)

from .computer_use_executor import (
    ComputerUseExecutor,
    get_executor,
    NATIVE_CLAUDE_ACTIONS,
    BROWSER_EXTENSION_ACTIONS,
    ALL_SUPPORTED_ACTIONS,
    ADAPTER_WATERFALL,
)

from .multi_model_tools import (
    get_computer_tool,
    decode_tool_call,
    OPENAI_COMPUTER_TOOL,
    ANTHROPIC_COMPUTER_TOOL,
    GEMINI_COMPUTER_TOOL,
)

from .vision_providers import (
    VisionProvider,
    AnthropicVisionClient,
    MockVisionClient,
    VisionProviderFactory,
    VisionResponse,
    VisionElement,
    VisionAction,
    ProviderType,
    VisionProviderError,
    VisionAPIError,
    VisionConfigError,
    get_vision_provider,
)

__all__ = [
    # Base Adapter
    "BaseAdapter",
    "ActionRequest",
    "Artifact",
    "PolicyDecision",
    "ResultEnvelope",
    "Receipt",
    "AdapterCapabilities",
    # Executor
    "ComputerUseExecutor",
    "get_executor",
    "NATIVE_CLAUDE_ACTIONS",
    "BROWSER_EXTENSION_ACTIONS",
    "ALL_SUPPORTED_ACTIONS",
    "ADAPTER_WATERFALL",
    # Multi-model tool definitions (non-Claude path)
    "get_computer_tool",
    "decode_tool_call",
    "OPENAI_COMPUTER_TOOL",
    "ANTHROPIC_COMPUTER_TOOL",
    "GEMINI_COMPUTER_TOOL",
    # Vision Providers (non-Claude model path)
    "VisionProvider",
    "AnthropicVisionClient",
    "MockVisionClient",
    "VisionProviderFactory",
    "VisionResponse",
    "VisionElement",
    "VisionAction",
    "ProviderType",
    "VisionProviderError",
    "VisionAPIError",
    "VisionConfigError",
    "get_vision_provider",
]

__version__ = "0.1.0"
