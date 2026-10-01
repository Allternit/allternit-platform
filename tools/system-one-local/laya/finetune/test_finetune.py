"""End-to-end proof of the fine-tune pipeline on a tiny synthetic checkpoint + synthetic rows.

SYNTHETIC FIXTURES LIVE ONLY IN THIS FILE. No download: the "base checkpoint" is a 2-layer,
32-wide ModernBERT with a word-level tokenizer, built here and saved in Laya's on-disk layout,
so the real `laya.Agent` loader, `_encode_state`, `collate_items` and `DecisionModel` run.

Run with the Laya venv:  python -m unittest discover -s laya/finetune   (or pytest)
"""
import json
import os
import random
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))

try:
    import torch  # noqa: F401
    import laya  # noqa: F401
    HAVE_LAYA = True
except Exception:  # pragma: no cover
    HAVE_LAYA = False

WORDS = ["is", "the", "build", "red", "green", "error", "ok", "retry", "stop", "continue", "pick", "next",
         "step", "yes", "no", "statement", "does", "not", "hold", "holds", "false", "true", "level", "a", "b",
         "should", "we", "tests", "pass", "fail", "noul", "choice", "score", "the", "statement"]


def load(p):
    with open(p) as f:
        return json.load(f)


def make_fake_checkpoint(d: str) -> None:
    from tokenizers import Tokenizer, models, pre_tokenizers
    from transformers import ModernBertConfig, PreTrainedTokenizerFast
    from laya.common import DecisionModel
    from transformers import AutoModel
    from safetensors.torch import save_file

    specials = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"]
    vocab = {w: i for i, w in enumerate(specials + sorted(set(WORDS)) + [str(i) for i in range(10)] + list(":,.?_"))}
    tk = Tokenizer(models.WordLevel(vocab=vocab, unk_token="[UNK]"))
    tk.pre_tokenizer = pre_tokenizers.Sequence([pre_tokenizers.Whitespace()])
    fast = PreTrainedTokenizerFast(tokenizer_object=tk, pad_token="[PAD]", unk_token="[UNK]", cls_token="[CLS]",
                                   sep_token="[SEP]", mask_token="[MASK]")
    fast.save_pretrained(os.path.join(d, "tokenizer"))
    ecfg = ModernBertConfig(vocab_size=len(vocab), hidden_size=64, intermediate_size=96, num_hidden_layers=2,
                            num_attention_heads=2, max_position_embeddings=512, pad_token_id=0, cls_token_id=2,
                            sep_token_id=3, bos_token_id=2, eos_token_id=3, global_attn_every_n_layers=1)
    ecfg.save_pretrained(os.path.join(d, "encoder"))
    torch.manual_seed(0)
    model = DecisionModel(AutoModel.from_config(ecfg, attn_implementation="sdpa"), head_layers=1, n_act=2)
    save_file({k: v.contiguous() for k, v in model.state_dict().items()}, os.path.join(d, "model.safetensors"))
    with open(os.path.join(d, "rl_agent_config.json"), "w") as f:
        json.dump({"encoder": "tiny-test-encoder", "head_layers": 1, "act_costs": {"ask": 0.1}, "max_len": 128,
                   "head_max_len": 48, "temperature": [1.3, 1.3, 1.3]}, f)


def synth_export(d: str, n: int = 48, seed: int = 3) -> None:
    """Noul rows whose truth is visible in the state ("tests pass" -> true), split like `system-one export`."""
    r = random.Random(seed)
    os.makedirs(d, exist_ok=True)
    rows = []
    for i in range(n):
        ok = r.random() < 0.5
        truth = "true" if ok else "false"
        rows.append({
            "decision_id": f"d{i}", "ts": f"2026-10-01T00:00:{i:02d}Z", "primitive_id": "bank.test", "operation": "GATE",
            "question": {"question_id": "q", "instructions": "should we retry"}, "candidates": [], "options": ["true", "false"],
            "label": truth, "label_index": 0 if ok else 1, "state": "tests pass ok" if ok else "tests fail error red",
            "laya": {"question": {"type": "noul", "instructions": "should we retry"}, "label_index": 1 if ok else 0,
                     "option_order": [1, 0]},
            "scope": {"backend_id": "backend.laya", "model_ref": "x", "model_revision": "base", "tokenizer_id": "t",
                      "quantization": "none", "runtime_backend": "laya-serve", "question_id": "q",
                      "candidate_schema_hash": "h", "candidate_set_hash": "s", "threshold_profile": "threshold.default"},
        })
    for name, part in (("train", rows[:32]), ("tune", rows[32:40]), ("cert", rows[40:])):
        with open(os.path.join(d, f"{name}.jsonl"), "w") as f:
            f.writelines(json.dumps(x) + "\n" for x in part)


@unittest.skipUnless(HAVE_LAYA, "needs the Laya venv (torch + laya)")
class FineTunePipeline(unittest.TestCase):
    def test_end_to_end_tiny(self):
        import finetune_laya as ft

        with tempfile.TemporaryDirectory() as t:
            base, exp, out = (os.path.join(t, x) for x in ("base", "export", "out"))
            os.makedirs(base)
            make_fake_checkpoint(base)
            synth_export(exp)
            rc = ft.main(["--export-dir", exp, "--out", out, "--base", base, "--device", "cpu", "--epochs", "15",
                          "--unfreeze-last", "2", "--lr", "2e-3", "--batch", "8", "--model-ref", "laya/typed-decisions"])
            self.assertEqual(rc, 0)
            for f in ("rl_agent_config.json", "model.safetensors", "tokenizer", "encoder", "allternit_checkpoint.json"):
                self.assertTrue(os.path.exists(os.path.join(out, f)), f)
            meta = load(os.path.join(out, "allternit_checkpoint.json"))
            self.assertTrue(meta["revision"].startswith("ft-"))
            self.assertEqual(meta["data"]["train"]["rows"], 32)
            cfg = load(os.path.join(out, "rl_agent_config.json"))
            self.assertEqual(cfg["temperature"], [1.0, 1.0, 1.0])
            losses = meta["train_stats"]["epoch_loss"]
            self.assertLess(losses[-1], losses[0], "training must reduce the loss on a learnable task")
            cert = ft.read_jsonl(os.path.join(out, "scored-cert.jsonl"))
            self.assertEqual(len(cert), 8)
            for row in cert:
                self.assertEqual(row["scope"]["model_revision"], meta["revision"])
                self.assertAlmostEqual(sum(row["readout"]["probs"]), 1.0, places=4)
            acc = sum(max(range(2), key=lambda i: r["readout"]["probs"][i]) == r["label_index"] for r in cert) / len(cert)
            self.assertGreaterEqual(acc, 0.75, "fine-tuned head should learn the visible rule")

    def test_ledger_order_mapping(self):
        import finetune_laya as ft

        # noul: Laya order is [false, true]; ledger options are ["true", "false"].
        self.assertEqual(ft.to_ledger_order([0.2, 0.8], [1, 0]), [0.8, 0.2])
        self.assertEqual(ft.to_ledger_order([0.1, 0.2, 0.7], [0, 1, 2]), [0.1, 0.2, 0.7])


if __name__ == "__main__":
    unittest.main()
