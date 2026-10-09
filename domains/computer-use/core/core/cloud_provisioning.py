"""BYOC (Bring Your Own Cloud) credential validation.

Live "Test Connection" checks for the customer cloud credentials stored via
gateway/cloud_credentials_router.py: one real, lightweight API call per
provider (AWS/GCP/Azure) that creates no resources. The SDKs are optional
(the ``byoc`` extra) and imported lazily.

The per-provider VM provisioning strategies that used to live here served the
canonical environments surface, which was removed in the D0 cleanup.
"""

from __future__ import annotations

import asyncio
import importlib.util
import json
from typing import Any, Dict, Optional


def _require(module_name: str, extra_hint: str) -> None:
    if importlib.util.find_spec(module_name.split(".")[0]) is None:
        raise RuntimeError(
            f"'{module_name}' is not installed. Install the optional BYOC "
            f"dependencies with: pip install 'allternit-computer-use[byoc]' "
            f"({extra_hint})"
        )


# ---------------------------------------------------------------------------
# Live credential validation -- "Test Connection" in the Settings UI. Runs
# against the raw, not-yet-saved secret the user just typed in: a real,
# lightweight, no-resources-created API call per provider.
# ---------------------------------------------------------------------------

async def validate_credential(
    provider: str,
    secret: Dict[str, Any],
    region: Optional[str] = None,
    external_id: Optional[str] = None,
) -> Dict[str, Any]:
    if provider == "aws":
        return await _validate_aws(secret, region, external_id)
    if provider == "gcp":
        return await _validate_gcp(secret)
    if provider == "azure":
        return await _validate_azure(secret)
    raise ValueError(f"Unsupported cloud provider {provider!r}")


async def _validate_aws(secret: Dict[str, Any], region: Optional[str], external_id: Optional[str] = None) -> Dict[str, Any]:
    _require("boto3", "needs boto3")
    import boto3

    role_arn = secret.get("role_arn")
    if not role_arn:
        raise ValueError("secret.role_arn is required")

    def _do() -> Dict[str, Any]:
        sts = boto3.client("sts", region_name=region or "us-east-1")
        kwargs: Dict[str, Any] = {"RoleArn": role_arn, "RoleSessionName": "allternit-validate"}
        if external_id:
            kwargs["ExternalId"] = external_id
        assumed = sts.assume_role(**kwargs)
        creds = assumed["Credentials"]
        assumed_sts = boto3.client(
            "sts",
            region_name=region or "us-east-1",
            aws_access_key_id=creds["AccessKeyId"],
            aws_secret_access_key=creds["SecretAccessKey"],
            aws_session_token=creds["SessionToken"],
        )
        identity = assumed_sts.get_caller_identity()
        return {"account_id": identity["Account"], "arn": identity["Arn"]}

    return await asyncio.to_thread(_do)


async def _validate_gcp(secret: Dict[str, Any]) -> Dict[str, Any]:
    _require("google.oauth2", "needs google-cloud-compute")
    from google.oauth2 import service_account
    import google.auth.transport.requests

    info = secret.get("service_account_json")
    if not info:
        raise ValueError("secret.service_account_json is required")
    if isinstance(info, str):
        info = json.loads(info)

    def _do() -> Dict[str, Any]:
        creds = service_account.Credentials.from_service_account_info(
            info, scopes=["https://www.googleapis.com/auth/cloud-platform"]
        )
        creds.refresh(google.auth.transport.requests.Request())
        return {"project_id": info.get("project_id"), "client_email": info.get("client_email")}

    return await asyncio.to_thread(_do)


async def _validate_azure(secret: Dict[str, Any]) -> Dict[str, Any]:
    _require("azure.identity", "needs azure-identity")
    from azure.identity import ClientSecretCredential

    tenant_id = secret.get("tenant_id")
    client_id = secret.get("client_id")
    client_secret = secret.get("client_secret")
    missing = [k for k, v in (("tenant_id", tenant_id), ("client_id", client_id), ("client_secret", client_secret)) if not v]
    if missing:
        raise ValueError(f"secret missing required keys: {missing}")

    def _do() -> Dict[str, Any]:
        cred = ClientSecretCredential(tenant_id=tenant_id, client_id=client_id, client_secret=client_secret)
        token = cred.get_token("https://management.azure.com/.default")
        return {"tenant_id": tenant_id, "token_expires_on": token.expires_on}

    return await asyncio.to_thread(_do)
