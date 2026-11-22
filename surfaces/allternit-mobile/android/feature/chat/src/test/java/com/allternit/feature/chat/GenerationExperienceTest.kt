package com.allternit.feature.chat

import java.time.ZoneId
import java.util.Locale
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Generation experience on Android (step 11): progress detail, store, typed errors, block pulse numbers, answer room. Mirrors iOS GenerationExperienceTests. */
class GenerationExperienceTest {
    private fun meta(json: String) = Json.parseToJsonElement(json).jsonObject

    private fun call(id: String, state: ToolState, metadata: String? = null, name: String = "media_generate") =
        ToolCallRecord(id, name, state, metadata = metadata?.let(::meta))

    private val motion = """{"progress":"x","progress_detail":{"lane":"motion","stage":"rendering","label":"","frame":3,"frames":12,"preview":"data:image/jpeg;base64,AAAA"}}"""

    // MARK: progress detail

    @Test fun readsTheProgressDetail() {
        val d = GenerationProgressDetail.read(meta(motion))!!
        assertEquals(GenerationLane.Motion, d.lane)
        assertEquals(GenerationStage.Rendering, d.stage)
        assertEquals(3, d.frame); assertEquals(12, d.frames)
    }

    @Test fun badLaneOrStageIsDroppedNotGuessed() {
        assertNull(GenerationProgressDetail.read(meta("""{"progress_detail":{"lane":"audio","stage":"drawing"}}""")))
        assertNull(GenerationProgressDetail.read(meta("""{"progress_detail":{"lane":"picture","stage":"warming"}}""")))
        assertNull(GenerationProgressDetail.read(meta("""{"progress":"just words"}""")))
        assertNull(GenerationProgressDetail.read(null))
    }

    @Test fun mainLineCountsFramesOnlyForMotionRendering() {
        val d = GenerationProgressDetail.read(meta(motion))
        assertEquals("Frame 3 of 12", GenerationProgressDetail.mainLine(d, GenerationLane.Motion))
        assertEquals("Rendering frames", GenerationProgressDetail.mainLine(d?.copy(frame = null), GenerationLane.Motion))
        assertEquals("Rendering video", GenerationProgressDetail.mainLine(null, GenerationLane.Motion))
        assertEquals("Generating image", GenerationProgressDetail.mainLine(null, GenerationLane.Picture))
        assertEquals("Frame 12 of 12", GenerationProgressDetail.mainLine(d?.copy(frame = 99), GenerationLane.Motion))
    }

    @Test fun barFractionPrefersTheReportedFraction() {
        val d = GenerationProgressDetail.read(meta(motion))
        assertEquals(0.25, GenerationProgressDetail.barFraction(d)!!, 1e-9)
        assertEquals(0.6, GenerationProgressDetail.barFraction(d?.copy(fraction = 0.6))!!, 1e-9)
        assertEquals(1.0, GenerationProgressDetail.barFraction(d?.copy(fraction = 4.0))!!, 1e-9)
        assertNull(GenerationProgressDetail.barFraction(d?.copy(stage = GenerationStage.Encoding)))
        assertNull(GenerationProgressDetail.barFraction(null))
    }

    @Test fun chipsAndRowsFollowTheStage() {
        assertEquals(-1, GenerationProgressDetail.motionStageIndex(GenerationStage.Drawing))
        assertEquals(0, GenerationProgressDetail.motionStageIndex(GenerationStage.Rendering))
        assertEquals(1, GenerationProgressDetail.motionStageIndex(GenerationStage.Encoding))
        assertEquals(2, GenerationProgressDetail.motionStageIndex(GenerationStage.Saving))
        assertEquals(0, GenerationProgressDetail.pictureStepIndex(null))
        assertEquals(1, GenerationProgressDetail.pictureStepIndex(GenerationStage.Drawing))
        assertEquals(2, GenerationProgressDetail.pictureStepIndex(GenerationStage.Rendering))
        assertEquals(3, GenerationProgressDetail.pictureStepIndex(GenerationStage.Saving))
    }

