/**
 * Agent-First Authentication System
 * 
 * API key-based authentication for AI agents.
 * Similar to agenthub's auth system but integrated with the Allternit Factory.
 * 
 * Features:
 * - API key generation per agent
 * - Key validation and verification
 * - Agent identity verification
 * - Key revocation
 * - Audit logging
 */

import { createHash, randomBytes } from 'crypto'
import { Log } from '@/shared/util/log'
import { Bus } from '@/shared/bus'
import { BusEvent } from '@/shared/bus/bus-event'
import { z } from 'zod/v4'

export namespace AgentAuth {
  const log = Log.create({ service: 'agent-auth' })

  // ============================================================================
  // Types
  // ============================================================================

  export interface AgentKey {
    id: string
    key: string // Hashed
    keyPrefix: string // First 8 chars for identification
    agentId: string
    agentName: string
    agentRole: string
    createdAt: number
    expiresAt?: number
    revoked: boolean
    revokedAt?: number
    lastUsedAt?: number
    usageCount: number
    permissions: string[]
    metadata?: Record<string, unknown>
  }

  export interface AuthResult {
    valid: boolean
    agentId?: string
    agentName?: string
    agentRole?: string
    keyId?: string
    error?: string
  }

  export interface KeyCreationRequest {
    agentId: string
    agentName: string
    agentRole: string
    expiresAt?: number
    permissions?: string[]
    metadata?: Record<string, unknown>
  }

  export interface KeyCreationResponse {
    key: AgentKey
    plainTextKey: string // Only shown once at creation
  }

  // ============================================================================
  // Events
  // ============================================================================

  export const KeyCreated = BusEvent.define(
    'agent.auth.key.created',
    z.object({
      keyId: z.string(),
      agentId: z.string(),
      agentName: z.string(),
    }),
  )

  export const KeyValidated = BusEvent.define(
    'agent.auth.key.validated',
    z.object({
      keyId: z.string(),
      agentId: z.string(),
      valid: z.boolean(),
    }),
  )

  export const KeyRevoked = BusEvent.define(
    'agent.auth.key.revoked',
    z.object({
      keyId: z.string(),
      agentId: z.string(),
      reason: z.string().optional(),
    }),
  )

  // ============================================================================
  // State
  // ============================================================================

  const keysById = new Map<string, AgentKey>()
  const keysByPrefix = new Map<string, AgentKey>()
  // Note: We don't store plain text keys, only hashed versions

  // ============================================================================
  // Key Generation
  // ============================================================================

  /**
   * Generate a new API key for an agent
   */
  export function generateKey(request: KeyCreationRequest): KeyCreationResponse {
    // Generate random key (40 chars = 320 bits)
    const plainTextKey = `ak_${randomBytes(32).toString('hex')}`
    const keyPrefix = plainTextKey.slice(0, 8) // First 8 chars for identification

    // Hash the key for storage
    const hashedKey = hashKey(plainTextKey)

    const now = Date.now()
    const key: AgentKey = {
      id: `key_${randomBytes(16).toString('hex')}`,
      key: hashedKey,
      keyPrefix,
      agentId: request.agentId,
      agentName: request.agentName,
      agentRole: request.agentRole,
      createdAt: now,
      expiresAt: request.expiresAt,
      revoked: false,
      usageCount: 0,
      permissions: request.permissions || ['communicate', 'read', 'write'],
      metadata: request.metadata,
    }

    // Store key
    keysById.set(key.id, key)
    keysByPrefix.set(key.keyPrefix, key)

    log.info('Generated API key for agent', {
      keyId: key.id,
      agentId: request.agentId,
      agentName: request.agentName,
      expiresAt: key.expiresAt ? new Date(key.expiresAt).toISOString() : 'never',
    })

    // Publish event
    Bus.publish(KeyCreated, {
      keyId: key.id,
      agentId: request.agentId,
      agentName: request.agentName,
    })

    return {
      key,
      plainTextKey,
    }
  }

  /**
   * Hash an API key for storage
   */
  function hashKey(plainTextKey: string): string {
    return createHash('sha256')
      .update(plainTextKey)
      .digest('hex')
  }

  // ============================================================================
  // Key Validation
  // ============================================================================

