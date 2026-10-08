"""
Allternit Computer Use — Vision Providers (Non-Claude Model Path)

Unified interface for vision model providers used in the non-Claude planning loop.
GPT-4o, Gemini, Qwen, and other models that lack native computer-use tool support
use these providers to perceive screen state during the Plan→Act→Observe→Reflect loop.

Claude's native computer tool does not use this module — Claude handles perception
natively through its own vision capability.

Supports: OpenAI GPT-4o, Anthropic Claude (as a provider option), Azure OpenAI.
"""

import os
import base64
import json
import asyncio
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple, Union
from enum import Enum
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


#: Used only when a screenshot's size can't be read. Cloud computers run 1920x1080.
FALLBACK_SCREEN_SIZE: Tuple[int, int] = (1920, 1080)


def image_size(data: Union[bytes, str, None]) -> Optional[Tuple[int, int]]:
    """(width, height) of a PNG or JPEG screenshot, from raw bytes or base64."""
    if not data:
        return None
    if isinstance(data, str):
        try:
            data = base64.b64decode(data.split(",", 1)[-1] if data.startswith("data:") else data)
        except Exception:
            return None
    if data[:8] == b"\x89PNG\r\n\x1a\n" and len(data) >= 24:
        return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")
    if data[:2] == b"\xff\xd8":
        i = 2
        while i + 9 < len(data):
            if data[i] != 0xFF:
                i += 1
                continue
            marker = data[i + 1]
            if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                return int.from_bytes(data[i + 7:i + 9], "big"), int.from_bytes(data[i + 5:i + 7], "big")
            i += 2 + int.from_bytes(data[i + 2:i + 4], "big")
    return None


def resolve_screen_size(
    screenshot: Union[bytes, str, None],
    screen_size: Optional[Tuple[int, int]] = None,
) -> Tuple[int, int]:
    """The coordinate space the model plans in: an explicit size from the
    session wins, else the screenshot's own pixels, else 1920x1080."""
    if screen_size and screen_size[0] > 0 and screen_size[1] > 0:
        return int(screen_size[0]), int(screen_size[1])
    return image_size(screenshot) or FALLBACK_SCREEN_SIZE


def _screenshot_bytes(screenshot: Union[bytes, str, None]) -> bytes:
    if not screenshot:
        return b""
    if isinstance(screenshot, bytes):
        return screenshot
    return base64.b64decode(screenshot)


class VisionProviderError(Exception):
    """Base exception for vision provider errors."""
    pass


class VisionAPIError(VisionProviderError):
    """Exception raised when vision API call fails."""
    def __init__(self, message: str, provider: str, status_code: Optional[int] = None):
        super().__init__(message)
        self.provider = provider
        self.status_code = status_code


class VisionConfigError(VisionProviderError):
    """Exception raised when configuration is invalid."""
    pass


class ProviderType(Enum):
    """Supported vision provider types."""
    UITARS = "uitars"       # UI-TARS skill via Allternit skills HTTP API (best for GUI grounding)
    ALLTERNIT = "allternit" # Platform Gizzi brain (`providerID/modelID` session), same as Home/Code
    SUBPROCESS = "subprocess" # CLI brain subprocess: claude, codex, gemini CLI, or custom command
    OPENAI = "openai"       # Direct OpenAI API key (dev mode)
    ANTHROPIC = "anthropic" # Direct Anthropic API key (dev mode)
    AZURE_OPENAI = "azure"  # Direct Azure OpenAI (dev mode)
    GEMINI = "gemini"       # Direct Google Gemini (dev mode)
    QWEN = "qwen"           # OpenAI-compat endpoint for Qwen-VL (Ollama/vLLM)
    SHOWUI = "showui"       # Local ShowUI 2B via HuggingFace transformers
    MOCK = "mock"           # Test-only — never used in production


@dataclass
class VisionElement:
    """Represents a detected UI element."""
    label: str
    bbox: List[float]  # [x1, y1, x2, y2] normalized 0-1
    confidence: float
    text: Optional[str] = None


@dataclass
class VisionAction:
    """Represents a recommended action."""
    type: str  # click, type, scroll, etc.
    target: str
    reason: str
    coordinates: Optional[List[float]] = None  # [x, y] for click actions
    text: Optional[str] = None               # text payload for type actions


@dataclass
class ActionPlan:
    reasoning: str                    # why this action
    plan_steps: List[str]             # high-level remaining steps
    immediate_action: VisionAction    # what to do right now
    confidence: float
    requires_approval: bool = False
    risk_level: str = "low"           # low | medium | high | critical
    reflection: Optional[str] = None  # filled after observation
    done: bool = False                # True when task is complete
    tokens_used: int = 0              # input+output tokens consumed by this plan call
    cost_usd: float = 0.0             # estimated cost for this plan call
    input_tokens: int = 0             # prompt tokens, when the provider reports the split
    output_tokens: int = 0            # completion tokens, when reported
    # Optional batch continuation: when the next actions are all groundable on
    # the same page in the whitelisted browser vocabulary (core/batch_dispatch.py),
    # a provider may emit them here so the planning loop ships one grant-bound
    # batch instead of step-by-step turns. ``immediate_action`` stays the first
    # step. Never required — the loop falls back to per-step when absent.
    batch: Optional[List["VisionAction"]] = None
    # Optional code-mode request (core/code_mode.py): a validated, grant-bound
    # code payload the loop may dispatch INSTEAD of immediate_action when the
    # run explicitly opted in (PlanningLoopConfig.code_mode_enabled). Shape:
    # {"language": "playwright-js", "code": str, "declaredTargets": [str]}.
    # Never the default mode; refused payloads surface as observations.
    code: Optional[Dict[str, Any]] = None


@dataclass
class VisionResponse:
    """Structured response from vision model."""
    elements: List[VisionElement]
    action: Optional[VisionAction]
    confidence: float
    raw_response: Optional[str] = None
    tokens_used: int = 0
    cost_usd: float = 0.0
    input_tokens: int = 0             # prompt tokens, when the provider reports the split
    output_tokens: int = 0            # completion tokens, when reported


