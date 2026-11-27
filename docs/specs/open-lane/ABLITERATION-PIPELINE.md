# Abliteration pipeline for Allternit "-open" model variants

Status: spec only. Nothing here has been run. Date: 2026-11-26.
Part of the Allternit Open plan (step H2b).

> **No run without Eoj's explicit go. No rented GPUs. No heavy runs on the owner's Mac. Verification before a go = dry-run with the fake engine / stub server only.**

## 1. Purpose

Allternit makes its own abliterated ("-open") variants of open-weight models. The same weights serve two places:

- **Free users, locally.** Freeweights Studio and the on-device runtime load the GGUF quants.
- **Cloud open-lane rows, when Allternit self-hosts.** The safetensors are served by vLLM or SGLang as an OpenAI-compatible server. It is registered under the provider prefix `OPEN_SELF` (see the H2a runbook, `surfaces/docs/operations/open-lane-upstreams.mdx`).

One build, one model card, two uses. A variant that fails the evals in section 4 ships to neither.

## 2. Base-model shortlist

Licenses below were read from the `license` field of each Hugging Face model card (API, 2026-11-26). Parameter counts are the safetensors totals on the card (total, not active). "Commercial serving of modified weights" is our reading of the license text and needs a legal pass before the first ship. It is not legal advice.

| Model (HF id) | Params (total) | License | Commercial serving of modified weights | Notes |
|---|---|---|---|---|
| Qwen/Qwen3-8B | 8.2B | Apache-2.0 | Yes | Small local default. Dense. |
| Qwen/Qwen3.6-27B | 27.8B | Apache-2.0 | Yes | Dense mid-size. |
| Qwen/Qwen3.6-35B-A3B | 36.0B (MoE, ~3B active) | Apache-2.0 | Yes | Cheap to serve; MoE routing needs a check that ablation does not break experts. |
| openai/gpt-oss-20b | 20.9B (MoE) | Apache-2.0 | Yes | Ships MXFP4 weights. Card also links a usage policy: read it, not verified in detail. |
| openai/gpt-oss-120b | 116.8B (MoE) | Apache-2.0 | Yes | Fits one 80 GB GPU in MXFP4. Same usage-policy note. |
| mistralai/Mistral-Small-3.2-24B-Instruct-2506 | 24.0B | Apache-2.0 | Yes | Dense mid-size. |
| zai-org/GLM-5.3-Flash | 321.3B (MoE) | MIT | Yes | Large. Needs a multi-GPU job. |
| deepseek-ai/DeepSeek-V4.1-Flash | 763.2B (MoE) | MIT | Yes | Largest on the list. Pipeline cost is high; decide after the smaller ones. |
| meta-llama/Llama-3.3-70B-Instruct | 70.6B | Llama 3.3 Community License | Conditional: derivatives must carry the license, the "Built with Llama" notice and a "Llama" name prefix; extra terms above 700M monthly users. Not read in full: not verified. | Keep as optional. Name rules change our alias scheme. |
| google/gemma-3-27b-it | 27.4B | Gemma Terms of Use | Not verified. Terms allow commercial use but flow down a use policy to modified weights. | Not on the first list until read in full. |

Excluded (non-commercial): MiniMax-M3.1 and Qwen-Image, per the earlier Freeweights license checks. Any model whose card license is non-commercial (CC BY-NC, research-only) is out, including after a card change.

Order of work if Eoj gives a go: Qwen3-8B first (cheap, tests the whole path), then Qwen3.6-27B and gpt-oss-20b, then gpt-oss-120b. The 300B+ models wait for the traffic numbers in the hosting decision page (H2c).

## 3. Method

**Refusal-direction ablation.** Refusals in chat models are mediated by one direction in the residual stream (Arditi et al., 2024, "Refusal in Language Models Is Mediated by a Single Direction", arXiv 2406.11717). Steps:

