# Decisions scorer on the cloud VPS

Runs `domains/decision-runtime` (llama.cpp CPU engine, Qwen3.5-4B Q4_K_M) as a systemd
service on `127.0.0.1:8020`, and points allternit-api's `local` decision backend at it.

## Order

From a workstation (repo root):

    scp -r domains/decision-runtime infrastructure/decision-scorer root@<vps>:/root/
    # on the VPS, as root:
    cd /root/decision-scorer
    ./install.sh /root/decision-runtime      # first run: ~2.7 GB download, up to 30 min
    ./smoke.sh                               # health + one /v1/score, expect HTTP 200, yes > no
    ./wire-node.sh --restart                 # edits /etc/allternit-api/api.env, restarts allternit-api
    # then check POST /v1/decisions answers with backend:"local"

Re-running `install.sh` is safe (updates code, keeps the token and weights).
Optional: `ALLTERNIT_WEIGHTS_SHA256=<hex> ./install.sh ...` verifies the GGUF; with none set
it prints the sha256 so you can record it.

## Layout

- Code `/opt/allternit-decision-scorer/app`, venv `.../venv`
- State and weights `/var/lib/allternit-decision-scorer` (weights at `models/unsloth--Qwen3.5-4B-GGUF/`)
- Config and token `/etc/allternit-decision-scorer.env` (root, 0600). The scorer reads
  `ALLTERNIT_DECISIONS_TOKEN`; allternit-api sends the same name as the bearer.
- Every endpoint, including `/health`, needs `Authorization: Bearer <token>`.

## Resources

- RAM: about 3.5 to 4 GB resident (2.7 GB weights plus KV and snapshots); hard cap `MemoryMax=5G`.
- CPU: `CPUQuota=400%`, `CPUAffinity=0-3`, `Nice=5`. Scoring uses 4 cores while a request runs, idle otherwise.
- Thread count: `ALLTERNIT_DECISIONS_THREADS=4` in the env file (the engine's default is `cpu_count()-1`).
  On older Python it asks for 7 threads inside the 4-core quota (works, a bit slower).

## Logs

    journalctl -u allternit-decision-scorer -f
    systemctl status allternit-decision-scorer

## Rollback

    systemctl disable --now allternit-decision-scorer
    cp -p /etc/allternit-api/api.env.bak.<timestamp> /etc/allternit-api/api.env
    systemctl restart allternit-api

(`wire-node.sh` prints the backup name.) Removing the three env lines has the same effect;
with no `LOCAL_URL`, the `local` backend is skipped.

## Chain

`head,local,oracle`. `head` is an exact-match lookup over past decisions (no training data
needed, empty until decisions accumulate). `oracle` only works once
`ALLTERNIT_DECISIONS_ORACLE_MODEL` is set (`provider/model`), otherwise it is skipped.
`open`, `vendor` and `typesafe` are left out on purpose.