class VisionProvider(ABC):
    """Abstract base class for vision providers."""
    
    def __init__(self, api_key: Optional[str] = None, **kwargs):
        self.api_key = api_key
        self.config = kwargs
    
    @abstractmethod
    def analyze_image(
        self,
        image_bytes: bytes,
        task: str,
        prompt_template: Optional[str] = None,
        **kwargs
    ) -> VisionResponse:
        """
        Analyze an image and return structured vision response.
        
        Args:
            image_bytes: Raw image bytes
            task: Description of the task to perform
            prompt_template: Optional custom prompt template
            **kwargs: Additional provider-specific parameters
            
        Returns:
            VisionResponse with elements, actions, and confidence
        """
        pass
    
    @abstractmethod
    def is_available(self) -> bool:
        """Check if the provider is properly configured and available."""
        pass

    async def ground_and_reason(
        self,
        screenshot_b64: str,
        task: str,
        history: Optional[List] = None,
        screen_size: Optional[Tuple[int, int]] = None,
        **kwargs,
    ) -> "ActionPlan":
        """
        Plan, ground, and reason about the next action.
        Default implementation decodes screenshot_b64 → analyze_image() and wraps result.
        Subclasses with native planning support should override.
        """
        import base64 as _b64
        screenshot_bytes = _b64.b64decode(screenshot_b64) if screenshot_b64 else b""
        response = self.analyze_image(screenshot_bytes, task)
        return ActionPlan(
            reasoning=response.raw_response or "Vision analysis",
            plan_steps=[task],
            immediate_action=response.action or VisionAction(type="screenshot", target="screen", reason="No action determined"),
            confidence=response.confidence,
            done=False,
            tokens_used=response.tokens_used,
            cost_usd=response.cost_usd,
            input_tokens=response.input_tokens,
            output_tokens=response.output_tokens,
        )

    async def analyze_screenshot(self, screenshot_b64: str, task: str, **kwargs) -> "VisionResponse":
        """Async wrapper around analyze_image for providers that don't override it."""
        import base64 as _b64
        image_bytes = _b64.b64decode(screenshot_b64) if screenshot_b64 else b""
        return self.analyze_image(image_bytes, task, **kwargs)

    def encode_image(self, image_bytes: bytes) -> str:
        """Encode image bytes to base64 string."""
        return base64.b64encode(image_bytes).decode('utf-8')
    
    def _parse_json_response(self, text: str) -> Dict[str, Any]:
        """Parse JSON from model response, handling markdown code blocks."""
        # Try to extract JSON from markdown code blocks
        if "```json" in text:
            json_text = text.split("```json")[1].split("```")[0].strip()
        elif "```" in text:
            json_text = text.split("```")[1].split("```")[0].strip()
        else:
            json_text = text.strip()
        
        try:
            return json.loads(json_text)
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON response: {e}")
            logger.debug(f"Raw response: {text}")
            raise VisionAPIError(
                f"Failed to parse JSON response: {e}",
                self.__class__.__name__
            )


class OpenAIVisionClient(VisionProvider):
    """OpenAI GPT-4o vision provider."""
    
    DEFAULT_MODEL = "gpt-4o"
    DEFAULT_MAX_TOKENS = 4096
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = DEFAULT_MODEL,
        max_tokens: int = DEFAULT_MAX_TOKENS,
        **kwargs
    ):
        super().__init__(api_key, **kwargs)
        self.model = model
        self.max_tokens = max_tokens
        self._client = None
        
        # Check if openai is installed
        try:
            import openai
            self._openai_module = openai
        except ImportError:
            logger.error("OpenAI package not installed. Install with: pip install openai")
            self._openai_module = None
    
    def _init_client(self):
        """Initialize OpenAI client with current configuration."""
        if self._openai_module and not self._client and self.api_key:
            self._client = self._openai_module.OpenAI(api_key=self.api_key)
    
    def is_available(self) -> bool:
        """Check if OpenAI client is configured."""
        if not self._openai_module:
            return False
        if not self.api_key:
            self.api_key = os.environ.get("OPENAI_API_KEY")
        if self.api_key:
            self._init_client()
        return bool(self.api_key and self._client)
    
    def analyze_image(
        self,
        image_bytes: bytes,
        task: str,
        prompt_template: Optional[str] = None,
        **kwargs
    ) -> VisionResponse:
        """Analyze image using GPT-4o."""
        if not self.is_available():
            raise VisionConfigError(
                "OpenAI client not available. Set OPENAI_API_KEY environment variable."
            )
        
        prompt = prompt_template or self._default_prompt(task)
        base64_image = self.encode_image(image_bytes)
        
        try:
            response = self._client.chat.completions.create(
                model=self.model,
                max_tokens=self.max_tokens,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": f"data:image/png;base64,{base64_image}"
                                }
                            }
                        ]
                    }
                ],
                **kwargs
            )
            
            content = response.choices[0].message.content
            usage = response.usage
            input_tokens = getattr(usage, "prompt_tokens", 0) if usage else 0
            output_tokens = getattr(usage, "completion_tokens", 0) if usage else 0
            result = self._parse_vision_response(content)
            result.tokens_used = input_tokens + output_tokens
            result.input_tokens = input_tokens
            result.output_tokens = output_tokens
            # GPT-4o pricing: $5/1M input, $15/1M output
            result.cost_usd = (input_tokens * 5 + output_tokens * 15) / 1_000_000
            return result

        except Exception as e:
            logger.error(f"OpenAI API error: {e}")
            raise VisionAPIError(str(e), "openai")
    
    def _default_prompt(self, task: str) -> str:
        """Generate default prompt for computer use."""
        return VISION_PROMPT_TEMPLATE.format(task=task)
    
    def _parse_vision_response(self, content: str) -> VisionResponse:
        """Parse OpenAI response into structured VisionResponse."""
        try:
            data = self._parse_json_response(content)
            
            elements = [
                VisionElement(
                    label=e.get("label", ""),
                    bbox=e.get("bbox", [0, 0, 0, 0]),
                    confidence=e.get("confidence", 0.0),
                    text=e.get("text")
                )
                for e in data.get("elements", [])
            ]
            
            action_data = data.get("action")
            action = None
            if action_data:
                action = VisionAction(
                    type=action_data.get("type", ""),
                    target=action_data.get("target", ""),
                    reason=action_data.get("reason", ""),
                    coordinates=action_data.get("coordinates")
                )
            
            return VisionResponse(
                elements=elements,
                action=action,
                confidence=data.get("confidence", 0.0),
                raw_response=content
            )
            
        except Exception as e:
            logger.error(f"Failed to parse vision response: {e}")
            return VisionResponse(
                elements=[],
                action=None,
                confidence=0.0,
                raw_response=content
            )


class AnthropicVisionClient(VisionProvider):
    """Anthropic Claude 3 vision provider."""
    
    DEFAULT_MODEL = "claude-sonnet-5-5"
    DEFAULT_MAX_TOKENS = 4096
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = DEFAULT_MODEL,
        max_tokens: int = DEFAULT_MAX_TOKENS,
        **kwargs
    ):
        super().__init__(api_key, **kwargs)
        self.model = model
        self.max_tokens = max_tokens
        self._client = None
        
        # Check if anthropic is installed
        try:
            import anthropic
            self._anthropic_module = anthropic
        except ImportError:
            logger.error("Anthropic package not installed. Install with: pip install anthropic")
            self._anthropic_module = None
    
    def _init_client(self):
        """Initialize Anthropic client with current configuration."""
        if self._anthropic_module and not self._client and self.api_key:
            self._client = self._anthropic_module.Anthropic(api_key=self.api_key)
    
    def is_available(self) -> bool:
        """Check if Anthropic client is configured."""
        if not self._anthropic_module:
            return False
        if not self.api_key:
            self.api_key = os.environ.get("ANTHROPIC_API_KEY")
        if self.api_key:
            self._init_client()
        return bool(self.api_key and self._client)
    
    def analyze_image(
        self,
        image_bytes: bytes,
        task: str,
        prompt_template: Optional[str] = None,
        **kwargs
    ) -> VisionResponse:
        """Analyze image using Claude 3."""
        if not self.is_available():
            raise VisionConfigError(
                "Anthropic client not available. Set ANTHROPIC_API_KEY environment variable."
            )
        
        prompt = prompt_template or self._default_prompt(task)
        base64_image = self.encode_image(image_bytes)
        
        try:
            response = self._client.messages.create(
                model=self.model,
                max_tokens=self.max_tokens,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": base64_image
                                }
                            },
                            {
                                "type": "text",
                                "text": prompt
                            }
                        ]
                    }
                ],
                **kwargs
            )
            
            content = response.content[0].text
            input_tokens = getattr(response.usage, "input_tokens", 0) if response.usage else 0
            output_tokens = getattr(response.usage, "output_tokens", 0) if response.usage else 0
            result = self._parse_vision_response(content)
            result.tokens_used = input_tokens + output_tokens
            result.input_tokens = input_tokens
            result.output_tokens = output_tokens
            # Claude 3 Opus pricing: $15/1M input, $75/1M output
            result.cost_usd = (input_tokens * 15 + output_tokens * 75) / 1_000_000
            return result

        except Exception as e:
            logger.error(f"Anthropic API error: {e}")
            raise VisionAPIError(str(e), "anthropic")
    
    def _default_prompt(self, task: str) -> str:
        """Generate default prompt for computer use."""
        return VISION_PROMPT_TEMPLATE.format(task=task)
    
    def _parse_vision_response(self, content: str) -> VisionResponse:
        """Parse Claude response into structured VisionResponse."""
        try:
            data = self._parse_json_response(content)
            
            elements = [
                VisionElement(
                    label=e.get("label", ""),
                    bbox=e.get("bbox", [0, 0, 0, 0]),
                    confidence=e.get("confidence", 0.0),
                    text=e.get("text")
                )
                for e in data.get("elements", [])
            ]
            
            action_data = data.get("action")
            action = None
            if action_data:
                action = VisionAction(
                    type=action_data.get("type", ""),
                    target=action_data.get("target", ""),
                    reason=action_data.get("reason", ""),
                    coordinates=action_data.get("coordinates")
                )
            
            return VisionResponse(
                elements=elements,
                action=action,
                confidence=data.get("confidence", 0.0),
                raw_response=content
            )
            
        except Exception as e:
            logger.error(f"Failed to parse vision response: {e}")
            return VisionResponse(
                elements=[],
                action=None,
                confidence=0.0,
                raw_response=content
            )