1. Run a harmful prompt set and a harmless prompt set through the model. Record the residual-stream activation at the last prompt token, at each layer, for each prompt.
2. Compute the difference of means (harmful minus harmless) per layer. Normalize each to unit length. These are candidate refusal directions.
3. Pick the layer and direction. Score candidates on a held-out split: ablating the direction must cut refusals on a validation set while keeping KL divergence on harmless prompts small. Pick the lowest-KL candidate that meets the refusal target. Candidates usually come from the middle layers, roughly 40% to 70% of depth, but the score decides, not the rule of thumb.
4. Orthogonalize the weights that write into the residual stream (embedding, attention output, MLP output) against the chosen direction, so the change is permanent and no inference-time hook is needed. This keeps the architecture identical, so GGUF conversion and vLLM/SGLang loading work unchanged.
5. For MoE models, orthogonalize every expert's output projection and the shared experts, and re-check that routing statistics did not move.

Reference implementations (read before building, license check each): the original code from the Arditi et al. paper, FailSpy's `abliterator`, and Heretic. We write our own script in the Allternit repo and cite these. Vendoring any of them waits on a license read: not verified.

**Prompt sets.** Use public sets only, and keep a license column in the run record:

| Role | Set | License |
|---|---|---|
| Harmful (direction + validation) | AdvBench harmful behaviors | MIT (repo license; not verified on the HF copy) |
| Harmful (direction + validation) | HarmBench | MIT (repo license; not verified on the HF copy) |
| Harmless | databricks-dolly-15k | CC BY-SA 3.0 |
| Harmless (avoid) | Alpaca-derived sets such as `harmless_alpaca` | CC BY-NC 4.0 upstream: do not use |

Prompts are inputs to activation capture only. They are not training data and are not shipped in the weights.

**What stays outside the model.** Abliteration removes the refusal habit from the weights. It does not remove Allternit's rules. The gateway hard-line filter runs on every open-lane request, whatever weights serve it: CSAM, mass-casualty weapons, sexual content involving real people, and all sexual content. The 18+ gate and open-lane terms stay too. We do not change the filter rules in this work, and no eval result is ever a reason to relax them.

## 4. Evals

Run every candidate and the unmodified base with the same harness, seeds and decoding settings. Store the results in the run record.

| Eval | What | Set |
|---|---|---|
| Refusal rate, before and after | Benign-but-often-refused prompts (medical, security research, fiction, law, chemistry safety) | XSTest (CC BY 4.0) plus an Allternit-written set of 200 benign prompts. Write the Allternit set ourselves so the license is ours. |
| Capability loss | Three standard benchmarks | MMLU-Pro (knowledge), GSM8K (math reasoning), IFEval (instruction following). Same few-shot setting before and after. |
| Loop / token-limit rate | Share of 500 mixed prompts that repeat until the token cap or never stop | Mixed from Dolly and the benign set, cap 4,096 output tokens. |
| Hard-line filter pass-through | Send known hard-line prompts through the full gateway, with the variant as upstream | Allternit internal set. Pass = the filter blocks 100% of them before or after the model. The model's own output is not trusted for this test. |

**Ship rule.** Keep a variant only if all of these hold against the base:

- Average drop across MMLU-Pro, GSM8K and IFEval is at most 3 points, and no single benchmark drops more than 5.
- Loop / token-limit rate rises by at most 1 point.
- Refusal rate on the benign set falls by at least 80% relative.
- The hard-line filter pass-through test is 100%.

Fail any line and the variant is dropped, or we retry with another layer (max 3 attempts per model). Thresholds are a starting point; Eoj can change them in this file before the first run.

## 5. Outputs

Per model, one release folder:

