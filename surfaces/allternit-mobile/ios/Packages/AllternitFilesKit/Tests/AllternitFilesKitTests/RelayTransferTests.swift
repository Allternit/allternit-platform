import XCTest
@testable import AllternitFilesKit

/// The two transfer paths the File Provider extension depends on, through
/// the public client against a fake network edge (the cloud-api relay):
///
/// - a chunk sent over the relay is streamed into a base64 JSON envelope
///   piece by piece; the body must decode to exactly the bytes of the range
///   (a piece size that isn't a multiple of 3 would pad mid-stream);
/// - a download resumes after the bytes already on disk, and a file that
///   changed on the computer mid-download is never stitched together.
final class RelayTransferTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("relay-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func bytes(_ count: Int, seed: UInt8 = 7) -> Data {
        Data((0..<count).map { UInt8(truncatingIfNeeded: $0 &* 31 &+ Int(seed)) })
    }

    private struct Envelope: Decodable {
        let method: String
        let path: String
        let headers: [String: String]
        let body: String
        let body_encoding: String
    }

    func testRelayEnvelopeCarriesExactlyTheRangeAndOnlyRelayedHeaders() throws {
        let source = dir.appendingPathComponent("source.bin")
        let content = bytes(2_000_003)
        try content.write(to: source)
        let range: Range<UInt64> = 5..<1_900_000 // spans two 768 KiB pieces, not a multiple of 3

        let request = FilesRequest(method: "PUT", path: "/uploads/up_1/chunks/abc",
                                   headers: ["Content-Type": "application/octet-stream", "Authorization": "Bearer leak",
                                             "X-Allternit-Wake-Ticket": "awt1.x.y"],
                                   body: .file(source, range: range))
        let prepared = try FilesTransport.relay(proxyURL: URL(string: "https://cloud.test/api/v1/runtime-devices/rd_1/proxy")!)
            .prepare(request, bearer: "tok", scratch: dir, timeout: 30)
        let envelopeFile = try XCTUnwrap(prepared.bodyFile)
        let envelope = try JSONDecoder().decode(Envelope.self, from: Data(contentsOf: envelopeFile))

        XCTAssertEqual(envelope.method, "PUT")
        XCTAssertEqual(envelope.path, "/api/v1/files/v2/uploads/up_1/chunks/abc")
        XCTAssertEqual(envelope.body_encoding, "base64")
        XCTAssertEqual(Data(base64Encoded: envelope.body), content.subdata(in: 5..<1_900_000))
        XCTAssertEqual(envelope.headers["x-allternit-wake-ticket"], "awt1.x.y")
        XCTAssertNil(envelope.headers["authorization"], "the caller's Authorization must not ride inside the envelope")
        XCTAssertEqual(prepared.request.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
    }

    func testDownloadResumesAfterThePartialFileAndChecksTheHash() async throws {
        let content = bytes(3_000_001)
        let relay = FakeRelay(content: content, rev: "4-aa")
        let client = makeClient(relay)
        // 1,000,000 bytes from an earlier, interrupted run.
        let partial = client.configuration.scratchDirectory.appendingPathComponent("it_1.4-aa.partial")
        try FileManager.default.createDirectory(at: client.configuration.scratchDirectory, withIntermediateDirectories: true)
        try content.prefix(1_000_000).write(to: partial)

        let destination = dir.appendingPathComponent("out.bin")
        try await client.download(itemId: "it_1", rev: "4-aa", size: UInt64(content.count),
                                  contentHash: Blake3Hasher.hashHex(content), to: destination,
                                  wake: .never, segmentBytes: 1_048_576)

        XCTAssertEqual(try Data(contentsOf: destination), content)
        XCTAssertEqual(relay.ranges.first, "bytes=1000000-2048575", "resume starts after the bytes on disk")
        XCTAssertEqual(relay.ranges.count, 2)
        XCTAssertFalse(FileManager.default.fileExists(atPath: partial.path))
    }

    func testDownloadStopsWhenTheFileChangesOnTheComputer() async throws {
        let content = bytes(2_500_000)
        let relay = FakeRelay(content: content, rev: "4-aa")
        relay.revAfterFirstSegment = "5-bb"
        let client = makeClient(relay)
        let destination = dir.appendingPathComponent("out.bin")

        do {
            try await client.download(itemId: "it_1", rev: "4-aa", size: UInt64(content.count), contentHash: nil,
                                      to: destination, wake: .never, segmentBytes: 1_048_576)
            XCTFail("a changed file must not download")
        } catch FilesError.revisionChanged(let expected, let served) {
            XCTAssertEqual(expected, "4-aa")
            XCTAssertEqual(served, "5-bb")
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.path))
        let partial = client.configuration.scratchDirectory.appendingPathComponent("it_1.4-aa.partial")
        XCTAssertFalse(FileManager.default.fileExists(atPath: partial.path), "old bytes must not be resumed into the new revision")
    }

    private func makeClient(_ relay: FakeRelay) -> FilesClient {
        FilesClient(computerId: "rd_1", configuration: FilesClientConfiguration(
            cloudBaseURL: URL(string: "https://cloud.test")!,
            tokenProvider: { "tok" },
            deviceId: "dev_test_0001",
            deviceName: "iPhone",
            http: relay,
            scratchDirectory: dir.appendingPathComponent("scratch")
        ))
    }
}

