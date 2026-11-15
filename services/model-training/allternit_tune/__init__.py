# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""allternit-tune: Allternit's own fine-tuning engine for Freeweights Studio.

It speaks engine API v1 (services/studio-spec/openapi/engine-api.yaml) and
trains with TRL + PEFT on torch (CUDA, or Apple Silicon through MPS).
"""

ENGINE_ID = "allternit-tune"
ENGINE_NAME = "Allternit Tune"
__version__ = "0.1.0"
