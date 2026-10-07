/**
 * Agent Communication Runtime - Fixed Version
 * 
 * Properly initializes with Bus system and Session lifecycle.
 * Deferred subscription registration to avoid initialization order issues.
 */

import type { BusEvent } from "@/shared/bus/bus-event"
import { Log } from "@/shared/util/log"

export namespace AgentCommunicationRuntime {
  // Through the log, never stdout: `--json` commands and the TUI share it.
  const log = Log.create({ service: "agent.communication" })
  // Deferred initialization state
  let initializationPromise: Promise<void> | null = null
  let initialized = false
  let pendingHandlers: Array<() => void> = []

  /**
   * Initialize the communication runtime
   * Call this AFTER Bus and Session are initialized
   */
  export async function initialize(): Promise<void> {
    if (initialized) return
    if (initializationPromise) return initializationPromise

    initializationPromise = (async () => {
      log.info('Initializing...')

      try {
        // Import Bus and Session lazily to avoid circular dependencies
        const { Bus } = await import('@/shared/bus')
        const { Session } = await import('@/runtime/session')
        const { MentionRouter } = await import('@/runtime/agents/mention-router')
        const { AgentCommunicate } = await import('@/runtime/tools/builtins/agent-communicate')
        const { Agent } = await import('@/runtime/loop/agent')

        // Setup session handlers
        await setupSessionHandlers(Bus, Session, MentionRouter, Agent)
        
        // Setup message handlers
        setupMessageHandlers(Bus, AgentCommunicate)
        
        // Setup mention handlers
        setupMentionHandlers(Bus, MentionRouter)

        initialized = true
        log.info('Initialized successfully')

        // Execute any pending handlers
        for (const handler of pendingHandlers) {
          handler()
        }
        pendingHandlers = []
      } catch (error: any) {
        initializationPromise = null // Allow retry
        log.error('Initialization failed', { error: error.message })
        log.info('Will retry on first use')
        throw error
      }
    })()

    return initializationPromise
  }

  /**
   * Setup session lifecycle handlers
   */
  async function setupSessionHandlers(
    Bus: any,
    Session: any,
    MentionRouter: any,
    Agent: any
  ): Promise<void> {
    const { SessionStatus } = await import('@/runtime/session/status')

    // Register agent when session starts (Created event)
    Bus.subscribe(Session.Event.Created, async (event: any) => {
      const sessionID = event.properties?.info?.id
      const agentId = event.properties?.agent || "gizzi"

      if (sessionID) {
        try {
          // Use default name to avoid context issues during bootstrap
          const agentName = agentId === "gizzi" ? "Gizzi" : agentId
          
          MentionRouter.registerAgentSession({
            agentId,
            agentName,
            agentRole: "agent",
            sessionId: sessionID,
            status: "idle",
            lastActiveAt: Date.now(),
          })

          log.info(`Agent registered: ${agentId} in session ${sessionID}`)
        } catch (error: any) {
          log.error('Failed to register agent', { error: error.message })
        }
      }
    })

    // Update agent status during session status changes
    Bus.subscribe(SessionStatus.Event.Status, async (event: any) => {
      const sessionID = event.properties?.sessionID
      const status = event.properties?.status?.type
      
      if (sessionID && status) {
        const agents = MentionRouter.getAllAgents()
        const agent = agents.find((a: { sessionId: string }) => a.sessionId === sessionID)
        if (agent) {
          MentionRouter.updateAgentStatus(agent.agentId, status === "busy" ? "busy" : "idle")
        }
      }
    })

    // Mark agent as idle when session goes idle
    Bus.subscribe(SessionStatus.Event.Idle, async (event: any) => {
      const sessionID = event.properties?.sessionID
      if (sessionID) {
        const agents = MentionRouter.getAllAgents()
        const agent = agents.find((a: { sessionId: string }) => a.sessionId === sessionID)
        if (agent) {
          MentionRouter.updateAgentStatus(agent.agentId, "idle")
        }
      }
    })

    // Unregister agent when session is deleted
    Bus.subscribe(Session.Event.Deleted, async (event: any) => {
      const sessionID = event.properties?.info?.id
      if (sessionID) {
        const agents = MentionRouter.getAllAgents()
        const agent = agents.find((a: { sessionId: string }) => a.sessionId === sessionID)
        if (agent) {
          MentionRouter.unregisterAgentSession(agent.agentId)
          log.info(`Agent unregistered: ${agent.agentId}`)
        }
      }
    })
  }

