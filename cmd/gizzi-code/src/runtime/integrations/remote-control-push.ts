import { Bus } from "@/shared/bus"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Question } from "@/runtime/integrations/question/question"
import { Pairing } from "@/runtime/services/pairing/pairing"
import { Session } from "@/runtime/session"
import { Log } from "@/shared/util/log"
import { isSelfHosted } from "@/shared/selfhost/offline"

const log = Log.create({ service: "remote-control-push" })

type NotificationType = "permission" | "question" | "completed" | "error"

interface NotificationPayload {
  runtimeId: string
  title: string
  body: string
  tag: string
  type: NotificationType
  sessionId?: string
}

function pushWorkerUrl(): string | undefined {
  return process.env.ALLTERNIT_REMOTE_CONTROL_PUSH_URL
}

async function notifySubscribers(payload: NotificationPayload) {
  const url = pushWorkerUrl()
  if (!url) return

  const pairing = await Pairing.load()
  if (!pairing?.runtimeId) {
    log.debug("runtime not cloud-paired; skipping remote-control push notification")
    return
  }

  // Only notify for the cloud-paired runtime. Broadcasting to all local
  // runtimes would leak notifications across machines.
  if (payload.runtimeId !== pairing.runtimeId) {
    log.debug("notification runtime does not match paired runtime; skipping", {
      notificationRuntimeId: payload.runtimeId,
      pairedRuntimeId: pairing.runtimeId,
    })
    return
  }

  const notifySecret = process.env.ALLTERNIT_REMOTE_CONTROL_NOTIFY_SECRET
  const deviceToken = pairing.deviceToken
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  }
  if (notifySecret) {
    headers["Authorization"] = `Bearer ${notifySecret}`
  } else if (deviceToken) {
    // Fallback: authenticate with the paired device token when no service secret
    // is configured (e.g. local dev). The push worker validates this token with
    // the cloud runtime-pairing service.
    headers["Authorization"] = `Bearer ${deviceToken}`
  } else {
    log.warn("no notify secret or device token available; skipping push notification")
    return
  }

  try {
    const response = await fetch(`${url.replace(/\/$/, "")}/notify`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        runtimeId: payload.runtimeId,
        title: payload.title,
        body: payload.body,
        tag: payload.tag,
        type: payload.type,
        sessionId: payload.sessionId,
      }),
    })
    if (!response.ok) {
      log.warn("push worker returned error", { status: response.status })
    }
  } catch (error) {
    log.warn("failed to notify remote control subscribers", { error: error instanceof Error ? error.message : String(error) })
  }
}

export function initRemoteControlPush(): void {
  if (isSelfHosted()) {
    log.info("self-hosted mode; remote control push is off")
    return
  }
  if (!pushWorkerUrl()) {
    log.info("remote control push worker URL not configured; skipping subscriptions")
    return
  }

  Bus.subscribe(PermissionNext.Event.Asked, async (event) => {
    const pairing = await Pairing.load()
    if (!pairing?.runtimeId) return
    await notifySubscribers({
      runtimeId: pairing.runtimeId,
      title: "Permission required",
      body: `Machine ${pairing.name || pairing.runtimeId} is asking for permission: ${event.properties.permission}`,
      tag: `permission:${event.properties.id}`,
      type: "permission",
      sessionId: event.properties.sessionID,
    })
  })

  Bus.subscribe(Question.Event.Asked, async (event) => {
    const pairing = await Pairing.load()
    if (!pairing?.runtimeId) return
    const summary = event.properties.questions.map((q: { header?: string }) => q.header).filter(Boolean).join(", ")
    await notifySubscribers({
      runtimeId: pairing.runtimeId,
      title: "Question waiting",
      body: summary
        ? `Machine ${pairing.name || pairing.runtimeId} needs input: ${summary}`
        : `Machine ${pairing.name || pairing.runtimeId} needs input.`,
      tag: `question:${event.properties.id}`,
      type: "question",
      sessionId: event.properties.sessionID,
    })
  })

  Bus.subscribe(Session.Event.Error, async (event) => {
    const pairing = await Pairing.load()
    if (!pairing?.runtimeId) return
    const errorMessage = event.properties.error?.message ?? "An error occurred"
    await notifySubscribers({
      runtimeId: pairing.runtimeId,
      title: "Machine error",
      body: `Machine ${pairing.name || pairing.runtimeId} reported an error: ${errorMessage}`,
      tag: `error:${event.properties.sessionID ?? pairing.runtimeId}:${Date.now()}`,
      type: "error",
      sessionId: event.properties.sessionID,
    })
  })

  log.info("remote control push subscriptions initialized")
}