class GeminiVisionProvider(VisionProvider):
    """Google Gemini Vision provider (gemini-2.0-flash-exp or gemini-1.5-pro-vision)."""

    DEFAULT_MODEL = "gemini-2.0-flash-exp"

    def __init__(self, api_key: Optional[str] = None, model: Optional[str] = None, **kwargs):
        super().__init__(api_key=api_key or os.getenv("GOOGLE_API_KEY"), **kwargs)
        self.model = model or os.getenv("GEMINI_MODEL_NAME", self.DEFAULT_MODEL)

    def is_available(self) -> bool:
        return bool(self.api_key)

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template: Optional[str] = None, **kwargs) -> VisionResponse:
        if not self.is_available():
            raise VisionConfigError("GOOGLE_API_KEY not set")
        try:
            import google.generativeai as genai
            genai.configure(api_key=self.api_key)
            model = genai.GenerativeModel(self.model)
            import PIL.Image, io
            img = PIL.Image.open(io.BytesIO(image_bytes))
            prompt = prompt_template or VISION_PROMPT_TEMPLATE
            full_prompt = f"{prompt}\n\nTask: {task}"
            response = model.generate_content([full_prompt, img])
            raw = response.text
            parsed = self._parse_json_response(raw)
            return self._build_vision_response(parsed, raw)
        except Exception as e:
            raise VisionAPIError(f"Gemini API error: {e}", provider="gemini")

    async def ground_and_reason(self, screenshot_b64: Union[str, bytes], task: str, history: Optional[List] = None, screen_size: Optional[Tuple[int, int]] = None, **kwargs) -> ActionPlan:
        if not self.is_available():
            raise VisionConfigError("GOOGLE_API_KEY not set")
        screenshot_bytes = _screenshot_bytes(screenshot_b64)
        screen_size = resolve_screen_size(screenshot_bytes, screen_size)
        history = history or []
        try:
            import google.generativeai as genai
            import PIL.Image, io
            genai.configure(api_key=self.api_key)
            model = genai.GenerativeModel(self.model)
            img = PIL.Image.open(io.BytesIO(screenshot_bytes))
            history_text = "\n".join([f"Step {i+1}: {h.get('action','?')} → {h.get('observation','?')}" if isinstance(h, dict) else f"Step {i+1}: {h}" for i, h in enumerate(history[-5:])])
            prompt = _build_planning_prompt(task, history_text, screen_size)
            response = model.generate_content([prompt, img])
            return _parse_action_plan(response.text)
        except Exception as e:
            raise VisionAPIError(f"Gemini ground_and_reason error: {e}", provider="gemini")

    def _build_vision_response(self, parsed: Dict, raw: str) -> VisionResponse:
        elements = [VisionElement(label=e.get("label",""), bbox=e.get("bbox",[0,0,1,1]), confidence=e.get("confidence",0.5), text=e.get("text")) for e in parsed.get("elements", [])]
        action = None
        if "action" in parsed:
            a = parsed["action"]
            action = VisionAction(type=a.get("type","click"), target=a.get("target",""), reason=a.get("reason",""), coordinates=a.get("coordinates"))
        return VisionResponse(elements=elements, action=action, confidence=parsed.get("confidence", 0.5), raw_response=raw)


class QwenVisionProvider(VisionProvider):
    """Qwen2.5-VL provider via OpenAI-compatible endpoint (vLLM/Ollama)."""

    DEFAULT_MODEL = "Qwen/Qwen2.5-VL-7B-Instruct"

    def __init__(self, base_url: Optional[str] = None, api_key: Optional[str] = None, model: Optional[str] = None, **kwargs):
        super().__init__(api_key=api_key or os.getenv("QWEN_API_KEY", "not-required"), **kwargs)
        self.base_url = base_url or os.getenv("QWEN_BASE_URL", "http://localhost:8000/v1")
        self.model = model or os.getenv("QWEN_MODEL_NAME", self.DEFAULT_MODEL)

    def is_available(self) -> bool:
        return bool(self.base_url)

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template: Optional[str] = None, **kwargs) -> VisionResponse:
        if not self.is_available():
            raise VisionConfigError("QWEN_BASE_URL not set")
        try:
            from openai import OpenAI
            client = OpenAI(base_url=self.base_url, api_key=self.api_key)
            b64 = self.encode_image(image_bytes)
            prompt = prompt_template or VISION_PROMPT_TEMPLATE
            response = client.chat.completions.create(
                model=self.model,
                messages=[{"role": "user", "content": [
                    {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}},
                    {"type": "text", "text": f"{prompt}\n\nTask: {task}"},
                ]}],
                max_tokens=1024,
            )
            raw = response.choices[0].message.content
            parsed = self._parse_json_response(raw)
            return self._build_response(parsed, raw)
        except Exception as e:
            raise VisionAPIError(f"Qwen API error: {e}", provider="qwen")

    async def ground_and_reason(self, screenshot_b64: Union[str, bytes], task: str, history: Optional[List] = None, screen_size: Optional[Tuple[int, int]] = None, **kwargs) -> ActionPlan:
        if not self.is_available():
            raise VisionConfigError("QWEN_BASE_URL not set")
        screenshot_bytes = _screenshot_bytes(screenshot_b64)
        screen_size = resolve_screen_size(screenshot_bytes, screen_size)
        history = history or []
        try:
            from openai import OpenAI
            client = OpenAI(base_url=self.base_url, api_key=self.api_key)
            b64 = self.encode_image(screenshot_bytes)
            history_text = "\n".join([f"Step {i+1}: {h.get('action','?')} → {h.get('observation','?')}" if isinstance(h, dict) else f"Step {i+1}: {h}" for i, h in enumerate(history[-5:])])
            prompt = _build_planning_prompt(task, history_text, screen_size)
            response = client.chat.completions.create(
                model=self.model,
                messages=[{"role": "user", "content": [
                    {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}},
                    {"type": "text", "text": prompt},
                ]}],
                max_tokens=1024,
            )
            return _parse_action_plan(response.choices[0].message.content)
        except Exception as e:
            raise VisionAPIError(f"Qwen ground_and_reason error: {e}", provider="qwen")

    def _build_response(self, parsed: Dict, raw: str) -> VisionResponse:
        elements = [VisionElement(label=e.get("label",""), bbox=e.get("bbox",[0,0,1,1]), confidence=e.get("confidence",0.5)) for e in parsed.get("elements", [])]
        action = None
        if "action" in parsed:
            a = parsed["action"]
            action = VisionAction(type=a.get("type","click"), target=a.get("target",""), reason=a.get("reason",""), coordinates=a.get("coordinates"))
        return VisionResponse(elements=elements, action=action, confidence=parsed.get("confidence", 0.5), raw_response=raw)