  /**
   * Setup message event handlers
   */
  function setupMessageHandlers(Bus: any, AgentCommunicate: any): void {
    // Handle outgoing messages
    Bus.subscribe(AgentCommunicate.MessageSent, async (event: any) => {
      const props = event.properties

      log.info(`Message sent: ${props.fromAgent} → ${props.toAgent || props.toRole || props.channel}`)

      // Broadcast to UI for display
      Bus.publish(AgentCommunicate.MessageBroadcastToUI, {
        sessionId: props.sessionId,
        message: {
          id: props.messageId,
          from: {
            agentId: props.fromAgent,
            agentName: props.fromAgent,
            agentRole: "agent",
          },
          to: {
            agentName: props.toAgent,
            agentRole: props.toRole,
            channel: props.channel,
          },
          content: props.content,
          type: props.type,
          timestamp: Date.now(),
          correlationId: props.correlationId,
          mentions: props.mentions,
          read: false,
        },
      })
    })

    // Handle loop guard triggers
    Bus.subscribe(AgentCommunicate.LoopGuardTriggered, async (event: any) => {
      const props = event.properties

      log.warn(`Loop guard triggered! Hops: ${props.hopCount}`)

      // Create escalation message in session
      await createEscalationMessage(props)
    })
  }

  /**
   * Setup mention event handlers
   */
  function setupMentionHandlers(Bus: any, MentionRouter: any): void {
    // Handle mention detection
    Bus.subscribe(MentionRouter.MentionDetected, async (event: any) => {
      const props = event.properties
      log.info(`Mention detected: @${props.mention}`)
    })

    // Handle mention routing
    Bus.subscribe(MentionRouter.MentionRouted, async (event: any) => {
      const props = event.properties

      if (props.triggered) {
        log.info(`Mention routed and agent triggered: @${props.mention} → ${props.targetAgent}`)
      } else {
        log.info(`Mention routed but agent not triggered: @${props.mention}`)
      }
    })
  }

  /**
   * Create escalation message when loop guard is triggered
   */
  async function createEscalationMessage(props: any): Promise<void> {
    const content = `⚠️ **Loop Guard Triggered**

Agent communication chain has exceeded the maximum hop count (${props.hopCount}/4).

**Details:**
- Correlation ID: ${props.correlationId}
- Agent: ${props.agentId}
- Session: ${props.sessionId}

**Action Required:**
This conversation chain requires human intervention. Please review the communication thread and provide guidance.`

    // Note: In full integration, this would create a system message in the session
    log.info('Escalation message created')
  }

  /**
   * Get initialization status
   */
  export function isInitialized(): boolean {
    return initialized
  }

  /**
   * Get agent status (safe to call before initialization)
   */
  export async function getAgentStatus(agentId: string): Promise<"idle" | "busy" | "offline" | undefined> {
    if (!initialized) {
      return undefined
    }

    const { MentionRouter } = await import('@/runtime/agents/mention-router')
    const session = MentionRouter.getAgentSession(agentId)
    return session?.status
  }

  /**
   * Get all agents status (safe to call before initialization)
   */
  export async function getAllAgentsStatus(): Promise<Array<{
    agentId: string
    agentName: string
    agentRole: string
    sessionId: string
    status: "idle" | "busy" | "offline"
  }>> {
    if (!initialized) {
      return []
    }

    const { MentionRouter } = await import('@/runtime/agents/mention-router')
    return MentionRouter.getAllAgents()
  }
}

