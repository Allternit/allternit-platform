"""
Allternit Computer Use — Hybrid Orchestrator
Coordinates cross-family workflows: browser ↔ desktop.

A hybrid workflow is a sequence of steps where each step runs on a different
adapter. The orchestrator manages the handoff between adapters, passing
artifacts (files, data) from one step to the next.

Actions:
  handoff  — Execute a multi-step workflow across families
  execute  — Run a single hybrid step (delegates to sub-adapter)

Example workflow:
  1. browser.playwright → goto + extract (download CSV)
  2. desktop.accessibility → open file in native app, process
  3. browser.playwright → upload result

The orchestrator does NOT hold browser/desktop sessions itself.
It creates sub-adapters, runs steps, collects results, and chains artifacts.
"""

from core import BaseAdapter, ActionRequest, ResultEnvelope, Artifact
from datetime import datetime
from typing import Optional, Dict, Any, List
import json
import uuid


class HybridStep:
    """A single step in a hybrid workflow."""

    def __init__(
        self,
        step_id: str,
        family: str,
        adapter_id: str,
        action_type: str,
        target: str,
        parameters: Optional[Dict[str, Any]] = None,
    ):
        self.step_id = step_id
        self.family = family
        self.adapter_id = adapter_id
        self.action_type = action_type
        self.target = target
        self.parameters = parameters or {}

    def to_dict(self) -> Dict[str, Any]:
        return {
            "step_id": self.step_id,
            "family": self.family,
            "adapter_id": self.adapter_id,
            "action_type": self.action_type,
            "target": self.target,
            "parameters": self.parameters,
        }


class HybridWorkflow:
    """A multi-step cross-family workflow definition."""

    def __init__(self, workflow_id: str = None, name: str = "", steps: List[HybridStep] = None):
        self.workflow_id = workflow_id or str(uuid.uuid4())
        self.name = name
        self.steps = steps or []

    def add_step(self, step: HybridStep):
        self.steps.append(step)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "HybridWorkflow":
        wf = cls(workflow_id=data.get("workflow_id"), name=data.get("name", ""))
        for s in data.get("steps", []):
            wf.add_step(HybridStep(
                step_id=s["step_id"],
                family=s["family"],
                adapter_id=s["adapter_id"],
                action_type=s["action_type"],
                target=s["target"],
                parameters=s.get("parameters", {}),
            ))
        return wf


