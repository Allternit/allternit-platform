#!/usr/bin/env python3
"""Production-readiness soak: inference + billing chain against the LIVE API.

Runs on the control plane (mail). Exercises the real HTTP surface a user hits:
  1. synthetic paying user (users + user_credits + pro quota + api token)
  2. GET /api/v1/billing/credits reflects the seeded balance
  3. POST /v1/chat/completions (cheap Groq alias) -> 200, usage recorded,
     credits deducted by the retail amount
  4. free user (no credits row): cloud_models not allowed, the call is a
     402 credits_required before upstream spend (no free allowance since
     CONTRACT-cloud-credits-2026-11-15)
  5. exhausted-balance user: 402 credits_exhausted before upstream spend
  6. BYOK management endpoints respond (empty list; invalid key rejected)
  7. cleanup (DELETE users CASCADE smoke rows)

Exit 0 only if every check passes. Nothing here touches hosted-runtime
lifecycle; that chain is proven by tests/e2e_contabo_provision_heartbeat.rs.
"""

import hashlib
import json
import subprocess
import sys
import uuid

API = "http://localhost:8082"
PSQL = ["sudo", "-u", "postgres", "psql", "-d", "allternit", "-AtXc"]

PASS = "\u2705"
FAIL = "\u274c"
failures = []