    @Test fun previewBlursUpAndShowsOnlyOnceThereIsSomethingReal() {
        val d = GenerationProgressDetail(GenerationLane.Picture, GenerationStage.Rendering, "", fraction = 0.5, preview = "data:image/png;base64,AA")
        assertEquals(12, d.previewBlur)
        assertTrue(d.hasPreview)
        assertEquals(0, d.copy(stage = GenerationStage.Saving).previewBlur)
        assertFalse(d.copy(stage = GenerationStage.Drawing).hasPreview)
        assertFalse(d.copy(preview = null).hasPreview)
    }

    @Test fun recognisesMediaGenerateInAnySpelling() {
        assertTrue(GenerationProgressDetail.isMediaGenerate("media_generate"))
        assertTrue(GenerationProgressDetail.isMediaGenerate("mediaGenerate"))
        assertFalse(GenerationProgressDetail.isMediaGenerate("run_code"))
    }

    @Test fun laneAndAspectComeFromTheCallInput() {
        val video = ToolCallRecord("c", "media_generate", ToolState.Running, input = meta("""{"kind":"video","width":1920,"height":1080}"""))
        assertEquals(GenerationLane.Motion, GenerationProgressDetail.lane(video, null))
        assertEquals(16f / 9f, GenerationProgressDetail.aspect(video)!!, 1e-4f)
        assertEquals(GenerationLane.Picture, GenerationProgressDetail.lane(call("c", ToolState.Running), null))
        val tall = ToolCallRecord("c", "media_generate", ToolState.Running, input = meta("""{"width":100,"height":2000}"""))
        assertEquals(1f / 3f, GenerationProgressDetail.aspect(tall)!!, 1e-4f)
        assertNull(GenerationProgressDetail.aspect(call("c", ToolState.Running)))
    }

    // MARK: store

    @Test fun storeKeepsTheLastFourDistinctMotionThumbnails() {
        val store = GenerationProgressStore()
        val base = GenerationProgressDetail(GenerationLane.Motion, GenerationStage.Rendering, "")
        for (i in 1..6) store.set("c", base.copy(frame = i, frames = 6, preview = "data:image/jpeg;base64,P$i"))
        store.set("c", base.copy(frame = 6, frames = 6, preview = "data:image/jpeg;base64,P6"))
        assertEquals(listOf("P3", "P4", "P5", "P6").map { "data:image/jpeg;base64,$it" }, store.entry("c")!!.thumbs)
    }

    @Test fun storeKeepsTheStartTimeAcrossReports() {
        val store = GenerationProgressStore()
        val d = GenerationProgressDetail(GenerationLane.Picture, GenerationStage.Drawing, "")
        store.set("c", d, nowMs = 1_000)
        store.set("c", d.copy(stage = GenerationStage.Rendering), nowMs = 9_000)
        assertEquals(1_000L, store.entry("c")!!.startedAtMs)
    }

    @Test fun storeDropsTheOldestBeyondItsCap() {
        val store = GenerationProgressStore()
        val d = GenerationProgressDetail(GenerationLane.Picture, GenerationStage.Drawing, "")
        for (i in 0 until GenerationProgressStore.MAX_ENTRIES + 3) store.set("c$i", d)
        assertEquals(GenerationProgressStore.MAX_ENTRIES, store.order.size)
        assertNull(store.entry("c0")); assertNotNull(store.entry("c${GenerationProgressStore.MAX_ENTRIES + 2}"))
    }

