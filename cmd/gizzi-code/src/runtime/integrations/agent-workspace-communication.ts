/**
 * Agent Workspace Communication State
 * 
 * Manages communication state in .allternit/communication/
 * Syncs with kernel and Factory receipt system.
 * 
 * Structure:
 * .allternit/communication/
 * ├── messages.jsonl          # Message audit trail
 * ├── channels.json           # Channel definitions
 * ├── mentions.jsonl          # @mention routing log
 * └── loop-guard.jsonl        # Loop guard events
 */

import { promises as fs } from 'fs'
import path from 'path'
import { Log } from '@/shared/util/log'
import { Global } from '@/runtime/context/global'

// Define AgentMessage type locally to match expected structure
interface AgentMessage {
  id: string
  timestamp: number
  from: { agentId: string; agentName: string; agentRole: string }
  to: { agentName?: string; agentRole?: string; channel?: string }
  content: string
  type: 'direct' | 'channel' | 'broadcast'
  correlationId?: string
  mentions?: string[]
}

export namespace AgentWorkspaceCommunication {
  const log = Log.create({ service: 'agent-workspace-comm' })

  // ============================================================================
  // Types
  // ============================================================================

  export interface MessageRecord {
    id: string
    timestamp: number
    from: { agentId: string; agentName: string; agentRole: string }
    to: { agentName?: string; agentRole?: string; channel?: string }
    content: string
    type: 'direct' | 'channel' | 'broadcast'
    correlationId?: string
    mentions?: string[]
    receiptId?: string
  }

  export interface ChannelRecord {
    id: string
    name: string
    description?: string
    createdAt: number
    createdBy: string
    members: string[]
  }

  export interface MentionRecord {
    id: string
    timestamp: number
    messageId: string
    fromAgent: string
    mentionedAgent: string
    routed: boolean
    triggered: boolean
  }

  export interface LoopGuardRecord {
    id: string
    timestamp: number
    correlationId: string
    hopCount: number
    agentId: string
    escalated: boolean
  }

  // ============================================================================
  // State
  // ============================================================================

  let initialized = false
  let workspacePath: string = ''
  let communicationPath: string = ''

  // ============================================================================
  // Initialization
  // ============================================================================

  /**
   * Initialize workspace communication state
   */
  export async function initialize(): Promise<void> {
    if (initialized) {
      log.debug('Already initialized')
      return
    }

    workspacePath = Global.Path.data
    communicationPath = path.join(workspacePath, '.allternit', 'communication')

    log.info('Initializing workspace communication', {
      workspacePath,
      communicationPath,
    })

    // Create directory structure
    await fs.mkdir(communicationPath, { recursive: true })

    // Initialize files if they don't exist
    await initializeFile('messages.jsonl')
    await initializeFile('channels.json')
    await initializeFile('mentions.jsonl')
    await initializeFile('loop-guard.jsonl')

    initialized = true
    log.info('Workspace communication initialized')
  }

  /**
   * Initialize a file if it doesn't exist
   */
  async function initializeFile(filename: string): Promise<void> {
    const filePath = path.join(communicationPath, filename)
    try {
      await fs.access(filePath)
    } catch {
      await fs.writeFile(filePath, '', 'utf-8')
      log.debug('Created file', { filename })
    }
  }

  // ============================================================================
  // Message Logging
  // ============================================================================

  /**
   * Log a message to workspace
   */
  export async function logMessage(message: AgentMessage, receiptId?: string): Promise<void> {
    if (!initialized) await initialize()

    const record: MessageRecord = {
      id: message.id,
      timestamp: message.timestamp,
      from: message.from,
      to: message.to,
      content: message.content,
      type: message.type,
      correlationId: message.correlationId,
      mentions: message.mentions,
      receiptId,
    }

    await appendToFile('messages.jsonl', JSON.stringify(record) + '\n')
    log.debug('Logged message', { messageId: message.id })
  }

  /**
   * Read messages from workspace
   */
  export async function readMessages(options?: {
    limit?: number
    channel?: string
    fromAgent?: string
    unreadOnly?: boolean
  }): Promise<MessageRecord[]> {
    if (!initialized) await initialize()

    const filePath = path.join(communicationPath, 'messages.jsonl')
    const content = await fs.readFile(filePath, 'utf-8')
    const lines = content.trim().split('\n').filter(Boolean)

    let messages: MessageRecord[] = lines.map((line) => JSON.parse(line))

    // Apply filters
    if (options?.channel) {
      messages = messages.filter((m) => m.to.channel === options.channel)
    }

    if (options?.fromAgent) {
      messages = messages.filter((m) => m.from.agentId === options.fromAgent)
    }

    // Apply limit
    if (options?.limit) {
      messages = messages.slice(-options.limit)
    }

    return messages
  }

