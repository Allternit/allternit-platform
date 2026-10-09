"""Prompt rendering for one typed decision.

The prompt is the context, then the option list, then the question. The
engines keep the prefilled state at every cut point (each blank-line block of
the context, the end of the context, the end of the options), so the next
decision that shares a head only prefills what changed. Callers get the most
reuse by putting what stays the same first (the goal, the element map) and
what changes per step last, or in `question`.

Options get one-token labels (A-Z, then a-z) when the tokenizer has them, so a
decision is read from the next-token distribution of a single prefill. Longer
lists fall back to numeric labels scored as continuations.
"""

from __future__ import annotations

from dataclasses import dataclass

# What each built-in decision kind asks. Unknown kinds fall back to "choice";
# a request's own `question` always wins.
KIND_QUESTIONS = {
    "choice": "Choose the best option.",
    "element": "Which option makes progress toward the goal next?",
    "op": "Which operation should run next?",
    "verify": "Did the last action achieve its expected result?",
    "safety": "Is it safe to continue with the proposed action?",
    "escalate": "Should this step be handed back to the planner?",
    "route": "Which engine should run this step?",
}

SYSTEM = (
    "You answer one typed decision. Read the context and the options, then reply "
    "with only the label (the letter or number) of the single best option."
)

MAX_CUTS = 6
LETTERS = [chr(c) for c in range(ord("A"), ord("Z") + 1)] + [chr(c) for c in range(ord("a"), ord("z") + 1)]


@dataclass(frozen=True)
class Rendered:
    """The user turn, the character offsets where state may be kept, and option labels."""

    text: str
    cuts: list[int]
    labels: list[str]


def question_for(kind: str, question: str | None) -> str:
    if question and question.strip():
        return question.strip()
    return KIND_QUESTIONS.get(kind, KIND_QUESTIONS["choice"])


def labels_for(n: int, letters_ok: bool) -> list[str]:
    if letters_ok and n <= len(LETTERS):
        return LETTERS[:n]
    return [str(i + 1) for i in range(n)]


def render(
    context: str, options: list[str], labels: list[str], kind: str = "choice", question: str | None = None
) -> Rendered:
    if len(options) < 2:
        raise ValueError("need at least two options")
    if len(labels) != len(options):
        raise ValueError("one label per option")
    lines = "\n".join(f"{label}. {' '.join(text.split())}" for label, text in zip(labels, options))
    blocks = [b.strip() for b in context.strip().split("\n\n") if b.strip()]
    text, cuts = "Context:\n", []
    for block in blocks:
        text += block + "\n\n"
        cuts.append(len(text))
    text += f"Options:\n{lines}\n\n"
    cuts.append(len(text))
    # The answer format lives in the system prompt, so the per-decision tail
    # (the only part a warm decision prefills) is just the question.
    text += f"Question: {question_for(kind, question)}"
    return Rendered(text=text, cuts=cuts[-MAX_CUTS:], labels=labels)


def chat_messages(user: str) -> list[dict[str, str]]:
    return [{"role": "system", "content": SYSTEM}, {"role": "user", "content": user}]


def common_prefix_len(a: list[int], b: list[int]) -> int:
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    return i