    @Test fun syncRecordsRunningCallsAndClearsEveryOtherState() {
        val store = GenerationProgressStore()
        store.sync(listOf(call("a", ToolState.Running, motion)), replyIsLive = true)
        assertNotNull(store.entry("a"))
        store.sync(listOf(call("a", ToolState.Done, motion)), replyIsLive = true)
        assertNull(store.entry("a"))
        store.sync(listOf(call("a", ToolState.Running, motion)), replyIsLive = true)
        store.sync(listOf(call("a", ToolState.Running, motion)), replyIsLive = false)
        assertNull("a reply that ended clears its calls", store.entry("a"))
        store.sync(listOf(call("b", ToolState.Failed, motion)), replyIsLive = true)
        assertNull(store.entry("b"))
        store.sync(listOf(call("r", ToolState.Running, motion, name = "run_code")), replyIsLive = true)
        assertNull("other tools never enter the store", store.entry("r"))
        assertTrue(store.order.isEmpty())
    }

    // MARK: typed errors

    private val zone = ZoneId.of("UTC")
    private val now = java.time.Instant.parse("2026-11-22T12:00:00Z").toEpochMilli()

    @Test fun usageLimitByCodeStatusOrWords() {
        val reset = java.time.Instant.parse("2026-11-22T15:40:00Z").toEpochMilli()
        val byCode = ChatErrorInfo.make("ChatGPT will not take more.", code = "usage_limit_reached", resetAtMs = reset)
        assertEquals(ChatErrorInfo.Kind.UsageLimit, byCode.kind)
        assertEquals(ChatErrorInfo.Action.PickModel, byCode.action)
        assertEquals("Pick another model", byCode.actionLabel)
        assertEquals("ChatGPT usage limit — resets at 3:40 PM", byCode.title(ChatErrorInfo.Lane.Chat, now, zone, Locale.US).replace(' ', ' '))
        assertEquals(ChatErrorInfo.Kind.UsageLimit, ChatErrorInfo.make("x", status = 429).kind)
        assertEquals(ChatErrorInfo.Kind.UsageLimit, ChatErrorInfo.make("You hit the usage limit for today").kind)
        assertEquals("Usage limit reached", ChatErrorInfo.make(null, code = "plan_rate").title(ChatErrorInfo.Lane.Chat, now, zone))
    }

    @Test fun resetTimeNamesTheDayWhenItIsNotToday() {
        val tue = java.time.Instant.parse("2026-11-24T15:40:00Z").toEpochMilli()
        assertEquals("Tue 3:40 PM", ChatErrorInfo.formatReset(tue, now, zone, Locale.US)?.replace(' ', ' '))
        assertNull(ChatErrorInfo.formatReset(now - 1, now, zone, Locale.US))
        assertNull(ChatErrorInfo.formatReset(null, now, zone, Locale.US))
    }

    @Test fun signedOutOffersConnectAndTranslatesTheCliCommand() {
        val e = ChatErrorInfo.make("Run gizzi connect chatgpt to sign in", code = "subscription_sharing_invalid_user")
        assertEquals(ChatErrorInfo.Kind.SignedOut, e.kind)
        assertTrue(e.isChatGPT)
        assertEquals("Connect ChatGPT", e.actionLabel)
        assertEquals("ChatGPT is not signed in on your account. Tap Connect to sign it in and keep going.", e.reason)
        assertEquals("ChatGPT is not signed in on your account. Tap Connect to sign it in and make pictures with it.", ChatErrorInfo.make("gizzi connect chatgpt", code = "signed_out", lane = ChatErrorInfo.Lane.Picture).reason)
        val other = ChatErrorInfo.make(null, status = 401)
        assertEquals("Open Connections", other.actionLabel)
        assertEquals("Sign-in needed", other.title(ChatErrorInfo.Lane.Chat))
    }

    @Test fun networkAndServerErrorsRetryAndKeepTheServersWords() {
        val net = ChatErrorInfo.make("Failed to fetch")
        assertEquals(ChatErrorInfo.Kind.Network, net.kind)
        assertEquals("Couldn't reach Allternit. Check your connection and try again.", net.reason)
        assertEquals("Try again", net.actionLabel)
        val server = ChatErrorInfo.make("The model fell over")
        assertEquals("The model fell over", server.reason)
        assertEquals("Reply not finished", server.title(ChatErrorInfo.Lane.Chat))
        assertEquals("Picture not made", server.title(ChatErrorInfo.Lane.Picture))
        assertEquals("Clip not made", server.title(ChatErrorInfo.Lane.Motion))
        assertEquals("Something went wrong. Try again.", ChatErrorInfo.make("  ").reason)
    }

