import XCTest
@testable import Allternit

final class VoiceSessionProtocolTests: XCTestCase {

    func testParseSessionReady() {
        let json = #"{"type":"session.ready","sessionId":"s1","engine":"device","outputSampleRate":24000,"voices":["a","b"],"protocol":1}"#
        guard case .ready(let ready)? = VoiceServerEvent.parse(json) else { return XCTFail("not ready") }
        XCTAssertEqual(ready.sessionId, "s1")
        XCTAssertEqual(ready.engine, .device)
        XCTAssertEqual(ready.outputSampleRate, 24_000)
        XCTAssertEqual(ready.voices, ["a", "b"])
    }

    func testParseTurnAndSpeakEvents() {
        XCTAssertEqual(VoiceServerEvent.parse(#"{"type":"speech.started","atMs":120}"#), .speechStarted(atMs: 120))
        XCTAssertEqual(VoiceServerEvent.parse(#"{"type":"transcript.final","segmentId":"x","text":"hi"}"#),
                       .transcriptFinal(segmentId: "x", text: "hi"))
        XCTAssertEqual(VoiceServerEvent.parse(#"{"type":"speak.interrupted","id":"r1","sentMs":300}"#),
                       .speakInterrupted(id: "r1", sentMs: 300))
    }

    func testParseRejectsGarbageAndUnknown() {
        XCTAssertNil(VoiceServerEvent.parse("not json"))
        XCTAssertNil(VoiceServerEvent.parse(#"{"type":"nope"}"#))
        XCTAssertNil(VoiceServerEvent.parse(#"{"type":"session.ready"}"#))
    }

    func testClientMessagesEncode() {
        XCTAssertEqual(VoiceClientMessage.micMute.jsonString, #"{"type":"mic.mute"}"#)
        XCTAssertEqual(VoiceClientMessage.end.jsonString, #"{"type":"session.end"}"#)
        XCTAssertEqual(VoiceClientMessage.speakDelta(id: "r", text: "yo").jsonString,
                       #"{"id":"r","text":"yo","type":"speak.delta"}"#)
        XCTAssertTrue(VoiceClientMessage.start(VoiceSessionOptions()).jsonString.contains(#""type":"session.start""#))
    }

    func testPCMRoundTrip() {
        let samples: [Float] = [0, 0.5, -0.5, 1, -1]
        let decoded = VoicePCM.decode(VoicePCM.encode(samples))
        XCTAssertEqual(decoded.count, samples.count)
        for (a, b) in zip(samples, decoded) { XCTAssertEqual(a, b, accuracy: 0.001) }
    }

    func testJitterBufferPrebuffersThenFlushes() {
        var buf = PlaybackJitterBuffer(sampleRate: 1000, prebufferMs: 60)
        XCTAssertTrue(buf.push([Float](repeating: 0, count: 30)).isEmpty)
        XCTAssertEqual(buf.push([Float](repeating: 0, count: 40)).count, 2)
        XCTAssertEqual(buf.push([0, 0]).count, 1)
        buf.flush()
        XCTAssertFalse(buf.isPlaying)
        XCTAssertEqual(buf.queuedSamples, 0)
        XCTAssertTrue(buf.push([0]).isEmpty)
        XCTAssertEqual(buf.finish().count, 1)
    }
}
