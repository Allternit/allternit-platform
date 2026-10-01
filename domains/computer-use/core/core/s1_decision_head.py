"""
Allternit Computer Use — S1 decision runtime head

``S1DecisionHead`` is a ``DecisionHead`` that sends the planning loop's typed
closed-set questions to the one S1 decision runtime (``POST /v1/decision``)
instead of running a model in-process. It sits beside ``MlxDirectLogitHead``,
``KimiCliHead``, ``LayaHead`` and ``SemIfHead``; those stay selectable. The
runtime picks the model (backend ``ALLTERNIT_S1_BACKEND``, default ``auto``),
and every answer lands in the runtime's shadow ledger with an
``x-decision_id``, so the planning loop can report outcome labels back.

Mapping:

- one CHOICE request per ``Question`` (bank ``bank.computer_use``,
  ``question_id`` = the question name, ``x-primitive_id`` =
  ``computer_use.<operation|target|goal_satisfied|stuck>``,
  ``x-subject_ref`` = ``cu:<run>:<step>:<question>``);
- the result's probabilities, restricted to the options and renormalised,
  become the ``Choice`` distribution; ``confidence`` uses the same
  ``entropy_confidence`` convention as the other heads.

Shadow only: the head's answer never drives an action (the planning loop
treats every head as observational).
"""

from __future__ import annotations

import time
from typing import Dict, Optional, Sequence, Tuple

from .decision_head import (
    Choice,
    Question,
    ShadowHeadError,
    TypedDecision,
    entropy_confidence,
)
from .s1_decision_client import (
    S1DecisionClient,
    S1DecisionError,
    choice_request,
    decision_id_of,
    probabilities_of,
)

CU_BANK = "bank.computer_use"
CU_PRODUCER = "computer_use.planning_loop"
_INSTRUCTIONS = (
    "Using only the observed state and the given options, choose the answer "
    "to `{name}` for the next browser step."
)


def primitive_for(question_name: str) -> str:
    if question_name.endswith("_target"):
        return "computer_use.target"
    return f"computer_use.{question_name}"


class S1DecisionHead:
    """``DecisionHead`` backed by the S1 decision runtime."""

    def __init__(self, client: Optional[S1DecisionClient] = None, bank: str = CU_BANK) -> None:
        self.client = client or S1DecisionClient()
        self.bank = bank
        self._run_id = "cu"
        self._step = 0
        # (step, question) -> (decision_id, options)
        self._ids: Dict[Tuple[int, str], Tuple[str, Tuple[str, ...]]] = {}
        # question -> the planner's answer this step (Q26 x-incumbent)
        self._incumbent: Dict[str, str] = {}

    # Duck-typed planning-loop hooks -------------------------------------
    def begin_run(self, task_id: str) -> None:
        self._run_id = str(task_id) or "cu"
        self._step = 0
        self._ids.clear()

    def begin_step(self, step_num: int) -> None:
        self._step = int(step_num)
        self._incumbent = {}

    def set_incumbent(self, answers: Dict[str, str]) -> None:
        """The live planner's answers for this step (Q26 ``x-incumbent``).

        Call after ``begin_step``. Questions the planner doesn't answer in the
        head's terms (element-index targets, ``stuck``) are simply absent."""
        self._incumbent = {k: str(v) for k, v in answers.items() if v is not None}

    def decision_id(self, question: str, step: Optional[int] = None) -> Optional[str]:
        hit = self._ids.get((self._step if step is None else step, question))
        return hit[0] if hit else None

    def report_outcome(self, question: str, truth: str, source: str, step: Optional[int] = None) -> bool:
        """Label one earlier decision. Only an option of that question is a label."""
        hit = self._ids.get((self._step if step is None else step, question))
        if not hit or truth not in hit[1]:
            return False
        return self.client.report_outcome(hit[0], truth, source)

    # DecisionHead -------------------------------------------------------
    def decide(self, state_text: str, questions: Sequence[Question]) -> TypedDecision:
        started = time.time()
        choices: Dict[str, Choice] = {}
        for q in questions:
            options = list(q.options)
            req = choice_request(
                producer=CU_PRODUCER,
                bank=self.bank,
                question_id=q.name,
                options=options,
                instructions=_INSTRUCTIONS.format(name=q.name),
                primitive_id=primitive_for(q.name),
                subject_ref=f"cu:{self._run_id}:{self._step}:{q.name}",
                run_id=self._run_id,
                incumbent=self._incumbent.get(q.name),
            )
            try:
                result = self.client.decide(req, state_text)
                probs = probabilities_of(result, options)
            except S1DecisionError as err:
                raise ShadowHeadError(f"S1 decision head: {q.name}: {err}") from err
            chosen = max(options, key=lambda o: probs[o])
            choices[q.name] = Choice(
                question=q.name,
                chosen=chosen,
                probabilities=probs,
                confidence=entropy_confidence(list(probs.values())),
            )
            did = decision_id_of(result)
            if did:
                self._ids[(self._step, q.name)] = (did, tuple(options))
        decision = TypedDecision(
            choices=choices,
            latency_ms=(time.time() - started) * 1000.0,
            model_id=f"s1-decision:{self.client.backend}",
        )
        decision.validate()
        return decision
