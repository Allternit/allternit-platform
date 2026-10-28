# Triage eval

Measures how well Allternit sorts incoming mail into **needs you** and **can wait**. It runs the same code the server runs (`cmd/allternit-cloud-api/src/routes/mail_triage.rs`) against a labelled set. Adapted from OpenDuet's `evals/triage` (MIT, Copyright (c) 2026 CopilotKit; see `LICENSE-openduet`).

## The set

`emails.jsonl` holds 62 invented messages to Maya Okafor, who runs a small ceramics studio, on the morning of Thursday 22 October 2026. 27 need her and 35 can wait. Each has the label a careful assistant should give it and a reason a person would accept. `build-set.py` writes the file, so edit that and re-run it: `python3 evals/triage/build-set.py`.

It includes the cases that go wrong in practice: marketing that sounds urgent; `noreply` mail that does need her (failed payment, paused store, security alert); phishing; a message that tries to instruct the assistant; a customer's newsletter; a thread where she is only copied but asked by name; a request that says there is no rush; a security report through her team's Google Group; a vendor's "rotate your access tokens" notice with an unsubscribe header. No real mail is used.

## Running it

```sh
# Rules only: no model calls.
cargo run -p allternit-cloud-api --features triage-eval --bin mail-triage-eval -- --rules-only

# Rules, then the model (metered).
BOT_MODEL=<model> ALLTERNIT_API_KEY=<key> \
  cargo run -p allternit-cloud-api --features triage-eval --bin mail-triage-eval
```

The model is `TRIAGE_EVAL_MODEL`, then `BOT_MODEL`. It is called through the model router's OpenAI-compatible `/chat/completions` at `TRIAGE_EVAL_BASE_URL` (default `https://api.allternit.com/v1`) with `TRIAGE_EVAL_API_KEY` or `ALLTERNIT_API_KEY`. Results go to `results-<model>.md` (or `results-rules-only.md`) with the confusion counts and every verdict and its reason.

## What the numbers mean

Recall for needs you is the headline: how many of the messages that needed her were flagged. A low number means something important slipped past, and missing a failed payment costs more than reading one extra line. Precision is how many flagged messages really needed her; a low number means a brief people learn to ignore. The rules decide only "can wait", so their line shows how many messages skip the model and how many of those were wrong (it should stay at 0).

The set is small and invented, and the labels and the prompt were written together. Treat it as a regression check for changes to the rules, the prompt or the model, not as proof of accuracy on a real inbox. Corrections from real use are the next source of labels.
