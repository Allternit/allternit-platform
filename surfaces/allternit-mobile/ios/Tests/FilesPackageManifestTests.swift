import XCTest
@testable import Allternit

/// The model hand-off checks (SPEC section 19). A phone writes nothing for a
/// package that fails these, so they must match the Rust receiver
/// (`cmd/allternit-api/src/files_v2/models.rs`).
final class FilesPackageManifestTests: XCTestCase {
    private let hashA = String(repeating: "a", count: 64)
    private let hashB = String(repeating: "b", count: 64)
    private let sha = String(repeating: "c", count: 64)

    private func manifest(_ files: [FilesPackageManifest.File], total: UInt64? = nil, id: String? = nil) -> FilesPackageManifest {
        FilesPackageManifest(
            format: FilesPackageManifest.format,
            packageId: id ?? FilesPackageManifest.packageId(for: files),
            name: "qwen3-1.7b-q4",
            kind: "model",
            weightsFormat: "gguf",
            totalBytes: total ?? files.reduce(0) { $0 + $1.size },
            files: files,
            grant: .init(transfer: true, execute: true),
            source: .init(computerId: "rd_1", itemId: "it_1", rev: "1-x"),
            createdAt: "2026-11-15T20:00:00Z"
        )
    }

    private func file(_ path: String, _ hash: String? = nil, size: UInt64 = 10) -> FilesPackageManifest.File {
        .init(path: path, size: size, contentHash: hash ?? hashA, sha256: sha, itemId: "it_\(path.count)", rev: "1-a")
    }

    /// Rust sorts the lines by bytes. Swift's `String` order treats NFC and
    /// NFD spellings as equal, so a plain `.sorted()` would make the id
    /// depend on the order the computer listed the files in.
    func testPackageIdSortsLinesByBytes() {
        let nfc = file("caf\u{00E9}.gguf", hashA)
        let nfd = file("cafe\u{0301}.gguf", hashB)
        XCTAssertEqual(FilesPackageManifest.packageId(for: [nfc, nfd]), FilesPackageManifest.packageId(for: [nfd, nfc]))
        XCTAssertTrue(FilesPackageManifest.packageId(for: [nfc]).hasPrefix("pkg_"))
        XCTAssertEqual(FilesPackageManifest.packageId(for: [nfc]).count, 28)
    }

    func testNfcAndNfdPathsAreTwoFilesLikeRust() {
        XCTAssertNoThrow(try manifest([file("caf\u{00E9}.gguf"), file("cafe\u{0301}.gguf")]).validate())
    }

    func testValidPackagePasses() {
        XCTAssertNoThrow(try manifest([file("model.gguf"), file("tokenizer/config.json", hashB)]).validate())
    }

    func testUnsafePathsAreRefused() {
        for path in ["../etc/passwd", "/abs.gguf", "a//b", "a/./b", "a\\b", "dir/.allternit-package.json", ""] {
            XCTAssertThrowsError(try manifest([file(path)]).validate(), path)
        }
    }

    func testDuplicatePathIgnoringCaseIsRefused() {
        XCTAssertThrowsError(try manifest([file("Model.gguf"), file("model.gguf", hashB)]).validate())
    }

    func testWrongTotalIsRefused() {
        XCTAssertThrowsError(try manifest([file("model.gguf")], total: 11).validate())
    }

    func testChangedFileHashNoLongerMatchesTheId() {
        let original = manifest([file("model.gguf", hashA)])
        var tampered = original
        tampered.files[0].contentHash = hashB
        XCTAssertThrowsError(try tampered.validate())
    }

    func testUpperCaseOrShortHashIsRefused() {
        XCTAssertThrowsError(try manifest([file("model.gguf", String(repeating: "A", count: 64))]).validate())
        XCTAssertThrowsError(try manifest([file("model.gguf", "abc")]).validate())
    }

    func testNameMustBePlain() {
        var m = manifest([file("model.gguf")])
        for name in ["", " ", "a/b", "..", "."] {
            m.name = name
            XCTAssertThrowsError(try m.validate(), name)
        }
    }
}
