package com.allternit.feature.chat

import com.allternit.core.api.ApiResult
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The eval calls against a fake transport: what the user gets for each answer the engine can give. */
class EvalBackendTest {
    private class Fake(var answer: ApiResult<JsonElement>) : EvalTransport {
        var method = ""; var segments = emptyList<String>(); var query = emptyMap<String, String>(); var body: String? = null
        override suspend fun call(method: String, segments: List<String>, query: Map<String, String>, body: String?): ApiResult<JsonElement> {
            this.method = method; this.segments = segments; this.query = query; this.body = body
            return answer
        }
    }
    private fun ok(s: String) = ApiResult.Ok(Json.parseToJsonElement(s))
    private fun http(status: Int, code: String?) = ApiResult.HttpError(status, code, null)

    @Test fun saveSendsTheLabelReasonAndNoteAndReadsTheSelfTest() = runBlocking {
        val t = Fake(ok("""{"case":{"id":"c1","title":"Fix it","label":"bad","needs_check":true},"self_test":{"passed":true,"agrees_with_label":false},"needs_check":true}"""))
        val r = EvalBackend(t).saveCase("s1", good = false, reason = "Broke a file", note = " keep the file ") as EvalOutcome.Ok
        assertEquals("POST", t.method)
        assertEquals(listOf("cases"), t.segments)
        val sent = Json.parseToJsonElement(t.body!!).jsonObject
        assertEquals("bad", sent["label"]!!.jsonPrimitive.content)
        assertEquals("Broke a file", sent["quick_check"]!!.jsonPrimitive.content)
        assertEquals("keep the file", sent["note"]!!.jsonPrimitive.content)
        assertTrue(r.value.needsCheck)
        assertEquals("c1", r.value.case.id)
    }

    @Test fun aGoodCaseCarriesNoReason() = runBlocking {
        val t = Fake(ok("""{"case":{"id":"c2"}}"""))
        EvalBackend(t).saveCase("s1", good = true, reason = "Other", note = null)
        val sent = Json.parseToJsonElement(t.body!!).jsonObject
        assertEquals("good", sent["label"]!!.jsonPrimitive.content)
        assertNull(sent["quick_check"])
        assertNull(sent["note"])
    }

    @Test fun errorsMapToTheNamedStates() = runBlocking {
        fun err(status: Int, code: String?) = (EvalBackend(Fake(http(status, code))).deleteCaseErr())
        assertEquals(EvalError.CaseLimit, EvalBackend.mapError(409, "case_limit", null))
        assertEquals(EvalError.EstimateRequired, EvalBackend.mapError(409, "estimate_required", null))
        assertEquals(EvalError.NotFound, EvalBackend.mapError(404, "session_not_found", null))
        assertEquals(EvalError.Unreachable, EvalBackend.mapError(502, "bad_gateway", null))
        assertEquals(EvalError.Unreachable, (EvalBackend(Fake(ApiResult.Network(java.io.IOException("x")))).getRun("r") as EvalOutcome.Err).error)
        assertEquals(EvalError.CaseLimit, err(409, "case_limit"))
    }

    private suspend fun EvalBackend.deleteCaseErr() = (deleteCase("c") as EvalOutcome.Err).error

    @Test fun deleteAcceptsAnEmptyAnswerAndListUsesTheQuery() = runBlocking {
        val t = Fake(ApiResult.Decode(204, SerializationException("empty")))
        assertTrue(EvalBackend(t).deleteCase("c1") is EvalOutcome.Ok)
        assertEquals("DELETE", t.method)
        assertEquals(listOf("cases", "c1"), t.segments)
        t.answer = ok("""{"cases":[{"id":"a","title":"A","last_result":"fail"}]}""")
        val list = (EvalBackend(t).listCases(suite = "mine") as EvalOutcome.Ok).value
        assertEquals(mapOf("suite" to "mine"), t.query)
        assertEquals("fail", list.single().lastResult)
    }

    @Test fun liveRunPollsToDoneAndReplayReportsCaseResults() = runBlocking {
        val t = Fake(ok("""{"run_id":"r1","report":{"suites":{"mine":{"pass_rate":0.5,"extra":{"per_case":[{"id":"a","pass":true},{"id":"b","pass":false}]}}}}}"""))
        val b = EvalBackend(t)
        val replay = (b.startRun("mine", live = false) as EvalOutcome.Ok).value
        assertEquals("replay", Json.parseToJsonElement(t.body!!).jsonObject["mode"]!!.jsonPrimitive.content)
        assertEquals(listOf(true, false), EvalReportView.results(replay.report).map { it.pass })
        assertEquals(0.5, EvalReportView.passRate(replay.report)!!, 0.0)
        t.answer = ok("""{"run_id":"r1","state":"running","progress":{"done":1,"total":4}}""")
        val live = (b.getRun("r1") as EvalOutcome.Ok).value
        assertEquals(false, live.finished)
        assertEquals(4, live.progress.total)
        t.answer = http(409, "estimate_required")
        assertEquals(EvalError.EstimateRequired, (b.startRun("mine", live = true) as EvalOutcome.Err).error)
    }
}
