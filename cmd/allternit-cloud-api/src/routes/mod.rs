//! Routes module

pub mod agency_forward;
pub mod office_engine;
pub mod agent_sessions;
pub mod api_keys;
pub mod artifacts_comments;
pub mod artifacts_suggestions;
pub mod apps;
pub mod artifacts_v2;
// Apps + MCP 2026 share-link registry (migration 161).
pub mod share_links;
pub mod share_link_usage;
// Admin kill / flag for share links (lane 8).
pub mod admin_share_links;
pub mod gateway_catalog;
pub mod runtimes;
// Freeweights entitlement claim (signed plan/perks for freeweights.allternit.com).
pub mod freeweights;
pub mod approvals;
pub mod auth;
pub mod billing_credits;
pub mod billing_auto_topup;
pub mod billing_checkout;
pub mod billing_invoices;
pub mod billing_payment_methods;
pub mod billing_plan_change;
pub mod admin_customers;
pub mod admin_funding;
pub mod admin_open_lane;
pub mod admin_voice_billing;
pub mod billing_store;
pub mod billing_subscriptions;
pub mod billing_transfer;
pub mod billing_webhooks;
pub mod beta;
pub mod canvases;
pub mod clerk_webhooks;
pub mod cloud_phones;
pub mod code_environments;
pub mod contabo_hosted_runtimes;
pub mod continuation;
pub mod costs;
pub mod data_plane;
pub mod data_plane_fallback;
pub mod deployments;
pub mod discord_app;
pub mod dispatch_handoff;
pub mod gizzi_instances;
pub mod health;
pub mod hosted_entitlements;
pub mod hosted_runtimes;
pub mod instances;
pub mod inference_keys;
pub mod voice_keys;
pub mod brain_runners;
pub mod connections;
pub mod plan_threads;
/// Account projects and bots made with no computer (migration 149).
pub mod account_projects;
pub mod jobs;
pub mod live_activity;
pub mod livekit_admin;
pub mod me_flags;
/// Allternit Meetings (docs/meetings/CONTRACT.md): user routes, settings,
/// pod-token routes, link parsing, the session state machine, SSE fan-out.
pub mod meetings;
pub mod calendar_oauth;
pub mod meeting_events;
pub mod meeting_links;
pub mod meeting_notify;
pub mod meeting_pod;
pub mod meeting_state;
pub mod org_meeting_settings;
/// Zoom connect, deauthorization webhook and pod OBF tokens (docs/meetings/ZOOM.md).
pub mod zoom;
/// Compute gate: GET /api/v1/account/compute-status (+ acknowledge) and the relay's 409.
pub mod account_compute;
/// Account preferences that follow the person: GET/PUT /api/v1/account/pad-templates.
pub mod account_prefs;
pub mod web_search;
pub mod me_usage;
pub mod setup_checks;
pub mod a2a_edge;
pub mod mcp_edge;
pub mod mcp_edge_apps;
pub mod mcp_edge_cards;
pub mod mcp_edge_skills;
pub mod mcp_edge_tasks;
/// Allternit as an ID-JAG issuer for managed connector auth (WP1).
pub mod managed_auth;
/// Trusted outside issuers and the edge JWT-bearer authorization server (WP4).
pub mod edge_auth;
/// Link your Claude organization: our issuer's JWKS and discovery (contract B1).
pub mod anthropic_client_issuer;
/// Link your Claude organization: start, callback, status, unlink (contract B2).
pub mod claude_link;
/// "Sign in with Allternit": OAuth authorization endpoint, metadata and consent API (migration 150).
pub mod oauth_server;
/// Signed-in person's OAuth grants (list, cap, revoke) and the admin review of apps.
pub mod oauth_grants;
pub mod mcp_events;
pub mod mcp_event_callbacks;
pub mod allternit_events;
pub mod runtime_events;
pub mod floor_shares;
/// Share a chat thread by link: owner routes + the public /api/v2/public/threads/:token.
pub mod thread_shares;
pub mod runtime_presence;
pub mod mesh;
pub mod channel_inbound;
pub mod connect_return;
pub mod phone;
pub mod photo_avatars;
pub mod bot_email;
pub mod platform_v1;
pub mod phone_sync;
pub mod runtime_files;
pub mod phone_invites;
pub mod inapp_calls;
pub mod mcp_oauth_approvals;
pub mod vendor_bot_keys;
pub mod web_push;
pub mod apns;
pub mod fcm;
pub mod device_read;
pub mod mail_triage;
pub mod notifications;
pub mod channel_onboarding;
pub mod paired_computers;
pub mod computers_console;
pub mod computer_alerts;
pub mod aci_push;
pub mod computer_metrics;
pub mod computers_host;
pub mod computer_snippets;
pub mod cloud_skills;
pub mod computer_machines;
pub mod computer_file_roots;
pub mod computers_cloud;
pub mod phone_live;
pub mod registered_phones;
pub mod app_phones;
pub mod mirror;
pub mod model_hosts;
pub mod model_router;
pub mod mirror_ws;
pub mod office;
pub mod providers;
pub mod provisioned_instances;
pub mod regions;
pub mod runs;
pub mod runtime_pairing;
pub mod computer_files;
pub mod computer_files_drops;
pub mod computer_files_replicas;
pub mod computer_files_snapshot;
pub mod runtime_relay;
pub mod runtime_viewer;
pub mod schedules;
pub mod slack_app;
pub mod tasks;
pub mod team;
#[cfg(test)]
pub mod test_support;
pub mod files;
pub mod files_gateway;
pub mod files_keys;
pub mod sign_requests;
pub mod voice_consent;
pub mod voice_recordings;
pub mod voice_tickets;
pub mod whatsapp_es;
pub mod voice_calls_cloud;
pub mod wizard;
pub mod open_lane;
