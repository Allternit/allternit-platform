#!/usr/bin/env python3
"""Upload one file to R2 in retried 16 MB parts (boto3).

A single-request curl PUT of a ~760 MB DMG kept failing mid-stream with TLS
"bad record mac" (2026-10-08); small multipart parts with retries get through.
Usage: r2-upload.py <file> <bucket> <key> <content-type> [cache-control]
Env: R2_ENDPOINT, R2_ACCESS_KEY, R2_SECRET.
"""
import os
import sys

import boto3
from boto3.s3.transfer import TransferConfig
from botocore.config import Config

path, bucket, key, ctype = sys.argv[1:5]
extra = {"ContentType": ctype}
if len(sys.argv) > 5:
    extra["CacheControl"] = sys.argv[5]
s3 = boto3.client(
    "s3",
    endpoint_url=os.environ["R2_ENDPOINT"],
    aws_access_key_id=os.environ["R2_ACCESS_KEY"],
    aws_secret_access_key=os.environ["R2_SECRET"],
    region_name="auto",
    config=Config(retries={"max_attempts": 15, "mode": "adaptive"}),
)
s3.upload_file(path, bucket, key, ExtraArgs=extra,
               Config=TransferConfig(multipart_threshold=16 << 20, multipart_chunksize=16 << 20, max_concurrency=2))
