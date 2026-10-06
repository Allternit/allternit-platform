import { isEnvTruthy } from '../shared/utils/envUtils.js'
import { CLOUD_URLS } from '../shared/constants/cloudUrls.js'

// Default to prod config, override with test/staging if enabled
type OauthConfigType = 'prod' | 'staging' | 'local'

function getOauthConfigType(): OauthConfigType {
  if (process.env.USER_TYPE === 'ant') {
    if (isEnvTruthy(process.env.USE_LOCAL_OAUTH)) {
      return 'local'
    }
    if (isEnvTruthy(process.env.USE_STAGING_OAUTH)) {
      return 'staging'
    }
  }
  return 'prod'
}

export function fileSuffixForOauthConfig(): string {
  if (process.env.GIZZI_CUSTOM_OAUTH_URL) {
    return '-custom-oauth'
  }
  switch (getOauthConfigType()) {
    case 'local':
      return '-local-oauth'
    case 'staging':
      return '-staging-oauth'
    case 'prod':
      // No suffix for production config
      return ''
  }
}

export const CLAUDE_AI_INFERENCE_SCOPE = 'user:inference' as const
export const CLAUDE_AI_PROFILE_SCOPE = 'user:profile' as const
const CONSOLE_SCOPE = 'org:create_api_key' as const
export const OAUTH_BETA_HEADER = 'oauth-2025-04-20' as const

// Console OAuth scopes - for API key creation via Console
export const CONSOLE_OAUTH_SCOPES = [
  CONSOLE_SCOPE,
  CLAUDE_AI_PROFILE_SCOPE,
] as const

// Claude.ai OAuth scopes - for Claude.ai subscribers (Pro/Max/Team/Enterprise)
export const CLAUDE_AI_OAUTH_SCOPES = [
  CLAUDE_AI_PROFILE_SCOPE,
  CLAUDE_AI_INFERENCE_SCOPE,
  'user:sessions:claude_code',
  'user:mcp_servers',
  'user:file_upload',
] as const

// All OAuth scopes - union of all scopes used in Claude CLI
// When logging in, request all scopes in order to handle both Console -> Claude.ai redirect
// Ensure that `OAuthConsentPage` in apps repo is kept in sync with this list.
export const ALL_OAUTH_SCOPES = Array.from(
  new Set([...CONSOLE_OAUTH_SCOPES, ...CLAUDE_AI_OAUTH_SCOPES]),
)

type OauthConfig = {
  BASE_API_URL: string
  CONSOLE_AUTHORIZE_URL: string
  CLAUDE_AI_AUTHORIZE_URL: string
  /**
   * The claude.ai web origin. Separate from CLAUDE_AI_AUTHORIZE_URL because
   * that now routes through claude.com/cai/* for attribution — deriving
   * .origin from it would give claude.com, breaking links to /code,
   * /settings/connectors, and other claude.ai web pages.
   */
  CLAUDE_AI_ORIGIN: string
  TOKEN_URL: string
  API_KEY_URL: string
  ROLES_URL: string
  CONSOLE_SUCCESS_URL: string
  CLAUDEAI_SUCCESS_URL: string
  MANUAL_REDIRECT_URL: string
  CLIENT_ID: string
  OAUTH_FILE_SUFFIX: string
  MCP_PROXY_URL: string
  MCP_PROXY_PATH: string
}

// Production OAuth configuration - Used in normal operation
const PROD_OAUTH_CONFIG = {
  BASE_API_URL: CLOUD_URLS.api,
  CONSOLE_AUTHORIZE_URL: `${CLOUD_URLS.platform}/sign-in`,
  CLAUDE_AI_AUTHORIZE_URL: `${CLOUD_URLS.platform}/sign-in`,
  CLAUDE_AI_ORIGIN: CLOUD_URLS.platform,
  TOKEN_URL: `${CLOUD_URLS.api}/v1/oauth/token`,
  API_KEY_URL: `${CLOUD_URLS.platform}/api-keys`,
  ROLES_URL: `${CLOUD_URLS.api}/api/oauth/gizzi/roles`,
  CONSOLE_SUCCESS_URL: `${CLOUD_URLS.platform}/billing`,
  CLAUDEAI_SUCCESS_URL: `${CLOUD_URLS.platform}/billing`,
  MANUAL_REDIRECT_URL: `${CLOUD_URLS.platform}/sign-in`,
  CLIENT_ID: 'allternit-gizzi-code',
  OAUTH_FILE_SUFFIX: '',
  MCP_PROXY_URL: `${CLOUD_URLS.api}/mcp-proxy`,
  MCP_PROXY_PATH: '/v1/mcp/{server_id}',
} as const

/**
 * Client ID Metadata Document URL for MCP OAuth (CIMD / SEP-991).
 * When an MCP auth server advertises client_id_metadata_document_supported: true,
 * Gizzi uses this URL as its client_id instead of Dynamic Client Registration.
 * MCP 2026-07-28 deprecates DCR in favor of CIMD, so this is preferred whenever the
 * auth server supports it. The URL is Allternit's hosted CIMD JSON document (served
 * by the platform.allternit.com console from public/oauth/gizzi-client.json); it must
 * be an https URL with a non-root path, and its redirect_uris must list every
 * loopback callback gizzi uses.
 * See: https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00
 */
export const MCP_CLIENT_METADATA_URL =
  'https://platform.allternit.com/oauth/gizzi-client.json'