  /**
   * Validate an API key
   */
  export function validateKey(plainTextKey: string): AuthResult {
    if (!plainTextKey || !plainTextKey.startsWith('ak_')) {
      return {
        valid: false,
        error: 'Invalid key format',
      }
    }

    // Hash the provided key
    const hashedKey = hashKey(plainTextKey)

    // Find key by prefix first (optimization)
    const keyPrefix = plainTextKey.slice(0, 8)
    const key = keysByPrefix.get(keyPrefix)

    if (!key) {
      log.warn('Key not found', { keyPrefix })
      return {
        valid: false,
        error: 'Key not found',
      }
    }

    // Verify the hash matches
    if (key.key !== hashedKey) {
      log.warn('Key hash mismatch', { keyId: key.id })
      return {
        valid: false,
        error: 'Invalid key',
      }
    }

    // Check if revoked
    if (key.revoked) {
      log.warn('Key revoked', { keyId: key.id, agentId: key.agentId })
      return {
        valid: false,
        error: 'Key has been revoked',
      }
    }

    // Check if expired
    if (key.expiresAt && Date.now() > key.expiresAt) {
      log.warn('Key expired', { keyId: key.id, agentId: key.agentId })
      return {
        valid: false,
        error: 'Key has expired',
      }
    }

    // Update usage stats
    key.lastUsedAt = Date.now()
    key.usageCount++

    log.debug('Key validated', {
      keyId: key.id,
      agentId: key.agentId,
      agentName: key.agentName,
      usageCount: key.usageCount,
    })

    // Publish event
    Bus.publish(KeyValidated, {
      keyId: key.id,
      agentId: key.agentId,
      valid: true,
    })

    return {
      valid: true,
      agentId: key.agentId,
      agentName: key.agentName,
      agentRole: key.agentRole,
      keyId: key.id,
    }
  }

  /**
   * Check if a key has a specific permission
   */
  export function hasPermission(keyId: string, permission: string): boolean {
    const key = keysById.get(keyId)
    if (!key) return false
    return key.permissions.includes(permission)
  }

  // ============================================================================
  // Key Management
  // ============================================================================

  /**
   * Revoke an API key
   */
  export function revokeKey(keyId: string, reason?: string): boolean {
    const key = keysById.get(keyId)
    if (!key) {
      log.warn('Key not found for revocation', { keyId })
      return false
    }

    key.revoked = true
    key.revokedAt = Date.now()

    log.info('Key revoked', {
      keyId,
      agentId: key.agentId,
      reason: reason || 'unspecified',
    })

    // Publish event
    Bus.publish(KeyRevoked, {
      keyId,
      agentId: key.agentId,
      reason,
    })

    return true
  }

  /**
   * Revoke all keys for an agent
   */
  export function revokeAllKeys(agentId: string, reason?: string): number {
    let count = 0
    for (const key of keysById.values()) {
      if (key.agentId === agentId && !key.revoked) {
        revokeKey(key.id, reason)
        count++
      }
    }
    log.info('Revoked all keys for agent', { agentId, count })
    return count
  }

  /**
   * Get key info (without exposing the key itself)
   */
  export function getKeyInfo(keyId: string): Partial<AgentKey> | undefined {
    const key = keysById.get(keyId)
    if (!key) return undefined

    // Return safe subset (no hashed key)
    return {
      id: key.id,
      keyPrefix: key.keyPrefix,
      agentId: key.agentId,
      agentName: key.agentName,
      agentRole: key.agentRole,
      createdAt: key.createdAt,
      expiresAt: key.expiresAt,
      revoked: key.revoked,
      revokedAt: key.revokedAt,
      lastUsedAt: key.lastUsedAt,
      usageCount: key.usageCount,
      permissions: key.permissions,
      metadata: key.metadata,
    }
  }

  /**
   * List all keys for an agent
   */
  export function listAgentKeys(agentId: string): Partial<AgentKey>[] {
    return Array.from(keysById.values())
      .filter((key) => key.agentId === agentId)
      .map((key) => getKeyInfo(key.id) as Partial<AgentKey>)
  }

  /**
   * Get all active keys count
   */
  export function getActiveKeyCount(): number {
    let count = 0
    const now = Date.now()
    for (const key of keysById.values()) {
      if (!key.revoked && (!key.expiresAt || key.expiresAt > now)) {
        count++
      }
    }
    return count
  }

  /**
   * Cleanup expired keys
   */
  export function cleanupExpiredKeys(): number {
    const now = Date.now()
    let removed = 0

    for (const [keyId, key] of keysById.entries()) {
      if (key.expiresAt && key.expiresAt < now) {
        keysById.delete(keyId)
        keysByPrefix.delete(key.keyPrefix)
        removed++
      }
    }

    if (removed > 0) {
      log.info('Cleaned up expired keys', { count: removed })
    }

    return removed
  }

  /**
   * Clear all keys (for testing)
   */
  export function clearAllKeys(): void {
    keysById.clear()
    keysByPrefix.clear()
    log.info('Cleared all keys')
  }

  // ============================================================================
  // Middleware Helper
  // ============================================================================

  /**
   * Create auth middleware for HTTP/gRPC
   */
  export function createAuthMiddleware() {
    return async function authMiddleware(
      apiKey: string | undefined,
    ): Promise<AuthResult> {
      if (!apiKey) {
        return {
          valid: false,
          error: 'Missing API key',
        }
      }

      return validateKey(apiKey)
    }
  }
}
