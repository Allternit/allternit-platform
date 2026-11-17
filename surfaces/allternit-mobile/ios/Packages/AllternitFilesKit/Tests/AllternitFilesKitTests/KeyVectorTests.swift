import CryptoKit
import XCTest
@testable import AllternitFilesKit

/// The Files key code must produce and open exactly the bytes the Rust
/// engine does (SPEC section 13), or a phone can't read a replica a computer
/// sealed. Every case comes from `contracts/files-drive/vectors/keys-v1.json`
/// (also run by the Android tests): the XChaCha20-Poly1305 draft vectors,
/// RFC 5869 HKDF, the official keyed BLAKE3 vectors, an envelope wrapped to a
/// known device key, and a sealed replica (head, catalog part, chunk).
final class KeyVectorTests: XCTestCase {
    private struct Vectors: Decodable {
        struct HChaCha: Decodable { let key: String; let nonce: String; let subkey: String }
        struct XChaCha: Decodable { let key: String; let nonce: String; let aad: String; let plaintext: String; let ciphertext_and_tag: String }
        struct Hkdf: Decodable { let source: String; let ikm: String; let salt: String; let info: String; let length: Int; let okm: String }
        struct Keyed: Decodable { let key: String; let input: String; let hash: String }
        struct KeyedOfficial: Decodable {
            struct Case: Decodable { let input_len: Int; let hash: String }
            let key: String
            let cases: [Case]
        }
        struct ReplicaIdCase: Decodable { let source_computer_id: String; let root_id: String; let host_device_id: String; let replica_id: String }
        struct Confirm: Decodable { let device_pub: String; let confirm_code: String }
        struct Envelope: Decodable { let frk: String; let device_secret: String; let device_pub: String; let key_envelope: KeyEnvelope }
        struct Replica: Decodable {
            let frk: String
            let key_id: String
            let replica_id: String
            let head_blob: String
            let head: ReplicaHead
            let catalog_part_locator: String
            let catalog_part_blob: String
            let item_id: String
            let chunk_locator: String
            let chunk_blob: String
            let plaintext: String
            let plaintext_hash: String
        }
        let hchacha20: HChaCha
        let xchacha20poly1305: XChaCha
        let hkdf_sha256: [Hkdf]
        let blake3_keyed: Keyed
        let blake3_keyed_official: KeyedOfficial
        let replica_id: ReplicaIdCase
        let confirm_code: Confirm
        let envelope: Envelope
        let replica: Replica
    }

    /// Repo root from this file (8 components up, as ChunkerVectorTests).
    static func vectors() throws -> Data {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<8 { url.deleteLastPathComponent() }
        return try Data(contentsOf: url.appendingPathComponent("contracts/files-drive/vectors/keys-v1.json"))
    }

    private var v: Vectors!

    override func setUpWithError() throws {
        v = try JSONDecoder().decode(Vectors.self, from: Self.vectors())
    }

    private func h(_ hex: String) -> Data { FilesKeyCrypto.unhex(hex)! }

    func testHChaCha20MatchesTheDraft() {
        XCTAssertEqual(Data(XChaCha20Poly1305.hChaCha20(key: [UInt8](h(v.hchacha20.key)), nonce16: [UInt8](h(v.hchacha20.nonce)))),
                       h(v.hchacha20.subkey))
    }

    func testXChaChaSealsAndOpensTheDraftVectorAndRefusesAChangedByte() throws {
        let x = v.xchacha20poly1305
        let sealed = try XChaCha20Poly1305.seal(h(x.plaintext), key: h(x.key), nonce: h(x.nonce), aad: h(x.aad))
        XCTAssertEqual(FilesKeyCrypto.hex(sealed), x.ciphertext_and_tag)
        XCTAssertEqual(try XChaCha20Poly1305.open(sealed, key: h(x.key), nonce: h(x.nonce), aad: h(x.aad)), h(x.plaintext))
        var tampered = sealed
        tampered[tampered.startIndex + 3] ^= 0x01
        XCTAssertThrowsError(try XChaCha20Poly1305.open(tampered, key: h(x.key), nonce: h(x.nonce), aad: h(x.aad)))
        XCTAssertThrowsError(try XChaCha20Poly1305.open(sealed, key: h(x.key), nonce: h(x.nonce), aad: Data("other".utf8)))
    }

    func testHKDF() {
        for c in v.hkdf_sha256 {
            let okm = FilesKeyCrypto.hkdf(ikm: SymmetricKey(data: h(c.ikm)), salt: h(c.salt), info: h(c.info), length: c.length)
            XCTAssertEqual(FilesKeyCrypto.hex(okm), c.okm, c.source)
        }
    }

    func testKeyedBlake3AndLocator() throws {
        let official = v.blake3_keyed_official
        for c in official.cases {
            let input = Data((0..<c.input_len).map { UInt8($0 % 251) })
            XCTAssertEqual(Blake3Hasher.keyedHashHex(key: h(official.key), input), c.hash, "keyed len \(c.input_len)")
        }
        XCTAssertEqual(Blake3Hasher.keyedHashHex(key: h(v.blake3_keyed.key), h(v.blake3_keyed.input)), v.blake3_keyed.hash)
        let sealer = ChunkSealer(frk: FilesRootKey(keyId: v.replica.key_id, keyBytes: h(v.replica.frk)), replicaId: v.replica.replica_id)
        XCTAssertEqual(try sealer.locator(plaintextHash: v.replica.plaintext_hash), v.replica.chunk_locator)
    }

