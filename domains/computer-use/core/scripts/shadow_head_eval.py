"""
Shadow head eval — offline harness entrypoint.

Runs the shadow decision head beside scripted (recorded) LLM decide steps
over three synthetic task observations and writes the eval report:

    python domains/computer-use/core/scripts/shadow_head_eval.py [--steps N]
        [--head {mock,mlx,kimi}] [--questioning {batched,sequential}]
        [--tasks {heldout,train,all}] [--task-seed S]
        [--trajectory {off,on}] [--few-shot N]
        [--reserved-slots {off,on}] [--last-action {off,on}]
        [--model HF-REPO] [--revision SHA]
        [--self-consistency K] [--sc-temperature T]
        [--trace-out PATH] [--out-dir DIR] [--quiet]

``--tasks`` selects the synthetic task set: ``heldout`` (default) runs only
the three canonical held-out tasks — byte-identical to the pre-existing
behavior; ``train`` runs only the TRAIN-split templates from
core/train_tasks.py (task ids outside HELD_OUT_TASK_IDS, so their traces are
labeled split=train and become Laya fine-tune cases); ``all`` runs both.
``--task-seed`` (default 42) reseeds the train templates' derived
names/values/orders — new seeds multiply training volume without new code.
Report stems carry a ``-train`` / ``-all`` suffix when those sets run, so
train reports never overwrite the held-out baseline.

The LLM provider replays a recorded transcript and the AX observation is
scripted, so the run is deterministic except for the head itself:
``--head mock`` (default) uses a deterministic MockHead with a scripted
opinion transcript (fully offline, no downloads); ``--head mlx`` uses the
real MlxDirectLogitHead (mlx-lm, Qwen3-4B-Instruct-2507-4bit) — the first
run downloads ~2.5GB of weights from a public ungated HF repo, then runs
fully local; ``--head kimi`` drives the KimiCliHead (cloud-iteration tier):
one ``kimi -p`` subprocess per decide step, auth handled inside the CLI;
``--head semif`` drives the SemIfHead (SemIf, the community System One
reproduction, behind the same protocol — mlx backend, Qwen3.5-4B pinned,
all questions scored in one shared-state pass).

The shadow state text is the canonical per-step format: [TASK], an
unconditional [SINCE LAST STEP] element-delta block (first step / no-change
/ ADDED+REMOVED+CHANGED rows), then [OBSERVED ELEMENTS] and [OPTIONS].
``--questioning`` applies to the kimi head only: ``batched`` (default) asks
all questions in one subprocess call per step; ``sequential`` asks the
operation + gates first, then the chosen operation's target menu (2 calls
per step, smaller menus per pass). ``--trajectory`` (kimi head only) turns
on live-trajectory retrieval: the head's own prior proposals for the run,
each with its step's delta summary, are rendered into the prompt as
[ACTIONS SO FAR THIS RUN] (explicitly labeled as never-executed shadow
proposals). ``--reserved-slots`` / ``--last-action`` turn on the System One
grafts (both default off): reserved ``reobserve``/``abstain`` slots appended
to the operation question's option list (no target menus; reserved-slot
picks count as disagreements — the recorded LLM policy never abstains), and
a [LAST ACTION] effect/escalation block derived from the real outcome of
the previously executed LLM step. Report stems carry a composed graft
suffix (``-graftA`` / ``-graftB`` / ``-graftAB``) when either is on, so
post-graft runs never overwrite pre-graft reports. See core/shadow_eval.py
for what the numbers do and do not mean.
"""

from __future__ import annotations

import argparse
import importlib.util
import logging
import sys
from pathlib import Path
from typing import Optional

DOMAIN_CORE_ROOT = Path(__file__).resolve().parents[1]
for _extra in (str(DOMAIN_CORE_ROOT),):
    if _extra not in sys.path:
        sys.path.insert(0, _extra)

DEFAULT_OUT_DIR = DOMAIN_CORE_ROOT / "evaluation" / "shadow-eval"