// Staging OAuth configuration - only included in ant builds with staging flag
// Uses literal check for dead code elimination
const STAGING_OAUTH_CONFIG =
  process.env.USER_TYPE === 'ant'
    ? ({
        BASE_API_URL: 'https://api-staging.allternit.com',
        CONSOLE_AUTHORIZE_URL: 'https://platform.allternit.com/sign-in',
        CLAUDE_AI_AUTHORIZE_URL: 'https://platform.allternit.com/sign-in',
        CLAUDE_AI_ORIGIN: 'https://platform.allternit.com',
        TOKEN_URL: 'https://api-staging.allternit.com/v1/oauth/token',
        API_KEY_URL: 'https://platform.allternit.com/api-keys',
        ROLES_URL: 'https://api-staging.allternit.com/api/oauth/gizzi/roles',
        CONSOLE_SUCCESS_URL: 'https://platform.allternit.com/billing',
        CLAUDEAI_SUCCESS_URL: 'https://platform.allternit.com/billing',
        MANUAL_REDIRECT_URL: 'https://platform.allternit.com/sign-in',
        CLIENT_ID: 'allternit-gizzi-code',
        OAUTH_FILE_SUFFIX: '-staging-oauth',
        MCP_PROXY_URL: 'https://api-staging.allternit.com/mcp-proxy',
        MCP_PROXY_PATH: '/v1/mcp/{server_id}',
      } as const)
    : undefined

// Three local dev servers: :8000 api-proxy (`api dev start -g ccr`),
// :4000 claude-ai frontend, :3000 Console frontend. Env vars let
// scripts/claude-localhost override if your layout differs.
function getLocalOauthConfig(): OauthConfig {
  const api =
    process.env.GIZZI_LOCAL_OAUTH_API_BASE?.replace(/\/$/, '') ??
    'http://localhost:8000'
  const apps =
    process.env.GIZZI_LOCAL_OAUTH_APPS_BASE?.replace(/\/$/, '') ??
    'http://localhost:4000'
  const consoleBase =
    process.env.GIZZI_LOCAL_OAUTH_CONSOLE_BASE?.replace(/\/$/, '') ??
    'http://localhost:3000'
  return {
    BASE_API_URL: api,
    CONSOLE_AUTHORIZE_URL: `${consoleBase}/oauth/authorize`,
    CLAUDE_AI_AUTHORIZE_URL: `${apps}/oauth/authorize`,
    CLAUDE_AI_ORIGIN: apps,
    TOKEN_URL: `${api}/v1/oauth/token`,
    API_KEY_URL: `${api}/api/oauth/gizzi/create_api_key`,
    ROLES_URL: `${api}/api/oauth/gizzi/roles`,
    CONSOLE_SUCCESS_URL: `${consoleBase}/buy_credits?returnUrl=/oauth/code/success%3Fapp%3Dgizzi`,
    CLAUDEAI_SUCCESS_URL: `${consoleBase}/oauth/code/success?app=gizzi`,
    MANUAL_REDIRECT_URL: `${consoleBase}/oauth/code/callback`,
    CLIENT_ID: 'allternit-gizzi-code',
    OAUTH_FILE_SUFFIX: '-local-oauth',
    MCP_PROXY_URL: 'http://localhost:8205',
    MCP_PROXY_PATH: '/v1/toolbox/shttp/mcp/{server_id}',
  }
}

// Allowed base URLs for GIZZI_CUSTOM_OAUTH_URL override.
// Only FedStart/PubSec deployments are permitted to prevent OAuth tokens
// from being sent to arbitrary endpoints.
const ALLOWED_OAUTH_BASE_URLS = [
  'https://api.allternit.com',
  'https://ai.allternit.com',
  'https://platform.allternit.com',
]

// Default to prod config, override with test/staging if enabled
export function getOauthConfig(): OauthConfig {
  let config: OauthConfig = (() => {
    switch (getOauthConfigType()) {
      case 'local':
        return getLocalOauthConfig()
      case 'staging':
        return STAGING_OAUTH_CONFIG ?? PROD_OAUTH_CONFIG
      case 'prod':
        return PROD_OAUTH_CONFIG
    }
  })()

  // Allow overriding all OAuth URLs to point to an approved FedStart deployment.
  // Only allowlisted base URLs are accepted to prevent credential leakage.
  const oauthBaseUrl = process.env.GIZZI_CUSTOM_OAUTH_URL
  if (oauthBaseUrl) {
    const base = oauthBaseUrl.replace(/\/$/, '')
    if (!ALLOWED_OAUTH_BASE_URLS.includes(base)) {
      throw new Error(
        'GIZZI_CUSTOM_OAUTH_URL is not an approved endpoint.',
      )
    }
    config = {
      ...config,
      BASE_API_URL: base,
      CONSOLE_AUTHORIZE_URL: `${base}/oauth/authorize`,
      CLAUDE_AI_AUTHORIZE_URL: `${base}/oauth/authorize`,
      CLAUDE_AI_ORIGIN: base,
      TOKEN_URL: `${base}/v1/oauth/token`,
      API_KEY_URL: `${base}/api/oauth/claude_cli/create_api_key`,
      ROLES_URL: `${base}/api/oauth/claude_cli/roles`,
      CONSOLE_SUCCESS_URL: `${base}/oauth/code/success?app=gizzi`,
      CLAUDEAI_SUCCESS_URL: `${base}/oauth/code/success?app=gizzi`,
      MANUAL_REDIRECT_URL: `${base}/oauth/code/callback`,
      OAUTH_FILE_SUFFIX: '-custom-oauth',
    }
  }

  // Allow CLIENT_ID override via environment variable (e.g., for Xcode integration)
  const clientIdOverride = process.env.GIZZI_OAUTH_CLIENT_ID
  if (clientIdOverride) {
    config = {
      ...config,
      CLIENT_ID: clientIdOverride,
    }
  }

  return config
}