    func testReplicaIdAndConfirmCode() {
        let r = v.replica_id
        XCTAssertEqual(FilesKeyCrypto.replicaId(sourceComputerId: r.source_computer_id, rootId: r.root_id, hostDeviceId: r.host_device_id),
                       r.replica_id)
        XCTAssertEqual(FilesKeyCrypto.confirmCode(devicePub: h(v.confirm_code.device_pub)), v.confirm_code.confirm_code)
    }

    func testEnvelopeOpensOnlyOnItsDevice() throws {
        let e = v.envelope
        let device = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: h(e.device_secret))
        XCTAssertEqual(device.publicKey.rawRepresentation, h(e.device_pub))
        let frk = try FilesKeyCrypto.unwrap(e.key_envelope, deviceSecret: device)
        XCTAssertEqual(frk.rawBytes, h(e.frk))
        XCTAssertEqual(frk.keyId, e.key_envelope.keyId)

        let other = Curve25519.KeyAgreement.PrivateKey()
        XCTAssertThrowsError(try FilesKeyCrypto.unwrap(e.key_envelope, deviceSecret: other)) {
            XCTAssertEqual(($0 as? FilesError)?.code, .keyUnavailable)
        }
        var renamed = e.key_envelope
        renamed.deviceId = "dev_ios_someoneelse"
        XCTAssertThrowsError(try FilesKeyCrypto.unwrap(renamed, deviceSecret: device), "the device id is bound by the AAD and the KEK")

        // An approving device wraps the key to a new device; only that device opens it.
        let newDevice = Curve25519.KeyAgreement.PrivateKey()
        let granted = try FilesKeyCrypto.wrap(frk, toDevicePub: newDevice.publicKey.rawRepresentation,
                                              deviceId: "dev_and_newphone", createdAt: "2026-11-15T21:00:00Z")
        XCTAssertEqual(granted.wrapAlg, "x25519-hkdf-sha256-xchacha20poly1305")
        XCTAssertEqual(try FilesKeyCrypto.unwrap(granted, deviceSecret: newDevice).rawBytes, h(e.frk))
        XCTAssertThrowsError(try FilesKeyCrypto.unwrap(granted, deviceSecret: device))
    }

    func testSealedReplicaOpensHeadCatalogAndChunk() async throws {
        let r = v.replica
        let frk = FilesRootKey(keyId: r.key_id, keyBytes: h(r.frk))
        XCTAssertEqual(try ReplicaReader.headKeyId(h(r.head_blob)), r.key_id)
        let head = try ReplicaReader.openHead(h(r.head_blob), frk: frk, replicaId: r.replica_id)
        XCTAssertEqual(head, r.head)
        XCTAssertThrowsError(try ReplicaReader.openHead(h(r.head_blob), frk: frk, replicaId: "rp_other"))

        let blobs = [r.catalog_part_locator: h(r.catalog_part_blob), r.chunk_locator: h(r.chunk_blob)]
        let catalog = try await ReplicaReader.readCatalog(head: head, frk: frk, maxBytes: 1 << 20) { blobs[$0] }
        let entry = try XCTUnwrap(catalog.entry(r.item_id))
        XCTAssertEqual(entry.contentHash, r.plaintext_hash)
        XCTAssertEqual(entry.chunks.map(\.hash), [r.plaintext_hash])

        let sealer = ChunkSealer(frk: frk, replicaId: r.replica_id)
        XCTAssertEqual(try sealer.open(blob: h(r.chunk_blob), plaintextHash: r.plaintext_hash), h(r.plaintext))

        // A changed ciphertext byte, a changed header field, or the wrong key fails.
        var flipped = h(r.chunk_blob)
        flipped[flipped.endIndex - 20] ^= 0x01
        XCTAssertThrowsError(try sealer.open(blob: flipped, plaintextHash: r.plaintext_hash)) {
            XCTAssertEqual(($0 as? FilesError)?.code, .integrityFailed)
        }
        let wrongKey = ChunkSealer(frk: FilesRootKey(keyId: r.key_id, keyBytes: Data(repeating: 7, count: 32)), replicaId: r.replica_id)
        XCTAssertThrowsError(try wrongKey.open(blob: h(r.chunk_blob), plaintextHash: r.plaintext_hash))
        let rotated = ChunkSealer(frk: FilesRootKey(keyId: "fk_rotated", keyBytes: h(r.frk)), replicaId: r.replica_id)
        XCTAssertThrowsError(try rotated.open(blob: h(r.chunk_blob), plaintextHash: r.plaintext_hash)) {
            XCTAssertEqual(($0 as? FilesError)?.code, .keyUnavailable)
        }
    }

    func testOwnSealOpensAgain() throws {
        let frk = FilesRootKey.generate()
        let sealer = ChunkSealer(frk: frk, replicaId: "rp_roundtrip")
        let plain = Data((0..<70_000).map { UInt8(truncatingIfNeeded: $0 &* 7) })
        let (locator, blob) = try sealer.seal(plain)
        XCTAssertEqual(locator, try sealer.locator(plaintextHash: Blake3Hasher.hashHex(plain)))
        XCTAssertEqual(try sealer.open(blob: blob, plaintextHash: Blake3Hasher.hashHex(plain)), plain)
    }
}
