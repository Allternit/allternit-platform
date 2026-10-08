#!/usr/bin/env node
/**
 * `computers-mcp` bin entry — stdio MCP server for the Allternit Computers API.
 *
 * Env:
 *   ALLTERNIT_API_URL  base URL of allternit-api (default http://127.0.0.1:8013)
 *   ALLTERNIT_TOKEN    Clerk bearer token (Authorization header), or a
 *                      Platform API project key (alt_live_… / alt_test_…)
 *   ALLTERNIT_API_KEY  Platform API project key (takes precedence)
 *   ALLTERNIT_PLATFORM_URL  Platform API base (default https://api.allternit.com)
 */
import { runComputersMcpServer } from './server.js';

runComputersMcpServer().catch((error) => {
  console.error('computers-mcp failed to start:', error);
  process.exit(1);
});
