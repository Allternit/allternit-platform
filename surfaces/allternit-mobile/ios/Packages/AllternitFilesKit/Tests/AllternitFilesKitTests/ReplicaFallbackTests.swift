import CryptoKit
import XCTest
@testable import AllternitFilesKit

/// Opening a file on a sleeping computer (SPEC section 10, step 4): the
/// client reads it from a replica on another computer through cloud-api,
/// without one request to the sleeping computer, and a replica that was
/// changed never reaches disk. The network edge (cloud-api endpoints,
/// placement and the relay to both computers) is faked; the replica bytes
/// are the sealed vectors in `contracts/files-drive/vectors/keys-v1.json`.
final class ReplicaFallbackTests: XCTestCase {
    private struct Vectors: Decodable {
        struct Envelope: Decodable { let device_secret: String; let key_envelope: KeyEnvelope }
        struct Replica: Decodable {
            let replica_id: String
            let head_blob: String
            let catalog_part_locator: String
            let catalog_part_blob: String
            let item_id: String
            let chunk_locator: String
            let chunk_blob: String
            let plaintext: String
            let plaintext_hash: String
        }
        let envelope: Envelope
        let replica: Replica
    }

    private var dir: URL!
    private var v: Vectors!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("replica-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        v = try JSONDecoder().decode(Vectors.self, from: KeyVectorTests.vectors())
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func h(_ hex: String) -> Data { FilesKeyCrypto.unhex(hex)! }

    private func makeClient(_ cloud: FakeCloud) -> FilesClient {
        let store = FilesSharedStore(appGroup: "test.files.\(UUID().uuidString)", keychainGroup: nil)
        store.memorySecrets = [:]
        store.deviceId = v.envelope.key_envelope.deviceId
        store.setFilesDeviceSecret(h(v.envelope.device_secret))
        store.setFilesKeyEnvelopes([v.envelope.key_envelope])
        let configuration = FilesClientConfiguration(
            cloudBaseURL: URL(string: "https://cloud.test")!, tokenProvider: { "tok" },
            deviceId: v.envelope.key_envelope.deviceId, deviceName: "iPhone", http: cloud,
            scratchDirectory: dir, sharedStore: store)
        return FilesClient(computerId: FakeCloud.source, configuration: configuration)
    }

    private func fakeCloud(chunkBlob: Data? = nil) -> FakeCloud {
        let rid = v.replica.replica_id
        return FakeCloud(hostBlobs: [
            "/api/v1/files/v2/replicas/\(rid)/head": h(v.replica.head_blob),
            "/api/v1/files/v2/replicas/\(rid)/blobs/\(v.replica.catalog_part_locator)": h(v.replica.catalog_part_blob),
            "/api/v1/files/v2/replicas/\(rid)/blobs/\(v.replica.chunk_locator)": chunkBlob ?? h(v.replica.chunk_blob),
        ])
    }

    func testAnAsleepComputersFileComesFromTheReplicaWithoutTouchingIt() async throws {
        XCTAssertEqual(FilesKeyCrypto.replicaId(sourceComputerId: FakeCloud.source, rootId: "root_home", hostDeviceId: FakeCloud.host),
                       v.replica.replica_id)
        let cloud = fakeCloud()
        let client = makeClient(cloud)
        let plaintext = h(v.replica.plaintext)
        let destination = dir.appendingPathComponent("note.txt")
        let served = try await client.download(itemId: v.replica.item_id, rev: "r_7", size: UInt64(plaintext.count),
                                               contentHash: v.replica.plaintext_hash, rootId: "root_home", to: destination)
        XCTAssertEqual(served, .replica)
        XCTAssertEqual(try Data(contentsOf: destination), plaintext)
        XCTAssertEqual(cloud.sourceRelayCalls, 0, "the sleeping computer got a request")
        XCTAssertEqual(cloud.wakeConsentCalls, 0)
    }

    func testAChangedReplicaChunkNeverReachesDiskAndTheAsleepAnswerStands() async throws {
        var tampered = h(v.replica.chunk_blob)
        tampered[tampered.endIndex - 20] ^= 0x01
        let cloud = fakeCloud(chunkBlob: tampered)
        let client = makeClient(cloud)
        let destination = dir.appendingPathComponent("note.txt")
        do {
            try await client.download(itemId: v.replica.item_id, rev: "r_7", size: UInt64(h(v.replica.plaintext).count),
                                      contentHash: v.replica.plaintext_hash, rootId: "root_home", to: destination, wake: .never)
            XCTFail("a tampered replica was served")
        } catch FilesError.wakeConsentRequired {
            // The live path's answer: the user is asked to wake it.
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.path))
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: dir.path).filter { $0.hasPrefix("replica-") }
        XCTAssertEqual(leftovers, [])
    }

    func testAReplicaThatIsBehindIsNotServed() async throws {
        let cloud = fakeCloud()
        let client = makeClient(cloud)
        do {
            try await client.download(itemId: v.replica.item_id, rev: "r_8", size: UInt64(h(v.replica.plaintext).count),
                                      contentHash: String(repeating: "ab", count: 32), rootId: "root_home",
                                      to: dir.appendingPathComponent("note.txt"), wake: .never)
            XCTFail("an older copy was served for a newer revision")
        } catch FilesError.wakeConsentRequired {}
    }
}

