"""
Recorder event schema - CU-030

Defines the frame model for capturing action executions.
Each frame represents one action step with before/after state.
"""

from datetime import datetime
from enum import Enum
from typing import Any, Optional
from pydantic import BaseModel, Field
import uuid


class ActionType(str, Enum):
    """Browser/Desktop action types."""
    GOTO = "goto"
    CLICK = "click"
    FILL = "fill"
    EXTRACT = "extract"
    SCREENSHOT = "screenshot"
    INSPECT = "inspect"
    EXECUTE = "execute"  # model-driven via the planning loop
    CLOSE = "close"
    # Desktop actions (future)
    KEYBOARD = "keyboard"
    MOUSE_MOVE = "mouse_move"
    SCROLL = "scroll"
    WAIT = "wait"


class FrameStatus(str, Enum):
    """Frame execution status."""
    PENDING = "pending"      # Before action starts
    RUNNING = "running"      # Action in progress
    COMPLETED = "completed"  # Action succeeded
    FAILED = "failed"        # Action failed
    CANCELLED = "cancelled"  # Action cancelled


class ActionFrame(BaseModel):
    """
    Single action frame - the atomic unit of recording.
    
    Captures:
    - What: action type, target, parameters
    - When: timestamps for each phase
    - Where: session/run context
    - Result: status, receipts, screenshots
    """
    
    # Identity
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    run_id: str
    session_id: str
    step_index: int = Field(ge=0, description="Sequential step number within run")
    
    # Action definition
    action: ActionType
    target: Optional[str] = None
    goal: Optional[str] = None  # For LLM-powered actions
    parameters: dict[str, Any] = Field(default_factory=dict)
    
    # Timing
    timestamp_start: datetime = Field(default_factory=datetime.utcnow)
    timestamp_end: Optional[datetime] = None
    duration_ms: Optional[int] = None
    
    # Status
    status: FrameStatus = FrameStatus.PENDING
    error: Optional[dict[str, Any]] = None  # {code, message, details}
    
    # Screenshots (paths or data URIs, depending on storage)
    before_screenshot_path: Optional[str] = None
    after_screenshot_path: Optional[str] = None
    failure_screenshot_path: Optional[str] = None
    
    # Results
    extracted_content: Optional[Any] = None
    adapter_id: Optional[str] = None
    adapter_family: Optional[str] = None  # browser, desktop, hybrid
    
    # Receipt reference
    receipt_id: Optional[str] = None
    trace_id: Optional[str] = None
    
    # Metadata
    metadata: dict[str, Any] = Field(default_factory=dict)
    
    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat()
        }
    
    def complete(self, status: FrameStatus, **kwargs) -> "ActionFrame":
        """Mark frame as complete with results."""
        self.status = status
        self.timestamp_end = datetime.utcnow()
        if self.timestamp_start:
            delta = self.timestamp_end - self.timestamp_start
            self.duration_ms = int(delta.total_seconds() * 1000)
        
        for key, value in kwargs.items():
            if hasattr(self, key):
                setattr(self, key, value)
        
        return self
    
    def to_event(self) -> dict[str, Any]:
        """Convert to event log format."""
        return {
            "event": "action_frame",
            "frame_id": self.id,
            "run_id": self.run_id,
            "session_id": self.session_id,
            "step_index": self.step_index,
            "action": self.action.value,
            "target": self.target,
            "status": self.status.value,
            "timestamp_start": self.timestamp_start.isoformat() if self.timestamp_start else None,
            "timestamp_end": self.timestamp_end.isoformat() if self.timestamp_end else None,
            "duration_ms": self.duration_ms,
            "adapter_id": self.adapter_id,
            "receipt_id": self.receipt_id,
            "trace_id": self.trace_id,
        }


class RunTimeline(BaseModel):
    """
    Complete timeline of a run - collection of frames.
    """
    run_id: str
    session_id: str
    frames: list[ActionFrame] = Field(default_factory=list)
    
    # Summary
    total_steps: int = 0
    completed_steps: int = 0
    failed_steps: int = 0
    total_duration_ms: int = 0
    
    # Timestamps
    run_started: Optional[datetime] = None
    run_completed: Optional[datetime] = None
    
    # Artifacts
    replay_gif_path: Optional[str] = None
    replay_video_path: Optional[str] = None
    timeline_json_path: Optional[str] = None
    
    def add_frame(self, frame: ActionFrame) -> None:
        """Add a frame and update summary."""
        self.frames.append(frame)
        self.total_steps = len(self.frames)
        
        if frame.status == FrameStatus.COMPLETED:
            self.completed_steps += 1
        elif frame.status == FrameStatus.FAILED:
            self.failed_steps += 1
        
        if frame.duration_ms:
            self.total_duration_ms += frame.duration_ms
    
    def get_slow_steps(self, threshold_ms: int = 5000) -> list[ActionFrame]:
        """Get steps that took longer than threshold."""
        return [f for f in self.frames if f.duration_ms and f.duration_ms > threshold_ms]
    
    def get_failed_steps(self) -> list[ActionFrame]:
        """Get all failed steps."""
        return [f for f in self.frames if f.status == FrameStatus.FAILED]
    
    def to_summary(self) -> dict[str, Any]:
        """Generate summary for analysis."""
        return {
            "run_id": self.run_id,
            "session_id": self.session_id,
            "total_steps": self.total_steps,
            "completed": self.completed_steps,
            "failed": self.failed_steps,
            "success_rate": self.completed_steps / max(self.total_steps, 1),
            "total_duration_ms": self.total_duration_ms,
            "avg_step_duration_ms": self.total_duration_ms / max(self.total_steps, 1),
            "slow_steps": len(self.get_slow_steps()),
            "has_replay": bool(self.replay_gif_path or self.replay_video_path),
        }
