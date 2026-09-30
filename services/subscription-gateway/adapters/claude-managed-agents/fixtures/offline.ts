// Offline AAI registration (fake SDK client over documented event shapes) for POST /aai/conformance/claude-managed-agents.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { createOfflineRig } from "./rig.js";

export function createOfflineAaiRegistration(): { registration: AaiRegistration; close?: () => Promise<void> } {
  const { provider, fixtures } = createOfflineRig();
  return { registration: { provider, fixtures } };
}
