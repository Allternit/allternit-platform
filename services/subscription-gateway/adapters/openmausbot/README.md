# openmausbot native AAI adapter

Authenticates to the user's vendor runtime on the subscription gateway computer. Loopback origins only; redirects refused; credentials come from the sealed account call scope or explicit deployment configuration. Runtime discovery loads aai.ts automatically.

Setup, capabilities and limitations: surfaces/docs/guides/local-vendor-bots.mdx.
Tests: services/subscription-gateway/test/openmausbot.test.ts.

Source-based fixtures passed; live vendor-account verification remains pending. Never substitute a model-provider credential for a vendor bot credential. Send outcomes and context ownership are retained in memory; no cross-process resume or durable deduplication is claimed.
