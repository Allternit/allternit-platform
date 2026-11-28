package com.allternit.feature.shell

import com.allternit.feature.push.PushNotifications
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Android twin of iOS NotificationActionsTests.testKernelApprovalLinkAndCategory (PR #3309). */
class KernelApprovalTest {
    @Test fun kernelLinkParsesToRowKey() {
        // allternit://inbox/kernel/rt1/req%3A9: the host and decoded path segments.
        assertEquals("rt1/req:9", KernelApprovalLink.parse(listOf("inbox", "kernel", "rt1", "req:9")))
        assertNull(KernelApprovalLink.parse(listOf("inbox", "approval:ap1")))
        assertNull(KernelApprovalLink.parse(listOf("inbox", "kernel", "rt1")))
    }

    @Test fun kernelNotificationIsOpenOnly() {
        // Open only: never Approve or Deny from the notification.
        assertEquals(listOf("OPEN"), PushNotifications.KERNEL_APPROVAL_ACTION_IDS)
        assertFalse(PushNotifications.KERNEL_APPROVAL_ACTION_IDS.contains(PushNotifications.ACTION_APPROVE))
        assertFalse(PushNotifications.KERNEL_APPROVAL_ACTION_IDS.contains(PushNotifications.ACTION_DENY))
        assertEquals(PushNotifications.CHANNEL_APPROVALS, PushNotifications.channelFor(PushNotifications.KERNEL_APPROVAL, null))
    }

    @Test fun deadlineAcceptsIsoAndEpoch() {
        fun d(json: String) = KernelApproval.deadlineMillis(Json.parseToJsonElement(json))
        assertEquals(1_700_000_000_000L, d("\"2023-11-14T22:13:20Z\""))
        assertEquals(1_700_000_000_000L, d("\"2023-11-14T22:13:20.000Z\""))
        assertEquals(1_700_000_000_000L, d("1700000000"))
        assertEquals(1_700_000_000_000L, d("1700000000000"))
    }

    @Test fun unreadableRowsAreSkippedAndUnknownMeansHumanOnly() {
        val body = Json.parseToJsonElement(
            """{"items":[{"runtimeId":"rt1","kernel":{"requestId":"r1","actionClass":"send_money"}},{"runtimeId":"rt1"},{"kernel":{}}]}""",
        )
        val rows = KernelApproval.list(body)
        assertEquals(1, rows.size)
        assertTrue(rows[0].humanOnly)
        assertEquals("rt1/r1", rows[0].id)
    }

    @Test fun failuresUsePlainMessages() {
        assertEquals(KernelResolveMessages.FORBIDDEN_CALLER, KernelResolveMessages.forFailure(400, "forbidden_caller"))
        assertEquals(KernelResolveMessages.TOKEN_EXPIRED, KernelResolveMessages.forFailure(409, null))
        assertEquals(KernelResolveMessages.KERNEL_UNAVAILABLE, KernelResolveMessages.forFailure(503, null))
        assertNull(KernelResolveMessages.forFailure(500, null))
    }
}
