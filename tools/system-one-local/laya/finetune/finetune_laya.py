#!/usr/bin/env python3
"""Fine-tune the S1 typed-decisions checkpoint (Laya, Apache-2.0) from the shadow ledger.

Input is the training set written by `system-one export` (one JSON row per labelled
decision, raw state included). Output is a local checkpoint directory that Laya serves
as-is (`LAYA_CHECKPOINT_PATH`, or Desktop `SystemOneManager.setCheckpoint({path})`):

    <out>/rl_agent_config.json   base config, temperatures reset to 1.0 (S1 owns calibration)
    <out>/model.safetensors      fine-tuned weights
    <out>/tokenizer/ <out>/encoder/   copied from the base checkpoint
    <out>/allternit_checkpoint.json   revision id (= new backend identity, Q29), base, data hashes
    <out>/scored-tune.jsonl <out>/scored-cert.jsonl   the NEW checkpoint's raw readouts on the
        held-out tuning (A) and certification (B) splits, in `system-one calibrate` row format,
        bound to the new revision. Q26 is run on these, never on the training rows.

By default only the decision head (type embedding, head transformer, scorer, action head)
trains and the encoder is frozen (`--unfreeze-last N` opens the last N encoder layers).
Loss is cross-entropy over the option logits (a strictly proper scoring rule).

    python finetune_laya.py --export-dir <dir> --out <ckpt dir> [--base <hub id|dir>]
        [--revision <hub rev>] [--device mps|cuda|cpu] [--epochs 3] [--lr 2e-4] [--batch 16]

Reproducible: fixed seed, deterministic row order, all inputs hashed into the manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import shutil
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

PINNED_BASE = "convaiinnovations/laya"
PINNED_REVISION = "55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851"
TYPE_IDS = {"choice": 0, "score": 1, "noul": 2}


def read_jsonl(path: str) -> List[Dict[str, Any]]:
    if not os.path.exists(path):
        return []
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def row_question(row: Dict[str, Any]) -> Dict[str, Any]:
    """The Laya question the export recorded for this decision (type, instructions, criteria)."""
    q = dict(row["laya"]["question"])
    if q.get("criteria") is None:
        q.pop("criteria", None)
    return q


def to_ledger_order(laya_probs: List[float], order: List[int]) -> List[float]:
    """Laya returns probabilities in its own option order; order[i] = Laya index of ledger option i."""
    return [float(laya_probs[j]) for j in order]


def resolve_base(base: str, revision: Optional[str]) -> str:
    if os.path.isdir(base):
        return base
    from huggingface_hub import snapshot_download

    return snapshot_download(base, revision=revision, allow_patterns=[
        "rl_agent_config.json", "model.safetensors", "tokenizer/*", "encoder/*"])


def encode_rows(agent, rows: List[Dict[str, Any]]) -> List[Tuple[Dict[str, Any], int]]:
    """One encoded Laya item per row plus its Laya-order label index. Rows that do not fit are skipped."""
    out = []
    for row in rows:
        q = row_question(row)
        try:
            agent._check_question("q", q)
            internal = {"q": agent._to_internal(q)}
            item = agent._encode_state(row["state"], ["q"], internal)[0]
        except ValueError as e:  # too many options for max_len etc.: never guessed, just dropped
            print(f"skip {row.get('decision_id')}: {e}", file=sys.stderr)
            continue
        label = int(row["laya"]["label_index"])
        if label >= len(item["markers"]):
            continue
        out.append((item, label))
    return out


def batches(encoded, size: int, shuffle: bool, rng: random.Random):
    idx = list(range(len(encoded)))
    if shuffle:
        rng.shuffle(idx)
    for i in range(0, len(idx), size):
        yield [encoded[j] for j in idx[i:i + size]]


def forward_logits(agent, group, detach_encoder: bool):
    import torch
    from laya.common import collate_items

    b = collate_items([[it for it, _ in group]], agent.tok.pad_token_id)
    dev = agent.device
    logits, _act = agent.model(b["input_ids"].to(dev), b["attention_mask"].to(dev), b["marker_pos"].to(dev),
                               b["marker_mask"].to(dev), b["qtype"].to(dev), detach_encoder=detach_encoder)
    labels = torch.tensor([lab for _, lab in group], device=dev)
    return logits, labels


def set_trainable(model, unfreeze_last: int) -> int:
    for p in model.parameters():
        p.requires_grad = False
    for name in ("head", "type_emb", "scorer", "act_head"):
        mod = getattr(model, name, None)
        if mod is not None:
            for p in mod.parameters():
                p.requires_grad = True
    if unfreeze_last > 0:
        layers = getattr(model.encoder, "layers", None)
        if layers is None:
            raise SystemExit("--unfreeze-last: encoder has no .layers")
        for layer in list(layers)[-unfreeze_last:]:
            for p in layer.parameters():
                p.requires_grad = True
        if hasattr(model.encoder, "final_norm"):
            for p in model.encoder.final_norm.parameters():
                p.requires_grad = True
    return sum(p.numel() for p in model.parameters() if p.requires_grad)


def train(agent, encoded, *, epochs: int, lr: float, batch: int, seed: int, unfreeze_last: int,
          max_minutes: float) -> Dict[str, Any]:
    import torch

    torch.manual_seed(seed)
    rng = random.Random(seed)
    n_train = set_trainable(agent.model, unfreeze_last)
    opt = torch.optim.AdamW([p for p in agent.model.parameters() if p.requires_grad], lr=lr, weight_decay=0.01)
    agent.model.train()
    t0, steps, losses = time.time(), 0, []
    stopped_early = False
    for ep in range(epochs):
        tot, n = 0.0, 0
        for group in batches(encoded, batch, True, rng):
            logits, labels = forward_logits(agent, group, detach_encoder=unfreeze_last == 0)
            loss = torch.nn.functional.cross_entropy(logits, labels)
            opt.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_([p for p in agent.model.parameters() if p.requires_grad], 1.0)
            opt.step()
            steps += 1
            tot += float(loss) * len(group)
            n += len(group)
            if (time.time() - t0) / 60 > max_minutes:
                stopped_early = True
                break
        losses.append(round(tot / max(n, 1), 5))
        print(f"epoch {ep + 1}/{epochs} loss {losses[-1]}", file=sys.stderr)
        if stopped_early:
            print(f"stopping: --max-minutes {max_minutes} reached", file=sys.stderr)
            break
    agent.model.eval()
    return {"trainable_params": n_train, "steps": steps, "epoch_loss": losses,
            "seconds": round(time.time() - t0, 1), "stopped_early": stopped_early}


def save_checkpoint(agent, base_dir: str, out: str, meta: Dict[str, Any]) -> str:
    from safetensors.torch import save_file

    os.makedirs(out, exist_ok=True)
    for sub in ("tokenizer", "encoder"):
        src = os.path.join(base_dir, sub)
        if os.path.isdir(src):
            shutil.copytree(src, os.path.join(out, sub), dirs_exist_ok=True)
    cfg = dict(agent.cfg)
    # S1 fits temperature per (bank, type, option count) on held-out data; the checkpoint serves raw logits.
    cfg["temperature"] = [1.0, 1.0, 1.0]
    cfg["temperature_by_options"] = {}
    with open(os.path.join(out, "rl_agent_config.json"), "w") as f:
        json.dump(cfg, f, indent=2)
    sd = {k: v.detach().cpu().contiguous() for k, v in agent.model.state_dict().items()}
    if "temperature" in sd:
        sd["temperature"] = sd["temperature"].new_ones(sd["temperature"].shape)
    weights = os.path.join(out, "model.safetensors")
    save_file(sd, weights)
    revision = "ft-" + sha256_file(weights)[:16]
    with open(os.path.join(out, "allternit_checkpoint.json"), "w") as f:
        json.dump({**meta, "revision": revision, "weights_sha256": sha256_file(weights)}, f, indent=2)
    return revision


def score_split(agent, rows: List[Dict[str, Any]], revision: str, model_ref: str) -> List[Dict[str, Any]]:
    """Raw (temperature 1) readouts of `agent` on held-out rows, as calibrate DatasetRows bound to `revision`."""
    import torch

    out = []
    with torch.no_grad():
        for row in rows:
            enc = encode_rows(agent, [row])
            if not enc:
                continue
            logits, _ = forward_logits(agent, enc, detach_encoder=True)
            k = len(enc[0][0]["markers"])
            p = torch.softmax(logits[0, :k].float(), -1).cpu().tolist()
            scope = {**row["scope"], "model_revision": revision, "model_ref": model_ref}
            out.append({
                "decision_id": row["decision_id"], "ts": row["ts"], "primitive_id": row["primitive_id"],
                "operation": row["operation"], "question": row["question"], "candidates": row["candidates"],
                "options": row["options"], "label": row["label"], "label_index": row["label_index"],
                "readout": {"probs": to_ledger_order(p, row["laya"]["option_order"]), "method": "finetune-score"},
                "scope": scope, "provenance": {**row.get("provenance", {}), "scored_by": revision},
                **({"incumbent": row["incumbent"]} if row.get("incumbent") is not None else {}),
            })
    return out


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--export-dir", required=True, help="directory written by `system-one export`")
    ap.add_argument("--out", required=True)
    ap.add_argument("--base", default=PINNED_BASE)
    ap.add_argument("--revision", default=None, help="hub revision of --base (default: the Q29 pin)")
    ap.add_argument("--device", default=None)
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--batch", type=int, default=16)
    ap.add_argument("--seed", type=int, default=1337)
    ap.add_argument("--unfreeze-last", type=int, default=0)
    ap.add_argument("--max-minutes", type=float, default=20.0)
    ap.add_argument("--model-ref", default="convaiinnovations/laya/typed-decisions",
                    help="scope.model_ref the S1 server reports for this backend")
    a = ap.parse_args(argv)

    from laya import Agent
    import laya

    train_rows = read_jsonl(os.path.join(a.export_dir, "train.jsonl"))
    tune_rows = read_jsonl(os.path.join(a.export_dir, "tune.jsonl"))
    cert_rows = read_jsonl(os.path.join(a.export_dir, "cert.jsonl"))
    if not train_rows:
        print("no training rows in the export; nothing to fine-tune", file=sys.stderr)
        return 3
    revision = a.revision if a.revision is not None else (PINNED_REVISION if a.base == PINNED_BASE else None)
    base_dir = resolve_base(a.base, revision)
    agent = Agent(base_dir, device=a.device)
    encoded = encode_rows(agent, train_rows)
    print(f"training rows: {len(encoded)} of {len(train_rows)} on {agent.device}", file=sys.stderr)
    stats = train(agent, encoded, epochs=a.epochs, lr=a.lr, batch=a.batch, seed=a.seed,
                  unfreeze_last=a.unfreeze_last, max_minutes=a.max_minutes)
    meta = {
        "schema": "allternit.s1.checkpoint.v1", "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "base": a.base, "base_revision": revision, "laya_version": getattr(laya, "__version__", None),
        "recipe": {"epochs": a.epochs, "lr": a.lr, "batch": a.batch, "seed": a.seed, "unfreeze_last": a.unfreeze_last,
                   "loss": "cross_entropy_over_options", "temperature": "reset to 1.0; S1 calibrates per (bank, type, k)"},
        "data": {name: {"rows": len(read_jsonl(os.path.join(a.export_dir, f"{name}.jsonl"))),
                        "sha256": sha256_file(os.path.join(a.export_dir, f"{name}.jsonl"))
                        if os.path.exists(os.path.join(a.export_dir, f"{name}.jsonl")) else None}
                 for name in ("train", "tune", "cert", "audit")},
        "train_stats": stats,
    }
    new_rev = save_checkpoint(agent, base_dir, a.out, meta)
    # Reload from disk: proves the directory is a loadable Laya checkpoint before anyone serves it.
    tuned = Agent(a.out, device=a.device)
    for name, rows in (("tune", tune_rows), ("cert", cert_rows)):
        scored = score_split(tuned, rows, new_rev, a.model_ref)
        with open(os.path.join(a.out, f"scored-{name}.jsonl"), "w") as f:
            for r in scored:
                f.write(json.dumps(r) + "\n")
    print(json.dumps({"out": a.out, "revision": new_rev, "train_rows": len(encoded), "tune_rows": len(tune_rows),
                      "cert_rows": len(cert_rows), **stats}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