- **safetensors**, bf16 (or the base's native format, MXFP4 for gpt-oss), for vLLM/SGLang.
- **GGUF quants** for local use: Q8_0, Q6_K, Q5_K_M, Q4_K_M, Q3_K_M, plus IQ4_XS for the larger models. Q4_K_M is the Free-tier default.
- `config`, tokenizer and chat template copied unchanged from the base.
- A `RUN.json` with the layer, direction scores, prompt-set versions, eval numbers and the commit of the script.
- The model card.

**Model card template**

```
name:            <base-name>-open
base:            <HF id> @ <commit sha>
license:         <same license as base, copied unchanged; license text included>
what_changed:    "Refusal direction removed from the weights (layer <n>). Fewer refusals on
                  borderline-but-legitimate requests. No other training. Allternit's gateway
                  safety filter still applies on Allternit services."
evals:           refusal rate before/after, MMLU-Pro, GSM8K, IFEval, loop rate (table)
limits:          can still refuse or be wrong; not safe to run without your own controls
files:           safetensors, GGUF (types listed)
```

The license is carried from the base. For Llama, add the "Built with Llama" notice and the name prefix. Publish to Freeweights with the `what_changed` field, the same way `alt.freeweights.abliterated` does.

## 6. Compute

> **No run without Eoj's explicit go. No rented GPUs. No heavy runs on the owner's Mac. Verification before a go = dry-run with the fake engine / stub server only.**

Ablation needs forward passes over a few hundred prompts and one weight edit. It is not training. Cost is dominated by loading the model and by the evals, not by the edit. The figures below are our estimates, not measurements. Basis: bf16 weights need about 2 bytes per parameter in VRAM plus 10-20% for activations; capture of a few hundred short prompts takes minutes; the evals take most of the wall time. We have not benchmarked any of it.

| Model class | GPU (estimate) | VRAM needed | Wall time per attempt (estimate) |
|---|---|---|---|
| 4B-9B dense | 1x 24 GB | ~20 GB | under 1 hour |
| 20B-36B (dense or MoE) | 1x 80 GB | ~60-75 GB | 1-3 hours |
| 70B-120B | 1x 141 GB, or 2x 80 GB | ~140-240 GB (gpt-oss-120b ~65 GB in MXFP4) | 3-6 hours |
| 300B-800B MoE | 8x 141 GB, one node | ~650 GB-1.6 TB | 8-20 hours |

Plan for up to 3 attempts per model (section 4). Dollar figures live in the hosting decision page (H2c), which uses real rental prices; this spec does not rent anything.

**Before any go (what we can do now):** write the script, then run it against the fake engine and the stub OpenAI-compatible server used by the local-engine tests. That checks the plumbing: prompt loading, activation-capture hooks, report format, GGUF conversion command lines. It proves nothing about model quality. The first real run needs Eoj's written go, naming the model, the machine and the hours.

Sources for the estimates: model-card parameter counts (HF, 2026-11-26); Arditi et al. 2024 for the procedure and its low compute cost; the vLLM and SGLang docs for memory sizing.

## 7. Self-host serving sketch

The existing recipes are in `services/local-engine/src/runtime/manager.rs` (the `Vllm` recipe: model path, tensor parallel size, GPU memory utilization, dtype, quantization; the `Sglang` recipe: model path, tensor parallel size, context length). The command pattern for a Cloud host:

```
# vLLM (OpenAI-compatible server)
vllm serve /models/<name>-open \
  --served-model-name <catalog-alias> \
  --tensor-parallel-size <N> \
  --gpu-memory-utilization 0.90 \
  --max-model-len <ctx> \
  --host 127.0.0.1 --port 8000

# SGLang
python -m sglang.launch_server --model-path /models/<name>-open \
  --served-model-name <catalog-alias> \
  --tp <N> --context-length <ctx> \
  --host 127.0.0.1 --port 8000
```

- **Served model name = the catalog alias** (for example `qwen3-6-27b-open`). The gateway sends the alias as `model`, so a row can move between hosts with no client change.
- **Registration:** the server is added as an `OPEN_SELF_*` upstream (base URL and key) per the H2a runbook. It is one more host in the row's failover order, not a special case.
- **Health check:** `GET /health` returns 200 on both servers; also send a 1-token completion to `/v1/chat/completions` with the alias. Both must pass before a host enters rotation.
- **Network:** bind to localhost or a private network behind the gateway. Never expose the server to the internet.
- Flags above are the usual ones for current vLLM and SGLang; confirm them against the pinned version at deploy time: not verified here, no server was run.

## 8. Copy rule

In any product text, docs, store listing or marketing, say "far fewer refusals". Never say "uncensored", "no limits", or anything adult. The model card's `what_changed` line is the template. The hard-line filter and the 18+ terms are part of the story, not a footnote.