class HybridOrchestrator(BaseAdapter):
    """
    Cross-family workflow orchestrator.
    Coordinates browser and desktop adapters to execute multi-step workflows.
    """

    def __init__(self, adapter_factory=None):
        """
        Args:
            adapter_factory: callable(adapter_id) -> BaseAdapter instance.
                If not provided, adapters must be pre-registered via register_adapter().
        """
        self._adapter_factory = adapter_factory
        self._adapters: Dict[str, BaseAdapter] = {}
        self._workflows_executed: List[Dict[str, Any]] = []

    @property
    def adapter_id(self) -> str:
        return "hybrid.orchestrator"

    @property
    def family(self) -> str:
        return "hybrid"

    def register_adapter(self, adapter_id: str, adapter: BaseAdapter):
        """Pre-register an adapter instance for use in workflows."""
        self._adapters[adapter_id] = adapter

    async def initialize(self) -> None:
        """Initialize all registered sub-adapters."""
        for aid, adapter in self._adapters.items():
            await adapter.initialize()

    async def execute(self, action: ActionRequest, session_id: str, run_id: str) -> ResultEnvelope:
        envelope = self._make_envelope(action, session_id, run_id)

        try:
            if action.action_type == "handoff":
                result_data = await self._execute_workflow(action, session_id, run_id)
            elif action.action_type == "execute":
                result_data = await self._execute_single_step(action, session_id, run_id)
            else:
                envelope.status = "failed"
                envelope.error = {
                    "code": "UNSUPPORTED_ACTION",
                    "message": f"Orchestrator does not support action '{action.action_type}'",
                    "adapter_id": self.adapter_id,
                }
                envelope.completed_at = datetime.utcnow().isoformat()
                return envelope

            # Propagate workflow-level status: partial/failed workflow → failed envelope
            wf_status = result_data.get("status", "completed") if isinstance(result_data, dict) else "completed"
            envelope.status = "failed" if wf_status in ("partial", "failed") else "completed"
            envelope.extracted_content = result_data
            envelope.completed_at = datetime.utcnow().isoformat()

            # Collect artifacts from all steps
            if "steps" in result_data:
                for step_result in result_data["steps"]:
                    for art in step_result.get("artifacts", []):
                        envelope.artifacts.append(art)

            self._emit_receipt(envelope, action, result_data)

        except Exception as e:
            envelope.status = "failed"
            envelope.error = {
                "code": "ORCHESTRATOR_ERROR",
                "message": str(e),
                "adapter_id": self.adapter_id,
            }
            envelope.extracted_content = {}
            envelope.completed_at = datetime.utcnow().isoformat()

        return envelope

    async def _execute_workflow(self, action: ActionRequest, session_id: str, run_id: str) -> Dict[str, Any]:
        """Execute a multi-step hybrid workflow."""
        workflow_data = action.parameters.get("workflow", {})
        workflow = HybridWorkflow.from_dict(workflow_data)

        step_results = []
        artifacts_chain: Dict[str, Any] = {}  # Shared artifact context between steps

        for i, step in enumerate(workflow.steps):
            try:
                adapter = await self._get_adapter(step.adapter_id)
            except ValueError as e:
                step_results.append({
                    "step_id": step.step_id,
                    "step_index": i,
                    "adapter_id": step.adapter_id,
                    "family": step.family,
                    "action": step.action_type,
                    "status": "failed",
                    "extracted_content": None,
                    "artifacts": [],
                    "error": {"code": "ADAPTER_NOT_FOUND", "message": str(e)},
                })
                break

            # Inject artifacts from previous steps into parameters
            step_params = dict(step.parameters)
            step_params["_artifacts_from_previous"] = artifacts_chain

            step_action = ActionRequest(
                action_type=step.action_type,
                target=step.target,
                parameters=step_params,
                timeout_ms=action.timeout_ms,
            )

            step_session = f"{session_id}_step_{i}"
            step_run = f"{run_id}_step_{i}"

            result = await adapter.execute(step_action, step_session, step_run)

            step_result = {
                "step_id": step.step_id,
                "step_index": i,
                "adapter_id": step.adapter_id,
                "family": step.family,
                "action": step.action_type,
                "status": result.status,
                "extracted_content": result.extracted_content,
                "artifacts": [a.to_dict() if hasattr(a, "to_dict") else a for a in result.artifacts],
            }

            if result.status == "failed":
                step_result["error"] = result.error
                # Fail-fast: stop workflow on step failure
                step_results.append(step_result)
                break

            # Pass artifacts forward
            if result.extracted_content:
                artifacts_chain[step.step_id] = result.extracted_content
            for art in result.artifacts:
                art_dict = art.to_dict() if hasattr(art, "to_dict") else art
                artifacts_chain[f"{step.step_id}_artifact_{art_dict.get('type', 'unknown')}"] = art_dict

            step_results.append(step_result)

        workflow_result = {
            "workflow_id": workflow.workflow_id,
            "workflow_name": workflow.name,
            "total_steps": len(workflow.steps),
            "completed_steps": sum(1 for s in step_results if s["status"] == "completed"),
            "status": "completed" if all(s["status"] == "completed" for s in step_results) else "partial",
            "steps": step_results,
        }

        self._workflows_executed.append(workflow_result)
        return workflow_result

    async def _execute_single_step(self, action: ActionRequest, session_id: str, run_id: str) -> Dict[str, Any]:
        """Execute a single step on a specific adapter."""
        sub_adapter_id = action.parameters.get("adapter_id")
        sub_action_type = action.parameters.get("sub_action", action.action_type)

        if not sub_adapter_id:
            raise ValueError("execute action requires 'adapter_id' in parameters")

        adapter = await self._get_adapter(sub_adapter_id)
        sub_action = ActionRequest(
            action_type=sub_action_type,
            target=action.target,
            parameters=action.parameters,
            timeout_ms=action.timeout_ms,
        )

        result = await adapter.execute(sub_action, session_id, run_id)
        return {
            "delegated_to": sub_adapter_id,
            "status": result.status,
            "extracted_content": result.extracted_content,
        }

    async def _get_adapter(self, adapter_id: str) -> BaseAdapter:
        """Get or create an adapter by ID."""
        if adapter_id in self._adapters:
            return self._adapters[adapter_id]

        if self._adapter_factory:
            adapter = self._adapter_factory(adapter_id)
            await adapter.initialize()
            self._adapters[adapter_id] = adapter
            return adapter

        raise ValueError(
            f"No adapter registered for '{adapter_id}' and no adapter_factory provided. "
            f"Register adapters via register_adapter() or provide an adapter_factory."
        )

    async def close(self) -> None:
        """Close all sub-adapters."""
        for adapter in self._adapters.values():
            try:
                await adapter.close()
            except Exception:
                pass
        self._adapters.clear()