/// cloud-api as the client sees it: the endpoints route and the relay
/// proxy, serving `content` for ranged reads with `ETag: "<rev>"`.
private final class FakeRelay: FilesHTTP, @unchecked Sendable {
    let content: Data
    let rev: String
    var revAfterFirstSegment: String?
    private let lock = NSLock()
    private var served = 0
    private(set) var ranges: [String] = []

    init(content: Data, rev: String) {
        self.content = content
        self.rev = rev
    }

    func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        guard request.url?.path == "/api/v1/computer-files/rd_1/endpoints" else {
            return (Data(#"{"error":"not_found","code":"not_found","message":"no"}"#.utf8), response(request, 404, [:]))
        }
        let endpoints = EndpointsResponse(
            computerId: "rd_1", computerState: .awake, wake: .ask,
            candidates: [TransportCandidate(kind: .relay, baseUrl: "https://cloud.test/api/v1/runtime-devices/rd_1/proxy",
                                            priority: 30, maxRequestBytes: 5_242_880)],
            host: .ownDevice)
        return (try JSONEncoder().encode(endpoints), response(request, 200, [:]))
    }

    func download(for request: URLRequest) async throws -> (URL, HTTPURLResponse) {
        struct Envelope: Decodable { let headers: [String: String] }
        let envelope = try JSONDecoder().decode(Envelope.self, from: request.httpBody ?? Data())
        let range = envelope.headers["range"] ?? ""
        let currentRev: String = lock.withLock {
            ranges.append(range)
            served += 1
            return served > 1 ? (revAfterFirstSegment ?? rev) : rev
        }
        let bounds = range.dropFirst("bytes=".count).split(separator: "-").compactMap { Int($0) }
        let slice = content.subdata(in: bounds[0]..<min(bounds[1] + 1, content.count))
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("seg-\(UUID().uuidString)")
        try slice.write(to: file)
        return (file, response(request, 206, ["ETag": "\"\(currentRev)\"",
                                              "Content-Range": "bytes \(bounds[0])-\(bounds[0] + slice.count - 1)/\(content.count)"]))
    }

    func upload(for request: URLRequest, fromFile file: URL) async throws -> (Data, HTTPURLResponse) {
        (Data(), response(request, 500, [:]))
    }

    private func response(_ request: URLRequest, _ status: Int, _ headers: [String: String]) -> HTTPURLResponse {
        HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)!
    }
}
