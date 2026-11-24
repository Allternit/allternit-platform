import Foundation
import XCTest
@testable import FreeweightsVerify

/// Real Freeweights manifest + signature (byte copies of
/// freeweights.allternit.com/manifests/mlx-community/Qwen3-1.7B-4bit.json{,.minisig}).
final class MinisignTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "Fixtures/\(name)", withExtension: nil))
        return try Data(contentsOf: url)
    }

    func testBlake2bKnownAnswer() {
        // RFC 7693 Appendix A: BLAKE2b-512("abc").
        let hex = Blake2b.hash(Data("abc".utf8)).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(hex, "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1"
            + "7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923")
        // Empty input, and input that crosses a block boundary.
        XCTAssertEqual(Blake2b.hash(Data()).prefix(4), [0x78, 0x6a, 0x02, 0xf7])
        var split = Blake2b()
        let long = Data((0..<300).map { UInt8($0 & 0xff) })
        split.update(long.prefix(129)); split.update(long.dropFirst(129))
        XCTAssertEqual(split.finalize(), Blake2b.hash(long))
    }

    func testRealManifestVerifiesAndDecodes() throws {
        let data = try fixture("mlx-community__Qwen3-1.7B-4bit.json")
        let sig = String(decoding: try fixture("mlx-community__Qwen3-1.7B-4bit.json.minisig"), as: UTF8.self)
        let m = try FreeweightsManifest.verified(data: data, signature: sig, expectedId: "mlx-community/Qwen3-1.7B-4bit")
        XCTAssertEqual(m.file("model.safetensors")?.size, 968_080_210)
        XCTAssertEqual(m.baseURLs.first?.host, "huggingface.co")
    }

    func testTamperedManifestIsRejected() throws {
        var data = try fixture("mlx-community__Qwen3-1.7B-4bit.json")
        let sig = String(decoding: try fixture("mlx-community__Qwen3-1.7B-4bit.json.minisig"), as: UTF8.self)
        data[data.startIndex + 40] ^= 0x01
        XCTAssertThrowsError(try FreeweightsManifest.verified(data: data, signature: sig, expectedId: "mlx-community/Qwen3-1.7B-4bit")) {
            XCTAssertEqual($0 as? Minisign.Failure, .invalidSignature)
        }
    }

    func testEditedTrustedCommentIsRejected() throws {
        let data = try fixture("mlx-community__Qwen3-1.7B-4bit.json")
        let sig = String(decoding: try fixture("mlx-community__Qwen3-1.7B-4bit.json.minisig"), as: UTF8.self)
            .replacingOccurrences(of: "Qwen3-1.7B-4bit 3b1b", with: "Qwen3-1.7B-8bit 3b1b")
        XCTAssertThrowsError(try Minisign.verifyFreeweights(message: data, signatureFile: sig)) {
            XCTAssertEqual($0 as? Minisign.Failure, .invalidTrustedComment)
        }
    }

    func testManifestForAnotherIdIsRejected() throws {
        let data = try fixture("mlx-community__Qwen3-1.7B-4bit.json")
        let sig = String(decoding: try fixture("mlx-community__Qwen3-1.7B-4bit.json.minisig"), as: UTF8.self)
        XCTAssertThrowsError(try FreeweightsManifest.verified(data: data, signature: sig, expectedId: "Qwen/Qwen3-1.7B"))
    }
}