    // MARK: block pulse (numbers from drawBlockPulse)

    @Test fun pulseCellIsAboutADozenSquaresAcrossAndClamped() {
        assertEquals(12.0, BlockPulseMath.cell(60.0, 400.0), 0.0)
        assertEquals(30.0, BlockPulseMath.cell(2000.0, 2000.0), 0.0)
        assertEquals(20.0, BlockPulseMath.cell(200.0, 800.0), 0.0)
        assertEquals(3.0, BlockPulseMath.gap(10.0), 0.0)
        assertEquals(8.1, BlockPulseMath.gap(30.0), 1e-9)
    }

    @Test fun aSquareIsAliveOnlyInsideItsOwnWindow() {
        val spread = BlockPulseMath.spread(300.0)
        assertNull(BlockPulseMath.tile(-0.01, 0.0, spread))
        assertNull(BlockPulseMath.tile(BlockPulseMath.TILE_SECONDS + 0.01, 0.0, spread))
        val peak = BlockPulseMath.tile(BlockPulseMath.RISE * BlockPulseMath.TILE_SECONDS, 0.0, spread)!!
        assertEquals(1.0, peak.alpha, 1e-9); assertEquals(1.0, peak.scale, 1e-9)
        assertEquals(0.3, BlockPulseMath.tile(0.0, 0.0, spread)!!.scale, 1e-9)
        assertEquals(0.5, BlockPulseMath.spread(1.0), 1e-9)
        assertEquals(BlockPulseMath.DEFAULT_SPREAD, BlockPulseMath.spread(0.0), 0.0)
    }

    @Test fun theOriginWaveReachesTheFarthestCorner() {
        assertEquals(500.0, BlockPulseMath.reach(0.0, 0.0, 300.0, 400.0), 1e-9)
        assertEquals(250.0, BlockPulseMath.reach(150.0, 200.0, 300.0, 400.0), 1e-9)
    }

    // MARK: chat wait

    private fun thread() = listOf(
        MessageRecord("u0", "user", "old"), MessageRecord("a0", "assistant", "old answer"),
        MessageRecord("u1", "user", "draw a fox"), MessageRecord("a1", "assistant", ""),
    )

    @Test fun anchorsTheNewestSentMessageWhenATurnStarts() {
        assertEquals("u1", AnswerRoom.anchorId(false, true, thread()))
        assertNull(AnswerRoom.anchorId(true, true, thread()))
        assertNull(AnswerRoom.anchorId(true, false, thread()))
        assertNull(AnswerRoom.anchorId(false, true, listOf(MessageRecord("a", "assistant", ""))))
    }

    @Test fun roomShrinksByWhatTheAnswerAdds() {
        val empty = AnswerRoom.room(600f, 80f, 16f, 20f)
        val grown = AnswerRoom.room(600f, 280f, 16f, 20f)
        assertEquals(200f, empty - grown, 0f)
        assertEquals(0f, AnswerRoom.room(600f, 900f, 16f, 20f), 0f)
    }

    @Test fun anAnchoredTurnDoesNotFollowTheBottomWhileItRuns() {
        assertFalse(AnswerRoom.followsBottom(anchored = true, streaming = true))
        assertTrue(AnswerRoom.followsBottom(anchored = true, streaming = false))
        assertTrue(AnswerRoom.followsBottom(anchored = false, streaming = true))
    }

    @Test fun aTypedErrorDoesNotGetOverwrittenAndOlderServersStayPlain() {
        val plain = ChatError(null, "bad", "bad")
        assertNull(plain.typed)
        val typed = ChatError(null, "limit", "limit", ChatErrorInfo.make("limit", code = "usage_limit"))
        assertEquals(ChatErrorInfo.Kind.UsageLimit, typed.typed!!.kind)
    }
}