/// cloud-api as the phone sees it: the source computer is asleep (its relay
/// answers `wake_consent_required`), the host computer is awake and serves
/// sealed replica blobs over the relay.
private final class FakeCloud: FilesHTTP, @unchecked Sendable {
    static let source = "rd_mac_vector"
    static let host = "rd_cloud_vector"
    let hostBlobs: [String: Data]
    private let lock = NSLock()
    private var _sourceRelayCalls = 0
    private var _wakeConsentCalls = 0
    var sourceRelayCalls: Int { lock.withLock { _sourceRelayCalls } }
    var wakeConsentCalls: Int { lock.withLock { _wakeConsentCalls } }

    init(hostBlobs: [String: Data]) { self.hostBlobs = hostBlobs }

    private struct Envelope: Decodable { let path: String }

    func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let path = request.url?.path ?? ""
        switch path {
        case "/api/v1/computer-files/\(Self.source)/endpoints":
            return try (json(endpoints(Self.source, .asleep)), response(request, 200))
        case "/api/v1/computer-files/\(Self.host)/endpoints":
            return try (json(endpoints(Self.host, .awake)), response(request, 200))
        case "/api/v1/computer-files/\(Self.source)/placement":
            let body = #"{"computer_id":"\#(Self.source)","root_id":"root_home","needed_bytes":0,"options":[{"device_id":"\#(Self.host)","label":"Cloud","score":80,"eligible":true,"reason":"Online now.","free_bytes":1000000,"computer_state":"awake"}]}"#
            return (Data(body.utf8), response(request, 200))
        case "/api/v1/computer-files/\(Self.source)/wake-consent":
            lock.withLock { _wakeConsentCalls += 1 }
            return (Data(), response(request, 500))
        case "/api/v1/runtime-devices/\(Self.source)/proxy":
            lock.withLock { _sourceRelayCalls += 1 }
            let body = #"{"error":"wake_consent_required","code":"wake_consent_required","message":"Asleep.","computer_state":"asleep"}"#
            return (Data(body.utf8), response(request, 409))
        case "/api/v1/runtime-devices/\(Self.host)/proxy":
            let envelope = try JSONDecoder().decode(Envelope.self, from: request.httpBody ?? Data())
            if let blob = hostBlobs[envelope.path] { return (blob, response(request, 200)) }
            return (Data(#"{"error":"not_found","code":"not_found","message":"no"}"#.utf8), response(request, 404))
        default:
            return (Data(#"{"error":"not_found","code":"not_found","message":"no"}"#.utf8), response(request, 404))
        }
    }

    func download(for request: URLRequest) async throws -> (URL, HTTPURLResponse) {
        let (data, response) = try await data(for: request)
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("blob-\(UUID().uuidString)")
        try data.write(to: file)
        return (file, response)
    }

    func upload(for request: URLRequest, fromFile file: URL) async throws -> (Data, HTTPURLResponse) {
        (Data(), response(request, 500))
    }

    private func endpoints(_ id: String, _ state: ComputerState) -> EndpointsResponse {
        EndpointsResponse(computerId: id, computerState: state, wake: .ask,
                          candidates: [TransportCandidate(kind: .relay, baseUrl: "https://cloud.test/api/v1/runtime-devices/\(id)/proxy",
                                                          priority: 30, maxRequestBytes: 5_242_880)],
                          host: .ownDevice)
    }

    private func json<T: Encodable>(_ value: T) throws -> Data { try JSONEncoder().encode(value) }

    private func response(_ request: URLRequest, _ status: Int) -> HTTPURLResponse {
        HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: [:])!
    }
}
