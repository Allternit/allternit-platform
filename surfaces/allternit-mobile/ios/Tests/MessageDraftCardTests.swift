import XCTest
@testable import Allternit

final class MessageDraftCardTests: XCTestCase {
    func testDraftResultParses() {
        let json = #"{"type":"message_draft","to":"+15551234567","toName":"Sam","text":"Running late"}"#
        XCTAssertEqual(
            MessageToolResult.parse(toolName: "message_draft", output: json),
            .draft(.init(to: "+15551234567", toName: "Sam", text: "Running late"))
        )
        // The type field alone is enough, whatever the tool is called.
        XCTAssertNotNil(MessageToolResult.parse(toolName: "other", output: json))
    }

    func testHeldResultParses() {
        let json = #"{"held": true, "inboxId": "approval:abc"}"#
        XCTAssertEqual(MessageToolResult.parse(toolName: "messages_send", output: json), .held(inboxId: "approval:abc"))
        XCTAssertNil(MessageToolResult.parse(toolName: "messages_send", output: #"{"held": false}"#))
        XCTAssertNil(MessageToolResult.parse(toolName: "messages_send", output: "not json"))
        XCTAssertNil(MessageToolResult.parse(toolName: "x", output: nil))
    }

    func testSmsFallbackURL() {
        let url = MessageComposer.smsURL(to: "+1 (555) 123-4567", text: "Hi & bye, 50% off?")
        XCTAssertEqual(url?.absoluteString, "sms:+15551234567&body=Hi%20%26%20bye%2C%2050%25%20off%3F")
    }
}