def check(name, ok, detail=""):
    print(f"{PASS if ok else FAIL} {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(name)


def psql(sql):
    out = subprocess.run(PSQL + [sql], capture_output=True, text=True)
    if out.returncode != 0:
        raise RuntimeError(f"psql failed: {out.stderr.strip()}")
    return out.stdout.strip()


def api(method, path, token, body=None):
    cmd = [
        "curl", "-s", "-X", method, f"{API}{path}",
        "-H", f"Authorization: Bearer {token}",
        "-H", "Content-Type: application/json",
    ]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    try:
        return out.returncode, json.loads(out.stdout) if out.stdout.strip() else {}
    except json.JSONDecodeError:
        return out.returncode, {"_raw": out.stdout[:200]}


def seed_user(uid, balance=None):
    psql(
        f"INSERT INTO users (id, email, status) VALUES ('{uid}', '{uid}@soak.local', 'active') "
        f"ON CONFLICT (id) DO NOTHING"
    )
    psql(
        "INSERT INTO user_runtime_quotas (user_id, plan_tier_id, max_active_devices, "
        "max_pairings_per_day, max_relay_sockets, max_relay_mb_per_day, "
        "max_hosted_runtime_hours_monthly, can_create_hosted_runtime, max_hosted_runtimes, "
        "max_hosted_runtime_memory_mb) "
        "SELECT '" + uid + "', id, max_active_devices, max_pairings_per_day, max_relay_sockets, "
        "max_relay_mb_per_day, max_hosted_runtime_hours_monthly, can_create_hosted_runtime, "
        "max_hosted_runtimes, max_hosted_runtime_memory_mb FROM plan_tiers WHERE id = 'pro' "
        "ON CONFLICT (user_id) DO NOTHING"
    )
    if balance is not None:
        psql(
            f"INSERT INTO user_credits (user_id, balance_usd) VALUES ('{uid}', {balance}) "
            f"ON CONFLICT (user_id) DO UPDATE SET balance_usd = {balance}"
        )
    token = f"soak-{uuid.uuid4()}"
    psql(
        f"INSERT INTO api_tokens (id, token_hash, name, user_id) VALUES "
        f"('tok-{uuid.uuid4().hex}', '{hashlib.md5(token.encode()).hexdigest()}', 'soak', '{uid}')"
    )
    return token


def balance_of(uid):
    return float(psql(f"SELECT COALESCE(balance_usd, -1) FROM user_credits WHERE user_id = '{uid}'") or -1)


def main():
    paid_uid = f"soak_paid_{uuid.uuid4().hex[:8]}"
    free_uid = f"soak_free_{uuid.uuid4().hex[:8]}"
    dead_uid = f"soak_dead_{uuid.uuid4().hex[:8]}"

    # 1-2: seed + balance readback
    paid_token = seed_user(paid_uid, balance=5.00)
    rc, credits = api("GET", "/api/v1/billing/credits", paid_token)
    check("billing/credits readback", rc == 0 and abs(credits.get("balance_usd", -1) - 5.00) < 1e-9,
          f"balance={credits.get('balance_usd')}")
    check("paid user may run Cloud models", (credits.get("cloud_models") or {}).get("allowed") is True)

    # 3: paid inference end-to-end. Priced model: the gpt-oss Groq aliases are
    # free upstream and would deduct ~$0, defeating the deduction check.
    PRICED_MODEL = "qwen3.6-27b-groq"
    before = balance_of(paid_uid)
    rc, body = api("POST", "/v1/chat/completions", paid_token, {
        "model": PRICED_MODEL,
        "messages": [{"role": "user", "content": "Reply with exactly: soak ok"}],
        "max_tokens": 16,
    })
    usage = body.get("usage", {}) if isinstance(body, dict) else {}
    check("chat completion 200 with usage", rc == 0 and usage.get("total_tokens", 0) > 0,
          f"tokens={usage.get('total_tokens')}")
    after = balance_of(paid_uid)
    deducted = psql(
        f"SELECT COALESCE(SUM(-amount_usd), 0) FROM credit_transactions "
        f"WHERE user_id = '{paid_uid}' AND source = 'inference'"
    )
    delta = before - after
    check("credits deducted (retail, >0)", 1e-9 < delta < 0.05,
          f"${before:.6f} -> ${after:.6f} (delta ${delta:.6f}), ledger={deducted}")
    row = psql(
        f"SELECT COUNT(*) FROM inference_usage WHERE user_id = '{paid_uid}' "
        f"AND created_at >= NOW() - INTERVAL '2 minutes'"
    )
    check("inference_usage row recorded", row == "1")

    # 4: free user (no credits row) — refused before upstream spend
    free_token = seed_user(free_uid, balance=None)
    rc, credits = api("GET", "/api/v1/billing/credits", free_token)
    cm = credits.get("cloud_models") or {}
    check("no-row user is not entitled", rc == 0 and cm.get("allowed") is False and cm.get("reason") == "credits_required",
          json.dumps(cm)[:120])
    rc, body = api("POST", "/v1/chat/completions", free_token, {
        "model": PRICED_MODEL,
        "messages": [{"role": "user", "content": "Say hi"}],
        "max_tokens": 8,
    })
    err = body.get("error") if isinstance(body.get("error"), dict) else {}
    check("no-row user -> 402 credits_required with 4 packs",
          err.get("code") == "credits_required" and len(err.get("packs") or []) == 4, json.dumps(body)[:120])
    check("no usage, no balance created", balance_of(free_uid) == -1 and psql(
        f"SELECT COUNT(*) FROM inference_usage WHERE user_id = '{free_uid}'") == "0")

    # 5: exhausted balance blocked pre-dispatch
    dead_token = seed_user(dead_uid, balance=0.00)
    rc, body = api("POST", "/v1/chat/completions", dead_token, {
        "model": PRICED_MODEL,
        "messages": [{"role": "user", "content": "Say hi"}],
    })
    err = body.get("error") if isinstance(body.get("error"), dict) else {}
    check("zero balance -> 402 credits_exhausted", err.get("code") == "credits_exhausted", json.dumps(body)[:120])

    # 6: BYOK management endpoints
    rc, keys = api("GET", "/api/v1/inference/keys", paid_token)
    key_list = keys if isinstance(keys, list) else keys.get("keys", [])
    check("inference keys list", rc == 0 and key_list == [])
    rc, body = api("PUT", "/api/v1/inference/keys", paid_token,
                   {"provider_id": "groq", "api_key": "sk-invalid-soak"})
    check("invalid BYOK key rejected", rc == 0 and "error" in body, json.dumps(body)[:120])

    # 7: cleanup
    for uid in (paid_uid, free_uid, dead_uid):
        psql(f"DELETE FROM users WHERE id = '{uid}'")
    check("cleanup", True)

    print()
    if failures:
        print(f"SOAK FAILED: {len(failures)} check(s): {failures}")
        return 1
    print("SOAK PASSED: inference + billing chain verified end-to-end")
    return 0


if __name__ == "__main__":
    sys.exit(main())
