-- MCP tunnel policy direction (2026-11-24, Apps + MCP 2026 plan v3 lanes 18-19,
-- decision D3: extend the tunnel policy instead of a new share_auth module).
--
-- Additive only. Existing rows become `outbound` and keep their behaviour: the
-- mTLS terminator asserts the cert thumbprint and OAuth issuer/audience in
-- headers and cmd/allternit-api/src/mcp_tunnel_auth.rs compares them offline.
--
-- `inbound_share` rows guard share links into this machine (share gate). They
-- never accept header-asserted claims: the bearer JWT's RS256 signature is
-- verified against the issuer's JWKS, fetched from `jwks_url` or, when NULL,
-- `<oauth_issuer>/.well-known/jwks.json` (https only; http only to loopback).
ALTER TABLE mcp_tunnel_auth
    ADD COLUMN direction TEXT NOT NULL DEFAULT 'outbound'
        CHECK (direction IN ('outbound', 'inbound_share'));

ALTER TABLE mcp_tunnel_auth ADD COLUMN jwks_url TEXT;