# Per-decide-step wall-clock budget passed down to the planning loop, by head.
# kimi CLI subprocesses run ~25s per call (batched: 1 call/step; sequential:
# 2 calls/step), so the default 15s/step budget would kill the run mid-task.
# semif (SemIf mlx shared-state pass) is mlx-class: one prefill + one batched
# forward per step, same budget as the mlx head — actual latencies land in
# the report.
_STEP_BUDGET_MS = {"mock": 15_000, "mlx": 15_000, "kimi": 60_000, "semif": 15_000, "decision": 60_000}


def build_head(
    name: str,
    questioning: str = "batched",
    few_shot: int = 0,
    few_shot_order: str = "random",
    few_shot_framing: str = "default",
    traces_path: Optional[Path] = None,
    trajectory: bool = False,
    model: Optional[str] = None,
    revision: Optional[str] = None,
    self_consistency: int = 1,
    sc_temperature: float = 0.7,
) -> "tuple[object, str]":
    """Construct the decision head for ``--head``; (head, report-stem-suffix)."""
    if name == "mock":
        return None, ""

    if name == "decision":
        # The one S1 decision runtime (POST /v1/decision); backend from
        # ALLTERNIT_S1_BACKEND (default auto), URL from ALLTERNIT_S1_URL.
        from core.s1_decision_head import S1DecisionHead
        return S1DecisionHead(), "-s1-decision"

    if name == "kimi":
        from core.decision_head import KimiCliHead

        few_shot_block = None
        suffix = "-kimi" + ("-sequential" if questioning == "sequential" else "")
        if few_shot > 0:
            from core.kimi_fewshot import (
                render_few_shot_block,
                select_few_shot_examples,
            )
            from core.tier_a_traces import load_traces

            path = traces_path or (
                DOMAIN_CORE_ROOT / "evaluation" / "tier-a" / "traces.jsonl"
            )
            traces = load_traces(path)
            examples = select_few_shot_examples(
                traces, n=few_shot, order=few_shot_order,
            )
            if len(examples) < few_shot:
                raise SystemExit(
                    f"--few-shot {few_shot} requested but only {len(examples)} "
                    f"valid train-split exemplars in {path}"
                )
            few_shot_block = render_few_shot_block(
                examples, framing=few_shot_framing,
            )
            variant = f"-{few_shot_order}-{few_shot_framing}" if (
                few_shot_order != "random" or few_shot_framing != "default"
            ) else ""
            suffix += f"-fewshot{few_shot}{variant}"
            print(
                f"Few-shot: {len(examples)} exemplars from {path} "
                f"(order={few_shot_order}, framing={few_shot_framing}); "
                "train split only, held-out tasks excluded by construction."
            )
        # The shadow state text now always carries the [SINCE LAST STEP]
        # delta block, so every post-change kimi run is a distinct variant:
        # stem the reports -deltas / -traj to never overwrite the pre-change
        # baseline (shadow-eval-report-kimi.*).
        suffix += "-traj" if trajectory else "-deltas"
        head = KimiCliHead(
            questioning=questioning,
            few_shot_block=few_shot_block,
            trajectory=trajectory,
        )
        print(
            f"Using KimiCliHead ({head.binary}, questioning={questioning}"
            f"{', few-shot' if few_shot_block else ''}"
            f"{', trajectory' if trajectory else ''}); "
            "one `kimi -p` subprocess per decide step."
        )
        return head, suffix

    # SemIf path — fail fast with an actionable error before the harness runs.
    if name == "semif":
        if importlib.util.find_spec("semif_phase1") is None:
            raise SystemExit(
                "--head semif needs the optional 'semif' extra, which is not "
                "installed in this environment. Install it with:\n"
                "    uv pip install -e '.[semif]'\n"
                "(pulls SemIf from git+https://github.com/TheoLeeCJ/SemIf with "
                "its pinned mlx runtime; Apple-silicon only.)"
            )
        from core.semif_head import SemIfHead

        head = SemIfHead()
        print(
            "Using SemIfHead "
            f"({head.model_source}@{head.revision}); first run downloads "
            "~9GB of weights; all questions per step score in ONE "
            "shared-state mlx pass (SemIf parallel mode)."
        )
        return head, "-semif"

    # mlx path — fail fast with an actionable error before the harness runs.
    if importlib.util.find_spec("mlx_lm") is None:
        raise SystemExit(
            "--head mlx needs the optional 'shadow-head' extra, which is not "
            "installed in this environment. Install it with:\n"
            "    uv pip install -e '.[shadow-head]'\n"
            "(mlx is Apple-silicon only; no hosted fallback exists by design.)"
        )
    from core.decision_head import DEFAULT_MODEL_REPO, MlxDirectLogitHead

    head: object = MlxDirectLogitHead(
        model_repo=model or DEFAULT_MODEL_REPO,
        revision=revision,
        temperature=sc_temperature if self_consistency > 1 else 0.0,
    )
    if self_consistency > 1:
        from core.self_consistency import SelfConsistencyHead

        head = SelfConsistencyHead(head, samples=self_consistency)
    inner = head.inner if self_consistency > 1 else head
    model_tag = {
        "mlx-community/Qwen3-4B-Instruct-2507-4bit": "qwen3",
        "mlx-community/Qwen3.5-4B-4bit": "q35",
        "Qwen/Qwen3.5-4B": "q35",
        "mlx-community/gemma-3-4b-it-4bit": "gemma3",
    }.get(inner.model_repo)
    if self_consistency > 1 and model_tag in (None, "qwen3"):
        suffix = f"-mlx-sc{self_consistency}"
    elif model_tag:
        suffix = f"-mlx-{model_tag}" + (
            f"-sc{self_consistency}" if self_consistency > 1 else ""
        )
    else:
        suffix = f"-mlx-sc{self_consistency}" if self_consistency > 1 else "-mlx"
    print(
        "Using MlxDirectLogitHead "
        f"({inner.model_repo}"
        f"{f' x{self_consistency} samples @ T={sc_temperature}' if self_consistency > 1 else ''}"
        f", revision={inner.revision or 'default'}); "
        "first run downloads the weights."
    )
    return head, suffix


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--steps",
        type=int,
        default=22,
        help="decide steps per task (default 22, >= 20 required by the acceptance criteria)",
    )
    parser.add_argument(
        "--head",
        choices=("mock", "mlx", "kimi", "semif", "decision"),
        default="mock",
        help="decision head to score (default mock; mlx = real local mlx-lm "
             "weights; kimi = KimiCliHead subprocess cloud tier; semif = "
             "SemIf community System One reproduction, mlx shared-state pass; "
             "decision = the S1 decision runtime /v1/decision, backend "
             "ALLTERNIT_S1_BACKEND, default auto)",
    )
    parser.add_argument(
        "--tasks",
        choices=("heldout", "train", "all"),
        default="heldout",
        help="synthetic task set: heldout (default) = the three canonical "
             "held-out tasks only, byte-identical to prior behavior; train = "
             "the TRAIN-split templates from core/train_tasks.py only (traces "
             "labeled split=train — Laya fine-tune volume); all = both. "
             "Report stem gains -train / -all accordingly.",
    )
    parser.add_argument(
        "--task-seed",
        type=int,
        default=42,
        metavar="S",
        help="seed for the train task templates' derived names/values/orders "
             "(default 42; applies to --tasks train/all — new seeds multiply "
             "training diversity without new code)",
    )
    parser.add_argument(
        "--questioning",
        choices=("batched", "sequential"),
        default="batched",
        help="questioning mode for the kimi head (default batched; sequential "
             "= operation+gates first, then the chosen operation's target menu, "
             "2 subprocess calls per step)",
    )
    parser.add_argument(
        "--few-shot",
        type=int,
        default=0,
        metavar="N",
        help="kimi head only: inject N train-split Tier A trace exemplars into "
             "the prompt (prompt-space distillation; held-out eval tasks are "
             "excluded from selection by construction). 0 = zero-shot.",
    )
    parser.add_argument(
        "--few-shot-order",
        choices=("random", "interleaved"),
        default="random",
        help="exemplar ordering: random (default) or interleaved by site "
             "template so consecutive examples come from different site types",
    )
    parser.add_argument(
        "--few-shot-framing",
        choices=("default", "task-neutral"),
        default="default",
        help="exemplar block intro framing: default, or task-neutral — an "
             "explicit anti-collapse preamble (site/flow varies; decide from "
             "the CURRENT state only)",
    )
    parser.add_argument(
        "--trajectory",
        choices=("off", "on"),
        default="off",
        help="kimi head only: render the head's own prior proposals for the "
             "current run into the prompt as [ACTIONS SO FAR THIS RUN], each "
             "with its step's element-delta summary (live-trajectory "
             "retrieval, inference-only). Default off — the state text still "
             "always carries the [SINCE LAST STEP] delta block.",
    )
    parser.add_argument(
        "--traces",
        type=Path,
        default=None,
        help="tier-a traces.jsonl path for --few-shot (default "
             "evaluation/tier-a/traces.jsonl)",
    )
    parser.add_argument(
        "--model",
        type=str,
        default=None,
        metavar="HF-REPO",
        help="mlx head only: HF repo id for the shadow-head weights (default "
             "mlx-community/Qwen3-4B-Instruct-2507-4bit). Any mlx-community "
             "4-bit build works; the commit pin applies only to the default repo.",
    )
    parser.add_argument(
        "--revision",
        type=str,
        default=None,
        metavar="SHA",
        help="mlx head only: pinned commit for --model (default: the verified "
             "pin for the default repo, else the repo's default branch)",
    )
    parser.add_argument(
        "--self-consistency",
        type=int,
        default=1,
        metavar="K",
        help="mlx head only: sample K times per decide step (temperature "
             "--sc-temperature) and majority-vote each question; confidence = "
             "winner vote share, per-step vote_margin recorded in the report "
             "rows for abstention curves. 1 (default) = single greedy pass, off.",
    )
    parser.add_argument(
        "--sc-temperature",
        type=float,
        default=0.7,
        help="sampling temperature for --self-consistency > 1 (default 0.7)",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"report output directory (default {DEFAULT_OUT_DIR})",
    )
    parser.add_argument(
        "--reserved-slots",
        choices=("off", "on"),
        default="off",
        help="graft A: append the reserved reobserve/abstain slots to the "
             "operation question's option list (after the whitelist "
             "operations; no target menus). Reserved-slot picks count as "
             "disagreements — the recorded LLM policy never abstains. "
             "Default off.",
    )
    parser.add_argument(
        "--last-action",
        choices=("off", "on"),
        default="off",
        help="graft B: render a [LAST ACTION] effect/escalation block from "
             "the real outcome of the previously executed LLM step "
             "(effect: confirmed / suspected_noop; escalation hint when it "
             "failed). Default off.",
    )
    parser.add_argument(
        "--trace-out",
        type=Path,
        default=None,
        metavar="PATH",
        help="live trace accumulation (core/trace_recorder.py): append one "
             "redacted JSONL record per shadow decision to PATH (default "
             "evaluation/tier-a/live-traces.jsonl is the operator default; "
             "use a temp path for eval runs — held-out task ids are labeled "
             "split=heldout by the recorder and never become train exemplars). "
             "Default None = recording off.",
    )
    parser.add_argument("--quiet", action="store_true", help="suppress progress output")
    args = parser.parse_args(argv)

    if args.steps < 20:
        parser.error("acceptance criteria require >= 20 decide steps per task")

    logging.basicConfig(
        level=logging.WARNING if args.quiet else logging.INFO,
        format="%(levelname)s %(name)s: %(message)s",
    )

    if args.few_shot < 0:
        parser.error("--few-shot must be >= 0")
    if args.few_shot > 0 and args.head != "kimi":
        print(f"note: --few-shot applies to the kimi head only; "
              f"--head {args.head} ignores it.")
    if args.self_consistency < 1:
        parser.error("--self-consistency must be >= 1")
    for _only_mlx in ("model", "revision", "self_consistency"):
        if getattr(args, _only_mlx) not in (None, 1) and args.head != "mlx":
            print(f"note: --{_only_mlx.replace('_', '-')} applies to the mlx head only; "
                  f"--head {args.head} ignores it.")

    head, stem_suffix = build_head(
        args.head,
        questioning=args.questioning,
        few_shot=args.few_shot if args.head == "kimi" else 0,
        few_shot_order=args.few_shot_order,
        few_shot_framing=args.few_shot_framing,
        traces_path=args.traces,
        trajectory=args.trajectory == "on",
        model=args.model if args.head == "mlx" else None,
        revision=args.revision if args.head == "mlx" else None,
        self_consistency=args.self_consistency if args.head == "mlx" else 1,
        sc_temperature=args.sc_temperature,
    )
    if args.questioning != "batched" and args.head != "kimi":
        print(f"note: --questioning {args.questioning} applies to the kimi head only; "
              f"--head {args.head} ignores it.")
    if args.trajectory != "off" and args.head != "kimi":
        print(f"note: --trajectory {args.trajectory} applies to the kimi head only; "
              f"--head {args.head} ignores it.")

    # Composed graft suffix — explicit and ordered (A before B) so every
    # flag combination gets its own non-overwriting report stem.
    graft_suffix = ""
    if args.reserved_slots == "on":
        graft_suffix += "A"
    if args.last_action == "on":
        graft_suffix += "B"
    if graft_suffix:
        graft_suffix = f"-graft{graft_suffix}"

    from core.shadow_eval import default_tasks, run_eval, write_reports

    if args.tasks == "heldout":
        task_list = default_tasks(args.steps)
        tasks_suffix = ""
    elif args.tasks == "train":
        from core.train_tasks import train_tasks

        task_list = train_tasks(args.task_seed, args.steps)
        tasks_suffix = "-train"
    else:
        from core.train_tasks import train_tasks

        task_list = default_tasks(args.steps) + train_tasks(args.task_seed, args.steps)
        tasks_suffix = "-all"

    step_budget = _STEP_BUDGET_MS[args.head]
    if args.head == "kimi" and args.questioning == "sequential":
        step_budget *= 2  # two subprocess calls per decide step

    report = run_eval(
        tasks=task_list,
        steps_per_task=args.steps,
        head=head,
        head_label=args.head,
        step_budget_ms=step_budget,
        progress=not args.quiet,
        reserved_slots=args.reserved_slots == "on",
        last_action=args.last_action == "on",
        trace_path=str(args.trace_out) if args.trace_out else None,
    )
    report["questioning"] = args.questioning
    report["trajectory"] = args.trajectory
    json_path, md_path = write_reports(report, args.out_dir, stem=f"shadow-eval-report{stem_suffix}{tasks_suffix}{graft_suffix}")

    agg = report["aggregate"]
    print(f"\nShadow eval complete — reports written:")
    print(f"  JSON: {json_path}")
    print(f"  MD:   {md_path}")
    print(f"\nAggregate over {agg['tasks']} tasks / {agg['total_decide_steps']} decide steps:")
    print(f"  agreement rate:                {agg['agreement_rate']}")
    print(f"  agreement given LLM success:   {agg['agreement_given_llm_success']}")
    print(f"  agreement given LLM failure:   {agg['agreement_given_llm_failure']}")
    print(f"  stuck=true given LLM success:  {agg['stuck_true_rate_given_llm_success']}")
    print(f"  stuck=true given LLM failure:  {agg['stuck_true_rate_given_llm_failure']}")
    print(f"  goal-satisfied=true rate:      {agg['goal_satisfied_true_rate']}")
    print(f"  mean head latency:             {agg['mean_head_latency_ms']} ms")
    print(f"  mean LLM latency:              {agg['mean_llm_latency_ms']} ms")
    if "vocab_miss_count" in agg:
        print(f"  vocab misses:                  {agg['vocab_miss_count']}")
    if args.reserved_slots == "on":
        print(f"  reobserve picks:               {agg['reobserve_picks']} / {agg['total_decide_steps']}")
        print(f"  abstain picks:                 {agg['abstain_picks']} / {agg['total_decide_steps']}")
        print(f"  reserved-slot pick rate:       {agg['reserved_slot_rate']}")
    if args.trace_out:
        print(f"  live traces appended:          {report['trace_records']} -> {args.trace_out}")
    print(f"\nNote: {agg['note']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
