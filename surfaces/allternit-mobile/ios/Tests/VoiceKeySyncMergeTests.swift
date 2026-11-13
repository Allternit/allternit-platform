import XCTest
@testable import Allternit

/// Account sync for voice keys: newer copy wins per vendor; Disconnect opts out.
final class VoiceKeySyncMergeTests: XCTestCase {
    private let older = Date(timeIntervalSince1970: 1_800_000_000)
    private let newer = Date(timeIntervalSince1970: 1_800_000_600)

    private func remote(_ provider: String, _ key: String, at date: Date, voice: String? = nil) -> VoiceKeyRemote {
        VoiceKeyRemote(provider: provider, apiKey: key, voiceId: voice, label: nil, updatedAt: date)
    }

    func testAccountCopyNewerIsPulled() {
        let row = remote("openai", "sk-new", at: newer)
        let plan = VoiceKeyMerge.plan(local: [.openai: VoiceKeyLocal(apiKey: "sk-old", updatedAt: older)], remote: [row], optedOut: [])
        XCTAssertEqual(plan, VoiceKeyMergePlan(pull: [row], push: []))
    }

    func testPhoneCopyNewerIsPushed() {
        let plan = VoiceKeyMerge.plan(local: [.openai: VoiceKeyLocal(apiKey: "sk-new", updatedAt: newer)],
                                      remote: [remote("openai", "sk-old", at: older)], optedOut: [])
        XCTAssertEqual(plan, VoiceKeyMergePlan(pull: [], push: [.openai]))
    }

    func testOnlyOnOneSideCopiesAcross() {
        let row = remote("elevenlabs", "el", at: older)
        let plan = VoiceKeyMerge.plan(local: [.openai: VoiceKeyLocal(apiKey: "sk", updatedAt: older)], remote: [row], optedOut: [])
        XCTAssertEqual(plan, VoiceKeyMergePlan(pull: [row], push: [.openai]))
    }

    func testDisconnectedVendorIsNotPulledBack() {
        let plan = VoiceKeyMerge.plan(local: [:], remote: [remote("openai", "sk", at: newer)], optedOut: [.openai])
        XCTAssertEqual(plan, VoiceKeyMergePlan())
    }

    func testSameKeyDoesNothing() {
        let plan = VoiceKeyMerge.plan(local: [.openai: VoiceKeyLocal(apiKey: "sk", updatedAt: older)],
                                      remote: [remote("openai", "sk", at: newer)], optedOut: [])
        XCTAssertEqual(plan, VoiceKeyMergePlan())
    }

    func testVoiceChangedOnPhoneIsPushed() {
        let plan = VoiceKeyMerge.plan(local: [.elevenlabs: VoiceKeyLocal(apiKey: "el", voiceId: "new-voice", updatedAt: newer)],
                                      remote: [remote("elevenlabs", "el", at: older, voice: "old-voice")], optedOut: [])
        XCTAssertEqual(plan.push, [.elevenlabs])
    }

    func testKeyFromBeforeSyncLosesToAccount() {
        let row = remote("openai", "sk-account", at: older)
        let plan = VoiceKeyMerge.plan(local: [.openai: VoiceKeyLocal(apiKey: "sk-legacy", updatedAt: nil)], remote: [row], optedOut: [])
        XCTAssertEqual(plan.pull, [row])
    }

    func testDecodesServerRowsWithMicroseconds() throws {
        let json = #"{"keys":[{"provider":"openai","api_key":"sk","voice_id":null,"label":"sk-…abcd","updated_at":"2026-11-13T09:12:33.123456Z"},{"provider":"elevenlabs","api_key":"el","voice_id":"v","label":null,"updated_at":"2026-11-13T09:12:33+00:00"}]}"#
        struct List: Decodable { let keys: [VoiceKeyRemote] }
        let rows = try JSONDecoder().decode(List.self, from: Data(json.utf8)).keys
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0].updatedAt.timeIntervalSince(rows[1].updatedAt), 0.123456, accuracy: 0.000_01)
        XCTAssertEqual(rows[1].vendor, .elevenlabs)
    }
}