class ShowUIVisionProvider(VisionProvider):
    """ShowUI 2B lightweight vision model (local HuggingFace)."""

    DEFAULT_MODEL = "showlab/ShowUI-2B"

    def __init__(self, model_path: Optional[str] = None, **kwargs):
        super().__init__(**kwargs)
        self.model_path = model_path or os.getenv("SHOWUI_MODEL_PATH", self.DEFAULT_MODEL)
        self._model = None
        self._processor = None

    def is_available(self) -> bool:
        try:
            import transformers  # noqa
            return True
        except ImportError:
            return False

    def _load_model(self):
        if self._model is None:
            from transformers import AutoProcessor, AutoModelForVision2Seq
            self._processor = AutoProcessor.from_pretrained(self.model_path)
            self._model = AutoModelForVision2Seq.from_pretrained(self.model_path)

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template: Optional[str] = None, **kwargs) -> VisionResponse:
        if not self.is_available():
            raise VisionConfigError("transformers package not installed")
        try:
            import torch, PIL.Image, io
            self._load_model()
            img = PIL.Image.open(io.BytesIO(image_bytes))
            inputs = self._processor(images=img, text=f"Task: {task}\nFind the element to interact with.", return_tensors="pt")
            with torch.no_grad():
                outputs = self._model.generate(**inputs, max_new_tokens=256)
            raw = self._processor.decode(outputs[0], skip_special_tokens=True)
            # ShowUI outputs coordinates directly
            coords = self._extract_coords(raw)
            action = VisionAction(type="click", target="element", reason=raw, coordinates=coords) if coords else None
            return VisionResponse(elements=[], action=action, confidence=0.75 if coords else 0.3, raw_response=raw)
        except Exception as e:
            raise VisionAPIError(f"ShowUI error: {e}", provider="showui")

    def _extract_coords(self, text: str) -> Optional[List[float]]:
        import re
        m = re.search(r'\[(\d+(?:\.\d+)?),\s*(\d+(?:\.\d+)?)\]', text)
        if m:
            return [float(m.group(1)), float(m.group(2))]
        return None


class AzureOpenAIVisionClient(VisionProvider):
    """Azure OpenAI GPT-4o vision provider."""
    
    DEFAULT_MODEL = "gpt-4o"
    DEFAULT_API_VERSION = "2024-02-15-preview"
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        endpoint: Optional[str] = None,
        deployment_name: Optional[str] = None,
        api_version: str = DEFAULT_API_VERSION,
        **kwargs
    ):
        super().__init__(api_key, **kwargs)
        self.endpoint = endpoint
        self.deployment_name = deployment_name
        self.api_version = api_version
        
        # Try to import openai
        try:
            from openai import AzureOpenAI
            self._client_class = AzureOpenAI
            self._client = None  # Lazy init
        except ImportError:
            logger.error("OpenAI package not installed. Install with: pip install openai")
            self._client_class = None
    
    def _init_client(self):
        """Initialize Azure OpenAI client with current configuration."""
        if not self._client and self._client_class:
            self._client = self._client_class(
                api_key=self.api_key,
                azure_endpoint=self.endpoint,
                api_version=self.api_version
            )
    
    def is_available(self) -> bool:
        """Check if Azure OpenAI client is configured."""
        if not self._client_class:
            return False
        
        # Load from environment if not set
        if not self.api_key:
            self.api_key = os.environ.get("AZURE_OPENAI_API_KEY")
        if not self.endpoint:
            self.endpoint = os.environ.get("AZURE_OPENAI_ENDPOINT")
        if not self.deployment_name:
            self.deployment_name = os.environ.get("AZURE_OPENAI_DEPLOYMENT_NAME")
        
        available = bool(self.api_key and self.endpoint and self.deployment_name)
        if available:
            self._init_client()
        return available
    
    def analyze_image(
        self,
        image_bytes: bytes,
        task: str,
        prompt_template: Optional[str] = None,
        **kwargs
    ) -> VisionResponse:
        """Analyze image using Azure OpenAI GPT-4o."""
        if not self.is_available():
            raise VisionConfigError(
                "Azure OpenAI client not available. Set AZURE_OPENAI_API_KEY, "
                "AZURE_OPENAI_ENDPOINT, and AZURE_OPENAI_DEPLOYMENT_NAME environment variables."
            )
        
        prompt = prompt_template or self._default_prompt(task)
        base64_image = self.encode_image(image_bytes)
        
        try:
            response = self._client.chat.completions.create(
                model=self.deployment_name,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": f"data:image/png;base64,{base64_image}"
                                }
                            }
                        ]
                    }
                ],
                **kwargs
            )
            
            content = response.choices[0].message.content
            tokens_used = getattr(getattr(response, "usage", None), "total_tokens", 0) or 0
            result = self._parse_vision_response(content)
            result.tokens_used = tokens_used
            return result

        except Exception as e:
            logger.error(f"Azure OpenAI API error: {e}")
            raise VisionAPIError(str(e), "azure_openai")
    
    def _default_prompt(self, task: str) -> str:
        """Generate default prompt for computer use."""
        return VISION_PROMPT_TEMPLATE.format(task=task)
    
    def _parse_vision_response(self, content: str) -> VisionResponse:
        """Parse Azure OpenAI response into structured VisionResponse."""
        try:
            data = self._parse_json_response(content)
            
            elements = [
                VisionElement(
                    label=e.get("label", ""),
                    bbox=e.get("bbox", [0, 0, 0, 0]),
                    confidence=e.get("confidence", 0.0),
                    text=e.get("text")
                )
                for e in data.get("elements", [])
            ]
            
            action_data = data.get("action")
            action = None
            if action_data:
                action = VisionAction(
                    type=action_data.get("type", ""),
                    target=action_data.get("target", ""),
                    reason=action_data.get("reason", ""),
                    coordinates=action_data.get("coordinates")
                )
            
            return VisionResponse(
                elements=elements,
                action=action,
                confidence=data.get("confidence", 0.0),
                raw_response=content
            )
            
        except Exception as e:
            logger.error(f"Failed to parse vision response: {e}")
            return VisionResponse(
                elements=[],
                action=None,
                confidence=0.0,
                raw_response=content
            )


