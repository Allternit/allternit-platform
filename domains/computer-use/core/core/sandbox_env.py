"""Run-scoped sandbox environment for Allternit Computer Use runs.

`sandbox_env` is the credential-injection channel from the ACI API server
(``cmd/allternit-api`` ``aci_credentials.rs``, PR #177): plaintext
credential values resolved server-side at run-provision time arrive as a
top-level ``sandbox_env`` map on the ``/v1/computer-use/execute`` payload
(see ``aci_routes.rs``). The values are environment material for the run's
child processes only — the same role ``extra_env`` plays in the Rust
``vm_session_routes`` ``/etc/environment`` flow for cloud VM sessions.

Contract (mirrors the Rust side):

* Values reach the run environment through :func:`sandbox_env_context`,
  which sets ``os.environ`` for the duration of the run so every child
  process the adapters launch (Playwright browser processes, desktop
  helper subprocesses such as osascript) inherits them. There is no Python
  VM/microVM session path: the Firecracker/container sandbox under
  ``sandbox/`` is a separate provisioning concept and does not participate
  in the execute path, and the cloud-VM ``/etc/environment`` bootstrap is
  Rust-side only. Run-scoped ``os.environ`` injection is therefore the
  entire Python-side channel.
* Values NEVER enter the model/planning context (the ``task`` string —
  the field is top-level on the request, not part of ``options``), log
  records, SSE/stream frames, run receipts, or persisted run records.
  :func:`scrub_secrets` enforces the output boundaries at the run-store
  event queue and at every result/error assignment.

v1 tradeoff: concurrent runs in one gateway process share ``os.environ``,
so overlapping runs that set the same variable name see last-writer-wins
values for the overlap window. Runs are sequential in practice; per-run
isolation of the OS environment is a v2 concern.
"""

from __future__ import annotations

import os
import re
from contextlib import contextmanager
from typing import Any, Dict, Iterator, List, Mapping, Optional, Sequence

ENV_KEY_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
MAX_KEY_BYTES = 128
MAX_VALUE_BYTES = 16 * 1024
SCRUBBED = "***"


def validate_sandbox_env(env: Mapping[str, str]) -> Dict[str, str]:
    """Validate a sandbox_env map, returning a plain-dict copy.

    Raises ValueError naming only the offending variable name — variable
    names are metadata (the Rust side treats env keys as receipt-safe);
    values never appear in any error text.
    """
    validated: Dict[str, str] = {}
    for key, value in env.items():
        if (
            not isinstance(key, str)
            or not key
            or len(key.encode()) > MAX_KEY_BYTES
            or not ENV_KEY_RE.fullmatch(key)
        ):
            raise ValueError(
                f"Invalid sandbox_env variable name {key!r}: must be a shell "
                f"identifier ([A-Za-z_][A-Za-z0-9_]*, <= {MAX_KEY_BYTES} bytes)"
            )
        if not isinstance(value, str) or not value:
            raise ValueError(
                f"Invalid sandbox_env value for {key!r}: must be a non-empty string"
            )
        if len(value.encode()) > MAX_VALUE_BYTES:
            raise ValueError(
                f"Invalid sandbox_env value for {key!r}: exceeds "
                f"{MAX_VALUE_BYTES} byte limit"
            )
        validated[key] = value
    return validated


def secret_values(env: Mapping[str, str]) -> List[str]:
    """The plaintext values of a sandbox_env map, for scrubbing."""
    return [value for value in env.values() if value]


@contextmanager
def sandbox_env_context(env: Mapping[str, str]) -> Iterator[None]:
    """Set sandbox_env variables in os.environ for the run; restore after.

    Child processes launched while the context is active (Playwright
    browser processes, desktop subprocesses) inherit the variables through
    the process environment. Prior values are restored on exit; variables
    that did not exist before are removed. A no-op for an empty map.
    """
    if not env:
        yield
        return
    prior: Dict[str, Optional[str]] = {key: os.environ.get(key) for key in env}
    try:
        os.environ.update(env)
        yield
    finally:
        for key, old in prior.items():
            if old is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = old


def scrub_secrets(value: Any, secrets: Sequence[str]) -> Any:
    """Deep-copy `value` with every occurrence of any secret replaced by ***.

    Defense in depth for run-event frames, results, and errors: a child
    process that echoes its own environment cannot leak credential values
    into streamed, snapshotted, persisted, or replayed output. Mirrors
    ``scrub_frame`` in ``aci_credentials.rs``.
    """
    active = [secret for secret in secrets if secret]
    if not active:
        return value
    return _scrub(value, active)


def _scrub(value: Any, secrets: Sequence[str]) -> Any:
    if isinstance(value, str):
        for secret in secrets:
            if secret in value:
                value = value.replace(secret, SCRUBBED)
        return value
    if isinstance(value, dict):
        return {key: _scrub(item, secrets) for key, item in value.items()}
    if isinstance(value, list):
        return [_scrub(item, secrets) for item in value]
    return value
