"""
Allternit Computer Use Observability - Recorder

Event-driven session recording for debugging, replay, and analysis.
Adapter-agnostic: works with Playwright, CDP, extension, desktop.

Usage:
    from observability.recorder import ActionFrame, SessionRecorder
    
    recorder = SessionRecorder(storage_path="/tmp/recordings")
    
    # In gateway action handler:
    frame = await recorder.capture_frame(
        run_id=run_id,
        session_id=session_id,
        step_index=step,
        action="click",
        target="#submit",
        before_screenshot=screenshot_bytes,
    )
    
    # After action completes:
    await recorder.complete_frame(
        frame_id=frame.id,
        status="completed",
        after_screenshot=screenshot_bytes,
        result=result,
    )
"""

from .frame import ActionFrame, FrameStatus, ActionType
from .recorder import SessionRecorder, RecorderConfig
from .storage import FrameStorage, StorageBackend

__all__ = [
    "ActionFrame",
    "FrameStatus", 
    "ActionType",
    "SessionRecorder",
    "RecorderConfig",
    "FrameStorage",
    "StorageBackend",
]