class MockVisionClient(VisionProvider):
    """
    Mock vision provider for testing only.
    Returns predictable responses for unit tests.
    """
    
    def __init__(self, **kwargs):
        super().__init__(api_key="mock", **kwargs)
        self._test_responses = kwargs.get("test_responses", {})
    
    def is_available(self) -> bool:
        """Mock is always available for testing."""
        return True
    
    def analyze_image(
        self,
        image_bytes: bytes,
        task: str,
        prompt_template: Optional[str] = None,
        **kwargs
    ) -> VisionResponse:
        """Return mock response for testing."""
        logger.warning("Using MockVisionClient - for testing only!")
        
        # Return mock data
        return VisionResponse(
            elements=[
                VisionElement(
                    label="submit button",
                    bbox=[0.5, 0.6, 0.6, 0.65],
                    confidence=0.95
                )
            ],
            action=VisionAction(
                type="click",
                target="submit button",
                reason="Task requires clicking submit",
                coordinates=[0.55, 0.625]
            ),
            confidence=0.9,
            raw_response='{"elements": [...], "action": {...}}'
        )


def _build_planning_prompt(task: str, history_text: str, screen_size: Tuple[int, int]) -> str:
    return f"""You are a computer-use agent. Analyze this screenshot and determine the next action.

TASK: {task}
SCREEN SIZE: {screen_size[0]}x{screen_size[1]}
HISTORY:
{history_text or "No previous steps."}

Respond with valid JSON only:
{{
  "reasoning": "why you chose this action",
  "plan_steps": ["remaining step 1", "remaining step 2"],
  "immediate_action": {{
    "type": "click|type|scroll|navigate|key|screenshot",
    "target": "element description",
    "reason": "why",
    "coordinates": [x, y],
    "text": "text to type (if type action)"
  }},
  "confidence": 0.0-1.0,
  "requires_approval": false,
  "risk_level": "low|medium|high|critical",
  "done": false,
  "batch": [
    {{
      "type": "click|type|fill|scroll|double_click|key",
      "target": "CSS selector or XPath on the SAME page (e.g. #submit)",
      "reason": "why",
      "text": "text to type (if type/fill action)"
    }}
  ]
}}

"batch" is OPTIONAL: list further actions only when they are all on the same
page, each uses a CSS selector / XPath target (not coordinates), and none
depends on observing the screen after an earlier action. Omit it when unsure —
the engine falls back to one step at a time."""


ACTION_PLAN_JSON_SCHEMA: Dict[str, Any] = {
    "type": "object",
    "properties": {
        "reasoning": {"type": "string"},
        "plan_steps": {"type": "array", "items": {"type": "string"}},
        "immediate_action": {
            "type": "object",
            "properties": {
                "type": {"type": "string"},
                "target": {"type": "string"},
                "reason": {"type": "string"},
                "coordinates": {"type": "array", "items": {"type": "number"}},
                "text": {"type": "string"},
            },
            "required": ["type", "target"],
        },
        "confidence": {"type": "number"},
        "requires_approval": {"type": "boolean"},
        "risk_level": {"type": "string"},
        "done": {"type": "boolean"},
        # Optional batch continuation (core/batch_dispatch.py): further
        # whitelisted actions on the same page, shipped as one grant-bound
        # batch. Omit when the next step depends on observing the screen.
        "batch": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "type": {"type": "string"},
                    "target": {"type": "string"},
                    "reason": {"type": "string"},
                    "coordinates": {"type": "array", "items": {"type": "number"}},
                    "text": {"type": "string"},
                },
                "required": ["type", "target"],
            },
        },
        # Optional code-mode request (core/code_mode.py): validated grant-bound
        # code payload, consumed only when the run explicitly enabled code
        # mode. Omit — whitelist actions and batches remain the primary paths.
        "code": {
            "type": "object",
            "properties": {
                "language": {"type": "string"},
                "code": {"type": "string"},
                "declaredTargets": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["code"],
        },
    },
    "required": ["immediate_action", "done"],
}


def gizzi_runtime_base(url: str) -> str:
    """Normalize a Gizzi origin. Session routes live at `{origin}/v1/session`."""
    raw = (url or "").strip().rstrip("/")
    if raw.endswith("/v1"):
        raw = raw[:-3]
    return raw or "http://127.0.0.1:4096"


def parse_platform_model(value: str) -> Tuple[str, str]:
    """Split the picker id `providerID/modelID` used by every Allternit surface."""
    text = (value or "").strip()
    if "/" not in text:
        raise VisionConfigError(
            f"Platform brain must be provider/model (got {value!r}). "
            "Pick a runtime in the same brain picker Home and Code use."
        )
    provider_id, model_id = text.split("/", 1)
    if not provider_id or not model_id:
        raise VisionConfigError(f"Invalid platform brain {value!r}")
    return provider_id, model_id


def plan_from_gizzi_message(result: Any) -> ActionPlan:
    """Read a Gizzi `/v1/session/:id/message` response into an ActionPlan."""
    if not isinstance(result, dict):
        raise VisionAPIError("Gizzi brain returned a non-object plan", provider="allternit")
    info = result.get("info") if isinstance(result.get("info"), dict) else {}
    structured = info.get("structured")
    if isinstance(structured, dict):
        return _parse_action_plan(json.dumps(structured))
    if isinstance(structured, str) and structured.strip():
        return _parse_action_plan(structured)
    parts = result.get("parts") or []
    texts: List[str] = []
    for part in parts:
        if not isinstance(part, dict):
            continue
        if part.get("type") == "text" and part.get("text"):
            texts.append(str(part["text"]))
        if part.get("type") == "tool":
            state = part.get("state") if isinstance(part.get("state"), dict) else {}
            payload = state.get("output") or state.get("input")
            if isinstance(payload, dict) and (
                "immediate_action" in payload or part.get("tool") == "StructuredOutput"
            ):
                return _parse_action_plan(json.dumps(payload))
            if isinstance(payload, str) and payload.strip():
                try:
                    return _parse_action_plan(payload)
                except Exception:
                    texts.append(payload)
    if texts:
        return _parse_action_plan("\n".join(texts))
    raise VisionAPIError("Gizzi brain returned no plan", provider="allternit")


def _parse_action_plan(raw: str) -> ActionPlan:
    """Parse LLM JSON response into ActionPlan."""
    try:
        # extract JSON from markdown if needed
        text = raw
        if "```json" in text:
            text = text.split("```json")[1].split("```")[0].strip()
        elif "```" in text:
            text = text.split("```")[1].split("```")[0].strip()
        data = json.loads(text)
        ia = data.get("immediate_action", {})
        action = VisionAction(
            type=ia.get("type", "screenshot"),
            target=ia.get("target", "screen"),
            reason=ia.get("reason", ""),
            coordinates=ia.get("coordinates"),
            text=ia.get("text"),
        )
        batch = None
        raw_batch = data.get("batch")
        if isinstance(raw_batch, list) and raw_batch:
            parsed_batch = []
            for item in raw_batch:
                if not isinstance(item, dict):
                    parsed_batch = None
                    break
                parsed_batch.append(VisionAction(
                    type=item.get("type", ""),
                    target=item.get("target", ""),
                    reason=item.get("reason", ""),
                    coordinates=item.get("coordinates"),
                    text=item.get("text"),
                ))
            batch = parsed_batch or None
        code = None
        raw_code = data.get("code")
        if isinstance(raw_code, dict) and isinstance(raw_code.get("code"), str):
            raw_targets = raw_code.get("declaredTargets") or raw_code.get("declared_targets") or []
            code = {
                "language": str(raw_code.get("language") or "playwright-js"),
                "code": raw_code["code"],
                "declaredTargets": [str(t) for t in raw_targets if isinstance(t, (str, int, float))],
            }
        return ActionPlan(
            reasoning=data.get("reasoning", ""),
            plan_steps=data.get("plan_steps", []),
            immediate_action=action,
            confidence=float(data.get("confidence", 0.5)),
            requires_approval=bool(data.get("requires_approval", False)),
            risk_level=data.get("risk_level", "low"),
            done=bool(data.get("done", False)),
            batch=batch,
            code=code,
        )
    except Exception:
        return ActionPlan(
            reasoning=raw[:200],
            plan_steps=[],
            immediate_action=VisionAction(type="screenshot", target="screen", reason="Parse error"),
            confidence=0.1,
        )


