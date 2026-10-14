# Steering checkpoint

## Goal
Unified channel API, Phase 1 audit + Phase 2 Slack slice (Brain spec Research/specs/allternit-unified-channel-api.md; Factory dag_818173). Extend the existing ChannelTransport/channel_gateway; no new gateway, no AgentChat dependency.

## Just did
Audit (docs/channels/UNIFIED_CHANNEL_API.md). Fixed cross-owner/cross-connection conversation routing (find_binding_for scoped to owner+account; ensure_binding_on, route_inbound, channel_start, lane, people use it; Slack team lookup scoped to the relayed owner). Added Capabilities per adapter, PostError::RateLimited/SendOutcome::RateLimited (429 + Retry-After, retryable by correlation id), fetch_since default = history_unsupported, delivery_status + GET thread channel/channel-messages routes, startup recover_interrupted_sends. Dev doc channel-gateway-api.mdx + user status section + nav + dependency map.

## Next
cargo test -p allternit-api channel tests; fix failures; report to Eoj. No commit/push/PR/deploy until Eoj says so.

## Open questions
Inbound record-before-ack split (all providers share dispatch_events_with_files) deferred to Phase 3. Live provider tests need authorized test accounts.