  // ============================================================================
  // Channel Management
  // ============================================================================

  /**
   * Create a channel
   */
  export async function createChannel(channel: {
    id: string
    name: string
    description?: string
    createdBy: string
    members?: string[]
  }): Promise<ChannelRecord> {
    if (!initialized) await initialize()

    const record: ChannelRecord = {
      id: channel.id,
      name: channel.name,
      description: channel.description,
      createdAt: Date.now(),
      createdBy: channel.createdBy,
      members: channel.members || [channel.createdBy],
    }

    // Read existing channels
    const channels = await readChannels()

    // Add new channel
    channels.push(record)

    // Write back
    const filePath = path.join(communicationPath, 'channels.json')
    await fs.writeFile(filePath, JSON.stringify(channels, null, 2), 'utf-8')

    log.debug('Created channel', { channelId: channel.id, name: channel.name })

    return record
  }

  /**
   * Read all channels
   */
  export async function readChannels(): Promise<ChannelRecord[]> {
    if (!initialized) await initialize()

    const filePath = path.join(communicationPath, 'channels.json')
    try {
      const content = await fs.readFile(filePath, 'utf-8')
      return JSON.parse(content)
    } catch {
      return []
    }
  }

  /**
   * Join a channel
   */
  export async function joinChannel(
    channelId: string,
    agentId: string,
  ): Promise<boolean> {
    if (!initialized) await initialize()

    const channels = await readChannels()
    const channel = channels.find((c) => c.id === channelId)

    if (!channel) return false

    if (!channel.members.includes(agentId)) {
      channel.members.push(agentId)
    }

    // Write back
    const filePath = path.join(communicationPath, 'channels.json')
    await fs.writeFile(filePath, JSON.stringify(channels, null, 2), 'utf-8')

    log.debug('Agent joined channel', { agentId, channelId })

    return true
  }

  // ============================================================================
  // Mention Logging
  // ============================================================================

  /**
   * Log a mention
   */
  export async function logMention(mention: {
    id: string
    messageId: string
    fromAgent: string
    mentionedAgent: string
    routed: boolean
    triggered: boolean
  }): Promise<void> {
    if (!initialized) await initialize()

    const record: MentionRecord = {
      id: mention.id,
      timestamp: Date.now(),
      messageId: mention.messageId,
      fromAgent: mention.fromAgent,
      mentionedAgent: mention.mentionedAgent,
      routed: mention.routed,
      triggered: mention.triggered,
    }

    await appendToFile('mentions.jsonl', JSON.stringify(record) + '\n')
    log.debug('Logged mention', { mentionId: mention.id })
  }

  // ============================================================================
  // Loop Guard Logging
  // ============================================================================

  /**
   * Log a loop guard event
   */
  export async function logLoopGuard(event: {
    id: string
    correlationId: string
    hopCount: number
    agentId: string
    escalated: boolean
  }): Promise<void> {
    if (!initialized) await initialize()

    const record: LoopGuardRecord = {
      id: event.id,
      timestamp: Date.now(),
      correlationId: event.correlationId,
      hopCount: event.hopCount,
      agentId: event.agentId,
      escalated: event.escalated,
    }

    await appendToFile('loop-guard.jsonl', JSON.stringify(record) + '\n')
    log.debug('Logged loop guard event', { eventId: event.id })
  }

  // ============================================================================
  // Helpers
  // ============================================================================

  /**
   * Append to a file
   */
  async function appendToFile(filename: string, content: string): Promise<void> {
    const filePath = path.join(communicationPath, filename)
    await fs.appendFile(filePath, content, 'utf-8')
  }

  /**
   * Clear all data (for testing)
   */
  export async function clear(): Promise<void> {
    if (!initialized) return

    const files = ['messages.jsonl', 'channels.json', 'mentions.jsonl', 'loop-guard.jsonl']

    for (const file of files) {
      const filePath = path.join(communicationPath, file)
      await fs.writeFile(filePath, '', 'utf-8')
    }

    log.debug('Cleared all communication data')
  }
}