class UITARSVisionProvider(VisionProvider):
    """
    UI-TARS vision provider — routes to the model.ui_tars.propose skill.

    UI-TARS is purpose-built for GUI grounding and produces the most reliable
    action proposals. When the skills server is unavailable, falls back gracefully
    to returning a raw screenshot action so the planning loop can continue.

    Env vars:
        ACU_SKILLS_URL  — skills HTTP endpoint (default http://localhost:8770)
    """

    SKILL_ID = "model.ui_tars.propose"

    def __init__(self, skills_url: Optional[str] = None):
        self._skills_url = (skills_url or os.environ.get("ACU_SKILLS_URL", "http://localhost:8770")).rstrip("/")

    async def analyze_screenshot(self, screenshot_b64: str, task: str, **kwargs) -> VisionResponse:
        try:
            import httpx
            payload = {"skill": self.SKILL_ID, "input": {"screenshot_b64": screenshot_b64, "task": task}}
            async with httpx.AsyncClient(timeout=30) as client:
                resp = await client.post(f"{self._skills_url}/v1/skills/invoke", json=payload)
                resp.raise_for_status()
                data = resp.json().get("output", {})
            proposals = data.get("proposals", [])
            top = proposals[0] if proposals else {}
            action = VisionAction(
                type=top.get("type", "screenshot"),
                target=str(top.get("params", {}).get("selector", "screen")),
                reason=top.get("description", ""),
                coordinates=[top["params"]["x"], top["params"]["y"]]
                if "x" in top.get("params", {}) else None,
            )
            return VisionResponse(
                elements=[],
                action=action,
                confidence=float(data.get("confidence", 0.9)),
                raw_response=json.dumps(data),
            )
        except Exception as e:
            logger.warning(f"UI-TARS skill unavailable ({e})")
            raise

    async def ground_and_reason(self, screenshot_b64: str, task: str, history: Optional[List[str]] = None, **kwargs) -> ActionPlan:
        try:
            import httpx
            payload = {
                "skill": self.SKILL_ID,
                "input": {"screenshot_b64": screenshot_b64, "task": task, "history": history or []},
            }
            async with httpx.AsyncClient(timeout=30) as client:
                resp = await client.post(f"{self._skills_url}/v1/skills/invoke", json=payload)
                resp.raise_for_status()
                data = resp.json().get("output", {})
            proposals = data.get("proposals", [])
            top = proposals[0] if proposals else {}
            action = VisionAction(
                type=top.get("type", "screenshot"),
                target=str(top.get("params", {}).get("selector", "screen")),
                reason=top.get("description", ""),
                coordinates=[top["params"]["x"], top["params"]["y"]]
                if "x" in top.get("params", {}) else None,
            )
            return ActionPlan(
                reasoning=data.get("reasoning", ""),
                plan_steps=[p.get("description", "") for p in proposals[1:]],
                immediate_action=action,
                confidence=float(data.get("confidence", 0.9)),
                done=top.get("type") == "done",
            )
        except Exception as e:
            logger.warning(f"UI-TARS ground_and_reason unavailable ({e})")
            raise

    def is_available(self) -> bool:
        return True  # optimistic — skill server absence is handled gracefully in analyze_screenshot

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template=None, **kwargs) -> "VisionResponse":
        import base64, asyncio
        b64 = base64.b64encode(image_bytes).decode()
        try:
            loop = asyncio.get_event_loop()
            plan = loop.run_until_complete(self.ground_and_reason(b64, task))
        except Exception:
            raise  # propagate — let planning_loop treat this as a hard failure
        return VisionResponse(
            elements=[],
            action=plan.immediate_action,
            confidence=plan.confidence,
        )


class AllternitGatewayProvider(VisionProvider):
    """
    Primary provider for all Allternit surfaces.

    Computer-use planning uses the same Gizzi brain runtime as Home and Code:
    POST `/v1/session` + `/v1/session/:id/message` with `providerID/modelID`.
    It does not call the OpenAI-compat `ak-` LLM gateway and does not invent
    a second cloud VL key.
    """

    def __init__(self, base_url: Optional[str] = None, model: Optional[str] = None, **kwargs):
        raw = (
            base_url
            or os.environ.get("ALLTERNIT_GIZZI_URL")
            or os.environ.get("TERMINAL_SERVER_URL")
            or os.environ.get("ALLTERNIT_LOCAL_BRAIN_URL")
            or "http://127.0.0.1:4096"
        )
        self._base = gizzi_runtime_base(str(raw))
        self._model = (
            model
            or os.environ.get("Allternit_VISION_MODEL_NAME")
            or os.environ.get("ALLTERNIT_BRAIN_MODEL")
            or ""
        )
        self._provider_id: Optional[str] = None
        self._model_id: Optional[str] = None
        if self._model:
            try:
                self._provider_id, self._model_id = parse_platform_model(self._model)
            except VisionConfigError:
                self._provider_id, self._model_id = None, None

    def set_model(self, model: str) -> None:
        self._model = model
        self._provider_id, self._model_id = parse_platform_model(model)

    def is_available(self) -> bool:
        return bool(self._base)

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template=None, **kwargs):
        import base64, asyncio
        b64 = base64.b64encode(image_bytes).decode()
        loop = asyncio.get_event_loop()
        plan = loop.run_until_complete(self.ground_and_reason(b64, task))
        return VisionResponse(elements=[], action=plan.immediate_action, confidence=plan.confidence)

    async def analyze_screenshot(self, screenshot_b64: str, task: str, **kwargs) -> VisionResponse:
        plan = await self.ground_and_reason(screenshot_b64, task, **kwargs)
        return VisionResponse(elements=[], action=plan.immediate_action, confidence=plan.confidence)

    def _brain_ref(self) -> Tuple[str, str]:
        if self._provider_id and self._model_id:
            return self._provider_id, self._model_id
        if self._model:
            return parse_platform_model(self._model)
        raise VisionConfigError(
            "No platform brain selected. Pick a runtime in the same picker Home and Code use "
            "and pass it as provider/model on /api/aci/run."
        )

    async def ground_and_reason(self, screenshot_b64: str, task: str, history: Optional[List] = None, **kwargs) -> ActionPlan:
        try:
            import httpx
            provider_id, model_id = self._brain_ref()
            brain = {"providerID": provider_id, "modelID": model_id}
            history_text = "\n".join(str(h) for h in (history or []))
            prompt = _build_planning_prompt(task, history_text, resolve_screen_size(screenshot_b64, kwargs.get("screen_size")))
            headers = {"Content-Type": "application/json"}
            async with httpx.AsyncClient(timeout=120) as client:
                session_resp = await client.post(
                    f"{self._base}/v1/session",
                    json={"title": f"ACI {task[:72]}", "defaultModel": brain},
                    headers=headers,
                )
                if session_resp.status_code >= 400:
                    raise VisionAPIError(
                        f"Gizzi session create failed ({session_resp.status_code}): {session_resp.text[:300]}",
                        provider="allternit",
                        status_code=session_resp.status_code,
                    )
                session = session_resp.json()
                session_id = session.get("id") or (session.get("info") or {}).get("id")
                if not session_id:
                    raise VisionAPIError("Gizzi session create returned no id", provider="allternit")
                message_resp = await client.post(
                    f"{self._base}/v1/session/{session_id}/message",
                    json={
                        "model": brain,
                        "system": (
                            "You are the Allternit computer-use planner. "
                            "Use the selected platform brain only. Return the next action as StructuredOutput JSON. "
                            "Do not call shell, browser, or other tools."
                        ),
                        "format": {"type": "json_schema", "schema": ACTION_PLAN_JSON_SCHEMA},
                        "parts": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "file",
                                "mime": "image/png",
                                "filename": "screen.png",
                                "url": f"data:image/png;base64,{screenshot_b64}",
                            },
                        ],
                    },
                    headers=headers,
                )
                if message_resp.status_code >= 400:
                    raise VisionAPIError(
                        f"Gizzi brain {provider_id}/{model_id} failed ({message_resp.status_code}): {message_resp.text[:300]}",
                        provider="allternit",
                        status_code=message_resp.status_code,
                    )
                return plan_from_gizzi_message(message_resp.json())
        except (VisionAPIError, VisionConfigError):
            raise
        except Exception as e:
            raise VisionAPIError(f"Gizzi brain error: {e}", provider="allternit")


async def _kill_process_tree(proc: asyncio.subprocess.Process) -> None:
    """Kill a brain subprocess AND its descendants, then reap the direct child.

    CLI brains spawn their own children (the actual model process); killing
    only the spawned CLI left the grandchild orphaned (cu22 follow-up F4).
    ``start_new_session=True`` put the child in its own process group, so on
    POSIX ``killpg`` reaps the whole tree; platforms without process groups
    fall back to the direct kill. Best effort throughout — never raise.
    """
    import signal

    try:
        if os.name == "posix":
            os.killpg(proc.pid, signal.SIGKILL)
        else:
            proc.kill()
    except (ProcessLookupError, PermissionError):
        try:
            proc.kill()
        except Exception:
            pass
    except Exception:
        pass
    # Reap the direct child so no zombie lingers.
    try:
        if proc.returncode is None:
            await asyncio.wait_for(proc.wait(), timeout=5)
    except Exception:
        pass


class SubprocessVisionProvider(VisionProvider):
    """
    Subprocess brain provider — invokes a CLI agent as a subprocess.

    Supports: claude CLI, codex CLI, gemini CLI, or any custom command.
    The subprocess receives a prompt + base64 screenshot via stdin and must
    return a JSON action plan on stdout.

    Env vars:
        ALLTERNIT_BRAIN_CMD         — command to invoke (e.g. "claude", "codex", "gemini")
        ALLTERNIT_BRAIN_ARGS        — space-separated extra args (optional)
        ALLTERNIT_BRAIN_TIMEOUT_S   — per-call wall-clock cap in seconds
                                      (default DEFAULT_BRAIN_TIMEOUT_S; the cu22
                                      real-model campaign hit the old fixed 60 s
                                      with gpt-6-astra via the codex CLI)
    """

    def __init__(self, cmd: Optional[str] = None, args: Optional[List[str]] = None,
                 timeout_s: Optional[float] = None):
        self._cmd = cmd or os.environ.get("ALLTERNIT_BRAIN_CMD", "claude")
        self._args = list(args) if args is not None else (
            os.environ.get("ALLTERNIT_BRAIN_ARGS", "").split() or []
        )
        # F3: one sourced timeout for the CLI-brain path (constructor wins,
        # then env, then default). Fast gateway providers use their own
        # transport timeouts and are unaffected.
        if timeout_s is not None:
            self._timeout_s = float(timeout_s)
        else:
            env_timeout = os.environ.get("ALLTERNIT_BRAIN_TIMEOUT_S", "").strip()
            self._timeout_s = float(env_timeout) if env_timeout else DEFAULT_BRAIN_TIMEOUT_S

    def is_available(self) -> bool:
        import shutil, subprocess as _sp
        if not shutil.which(self._cmd):
            return False
        try:
            r = _sp.run([self._cmd, "--version"], capture_output=True, timeout=5)
            return r.returncode == 0
        except Exception:
            return False

    def analyze_image(self, image_bytes: bytes, task: str, prompt_template=None, **kwargs):
        import base64, asyncio
        b64 = base64.b64encode(image_bytes).decode()
        loop = asyncio.get_event_loop()
        plan = loop.run_until_complete(self.ground_and_reason(b64, task))
        return VisionResponse(elements=[], action=plan.immediate_action, confidence=plan.confidence)

    async def analyze_screenshot(self, screenshot_b64: str, task: str, **kwargs) -> VisionResponse:
        plan = await self.ground_and_reason(screenshot_b64, task, **kwargs)
        return VisionResponse(elements=[], action=plan.immediate_action, confidence=plan.confidence)

    async def ground_and_reason(self, screenshot_b64: str, task: str, history: Optional[List] = None, **kwargs) -> ActionPlan:
        import asyncio
        history_text = "\n".join(str(h) for h in (history or []))
        prompt = _build_planning_prompt(task, history_text, resolve_screen_size(screenshot_b64, kwargs.get("screen_size")))
        stdin_payload = json.dumps({"prompt": prompt, "screenshot_b64": screenshot_b64})
        try:
            # F4: start_new_session puts the CLI in its own process group so a
            # timeout/cancellation can kill the whole tree — CLI brains spawn
            # their own children (the model process), and killing only the
            # direct child orphaned the grandchild (cu22 campaign finding).
            proc = await asyncio.create_subprocess_exec(
                self._cmd, *self._args,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                start_new_session=True,
            )
            try:
                stdout, stderr = await asyncio.wait_for(proc.communicate(stdin_payload.encode()), timeout=self._timeout_s)
            except asyncio.CancelledError:
                await _kill_process_tree(proc)
                raise  # cancellation must propagate
            except asyncio.TimeoutError:
                await _kill_process_tree(proc)
                raise VisionAPIError(f"Brain subprocess timed out after {self._timeout_s:g}s", provider="subprocess")
            if proc.returncode != 0:
                raise VisionAPIError(f"Brain subprocess exited {proc.returncode}: {stderr.decode()[:200]}", provider="subprocess")
            return _parse_action_plan(stdout.decode())
        except asyncio.TimeoutError:
            raise VisionAPIError(f"Brain subprocess timed out after {self._timeout_s:g}s", provider="subprocess")
        except FileNotFoundError:
            raise VisionConfigError(
                f"Brain command not found: {self._cmd!r}. "
                f"Install the CLI (e.g. 'npm install -g @anthropic-ai/claude-code') "
                f"or set ALLTERNIT_BRAIN_CMD to a valid command."
            )


# Default per-call wall-clock cap for the CLI-brain path. Real models via
# real CLIs routinely exceed 60 s (the cu22 campaign hit the old fixed 60 s
# cap with gpt-6-astra through the codex CLI); fast gateway providers use
# their own transport timeouts and never see this value. Overridable per
# deployment via ALLTERNIT_BRAIN_TIMEOUT_S or the provider constructor.
DEFAULT_BRAIN_TIMEOUT_S = 240.0

# Production computer-use always uses the Gizzi platform brain. Direct API
# keys, ak- virtual keys, and CLI subprocesses are not auto-selected.
PROVIDER_AUTO_DETECT_ORDER = [
    ("ALLTERNIT_GIZZI_URL", ProviderType.ALLTERNIT),
    ("TERMINAL_SERVER_URL", ProviderType.ALLTERNIT),
    ("ALLTERNIT_LOCAL_BRAIN_URL", ProviderType.ALLTERNIT),
]

# Gizzi session origin (not OpenAI /v1/chat/completions).
_LOCAL_BRAIN_PROBE_TARGETS = [
    (4096, "http://127.0.0.1:4096"),
]


class VisionProviderFactory:
    """Factory for creating vision provider instances."""

    @classmethod
    def _probe_local_brain(cls) -> Optional[str]:
        """
        Probe the Gizzi runtime (`GET /v1/global/health`). That is the
        platform brain used by every surface — not the ak- LLM gateway.
        """
        import urllib.error
        import urllib.request

        for _port, base_url in _LOCAL_BRAIN_PROBE_TARGETS:
            origin = gizzi_runtime_base(base_url)
            try:
                req = urllib.request.Request(
                    f"{origin}/v1/global/health",
                    method="GET",
                    headers={"Accept": "application/json"},
                )
                with urllib.request.urlopen(req, timeout=0.8) as resp:
                    if 200 <= getattr(resp, "status", 200) < 300:
                        return origin
            except urllib.error.HTTPError as err:
                if err.code < 500:
                    return origin
            except OSError:
                continue
        return None

    _providers = {
        ProviderType.ALLTERNIT: AllternitGatewayProvider,
        ProviderType.UITARS: UITARSVisionProvider,
        ProviderType.SUBPROCESS: SubprocessVisionProvider,
        ProviderType.OPENAI: OpenAIVisionClient,
        ProviderType.ANTHROPIC: AnthropicVisionClient,
        ProviderType.AZURE_OPENAI: AzureOpenAIVisionClient,
        ProviderType.GEMINI: GeminiVisionProvider,
        ProviderType.QWEN: QwenVisionProvider,
        ProviderType.SHOWUI: ShowUIVisionProvider,
        ProviderType.MOCK: MockVisionClient,  # test-only, never auto-selected
    }
    
    @classmethod
    def create(
        cls,
        provider_type: Union[str, ProviderType],
        **kwargs
    ) -> VisionProvider:
        """
        Create a vision provider instance.
        
        Args:
            provider_type: Type of provider (openai, anthropic, azure, mock)
            **kwargs: Provider-specific configuration
            
        Returns:
            VisionProvider instance
            
        Raises:
            VisionConfigError: If provider type is invalid
        """
        if isinstance(provider_type, str):
            try:
                provider_type = ProviderType(provider_type.lower())
            except ValueError:
                raise VisionConfigError(
                    f"Unknown provider type: {provider_type}. "
                    f"Available: {[p.value for p in ProviderType]}"
                )
        
        provider_class = cls._providers.get(provider_type)
        if not provider_class:
            raise VisionConfigError(f"Provider class not found for {provider_type}")
        
        return provider_class(**kwargs)
    
    @classmethod
    def create_from_env(cls, **kwargs) -> VisionProvider:
        """
        Create a vision provider from environment variables.

        Environment variables:
            ALLTERNIT_VISION_PROVIDER: Provider type (openai, anthropic, azure, gemini, qwen, showui, auto)
                                       When set to "auto" or not set, auto-detects from available API keys.
            OPENAI_API_KEY: OpenAI API key
            ANTHROPIC_API_KEY: Anthropic API key
            AZURE_OPENAI_API_KEY: Azure OpenAI API key
            AZURE_OPENAI_ENDPOINT: Azure OpenAI endpoint
            AZURE_OPENAI_DEPLOYMENT_NAME: Azure deployment name
            GOOGLE_API_KEY: Google Gemini API key
            QWEN_BASE_URL: Qwen vLLM/Ollama endpoint URL
            QWEN_API_KEY: Qwen API key (optional)
            SHOWUI_MODEL_PATH: Local path or HuggingFace model ID for ShowUI

        Returns:
            VisionProvider instance configured from environment
        """
        provider_type_str = (
            os.environ.get("ALLTERNIT_VISION_PROVIDER")
            or os.environ.get("Allternit_VISION_PROVIDER", "auto")
        )

        if provider_type_str.lower() in ("auto", "", "allternit"):
            probed = cls._probe_local_brain()
            return AllternitGatewayProvider(base_url=probed, **kwargs)

        try:
            provider_type = ProviderType(provider_type_str.lower())
        except ValueError:
            raise VisionConfigError(
                f"Unknown provider type: {provider_type_str!r}. "
                f"Available: {[p.value for p in ProviderType]}"
            )

        if provider_type == ProviderType.ALLTERNIT:
            return AllternitGatewayProvider(**kwargs)
        elif provider_type == ProviderType.UITARS:
            return UITARSVisionProvider(skills_url=os.getenv("ACU_SKILLS_URL"), **kwargs)
        elif provider_type == ProviderType.SUBPROCESS:
            return SubprocessVisionProvider(**kwargs)
        elif provider_type == ProviderType.GEMINI:
            return GeminiVisionProvider(api_key=os.getenv("GOOGLE_API_KEY"), **kwargs)
        elif provider_type == ProviderType.QWEN:
            return QwenVisionProvider(**kwargs)
        elif provider_type == ProviderType.SHOWUI:
            return ShowUIVisionProvider(**kwargs)

        return cls.create(provider_type, **kwargs)
    
    @classmethod
    def register_provider(
        cls,
        provider_type: ProviderType,
        provider_class: type
    ):
        """Register a custom provider class."""
        cls._providers[provider_type] = provider_class


# Default vision prompt template for computer use
VISION_PROMPT_TEMPLATE = """You are controlling a computer. Analyze this screenshot.

Task: {task}

Identify:
1. UI elements relevant to the task
2. Bounding boxes [x1, y1, x2, y2] in normalized coordinates (0-1)
3. Text content if relevant
4. Next action to take

Respond in JSON:
{{
  "elements": [
    {{"label": "submit button", "bbox": [0.5, 0.6, 0.6, 0.65], "confidence": 0.95}}
  ],
  "action": {{"type": "click", "target": "submit button", "reason": "..."}},
  "confidence": 0.9
}}"""


# Convenience function for quick usage
def get_vision_provider(
    provider: Union[str, ProviderType, None] = None,
    **kwargs
) -> VisionProvider:
    """
    Get a vision provider instance.
    
    Args:
        provider: Provider type or None to use environment variable
        **kwargs: Additional configuration
        
    Returns:
        VisionProvider instance
    """
    if provider is None:
        return VisionProviderFactory.create_from_env(**kwargs)
    return VisionProviderFactory.create(provider, **kwargs)
