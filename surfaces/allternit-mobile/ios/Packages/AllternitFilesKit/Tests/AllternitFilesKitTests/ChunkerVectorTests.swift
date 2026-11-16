import XCTest
@testable import AllternitFilesKit

/// The pinned chunker must cut exactly where the Rust engine cuts, or dedup
/// and replica checks miss (SPEC section 5). This loads the shared vectors,
/// `contracts/files-drive/vectors/chunker-v1.json`, regenerates each input
/// and compares every chunk's offset, size and BLAKE3 plus the file hash.
/// It also checks the gear table copy and runs the file streaming path.
final class ChunkerVectorTests: XCTestCase {
    private struct Vectors: Decodable {
        struct Params: Decodable {
            let min_bytes: Int
            let avg_bytes: Int
            let max_bytes: Int
            let mask_s: String
            let mask_l: String
            let mask_s_ls: String
            let mask_l_ls: String
        }
        struct Input: Decodable {
            let kind: String
            let len: Int
            let seed: UInt64?
        }
        struct Chunk: Decodable {
            let hash: String
            let offset: UInt64
            let size: UInt64
        }
        struct Case: Decodable {
            let name: String
            let input: Input
            let size: UInt64
            let content_hash: String
            let chunks: [Chunk]
        }
        let params: Params
        let gear: [String]
        let cases: [Case]
    }

    /// Repo root from this file: drop the file name, then
    /// AllternitFilesKitTests, Tests, AllternitFilesKit, Packages, ios,
    /// allternit-mobile and surfaces (8 components).
    private static var vectorsURL: URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<8 { url.deleteLastPathComponent() }
        return url.appendingPathComponent("contracts/files-drive/vectors/chunker-v1.json")
    }

    private func loadVectors() throws -> Vectors {
        let data = try Data(contentsOf: Self.vectorsURL)
        return try JSONDecoder().decode(Vectors.self, from: data)
    }

    private static func hex(_ value: String) -> UInt64 {
        UInt64(value.dropFirst(2), radix: 16) ?? 0
    }

    private static func input(_ input: Vectors.Input) -> Data {
        switch input.kind {
        case "zeros":
            return Data(count: input.len)
        case "splitmix64":
            var bytes = [UInt8]()
            bytes.reserveCapacity(input.len + 8)
            var x = input.seed ?? 0
            while bytes.count < input.len {
                x = x &+ 0x9E37_79B9_7F4A_7C15
                var z = x
                z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
                z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
                z ^= z >> 31
                var word = z
                for _ in 0..<8 {
                    bytes.append(UInt8(truncatingIfNeeded: word))
                    word >>= 8
                }
            }
            return Data(bytes.prefix(input.len))
        default:
            XCTFail("unknown input kind \(input.kind)")
            return Data()
        }
    }

    func testParametersAndGearTableMatchTheVectors() throws {
        let vectors = try loadVectors()
        XCTAssertEqual(vectors.params.min_bytes, FastCDC.minSize)
        XCTAssertEqual(vectors.params.avg_bytes, FastCDC.avgSize)
        XCTAssertEqual(vectors.params.max_bytes, FastCDC.maxSize)
        XCTAssertEqual(Self.hex(vectors.params.mask_s), FastCDC.maskS)
        XCTAssertEqual(Self.hex(vectors.params.mask_l), FastCDC.maskL)
        XCTAssertEqual(Self.hex(vectors.params.mask_s_ls), FastCDC.maskSLs)
        XCTAssertEqual(Self.hex(vectors.params.mask_l_ls), FastCDC.maskLLs)
        let gear: [UInt64] = vectors.gear.map { Self.hex($0) }
        XCTAssertEqual(gear, FastCDC.gear)
    }

    func testInMemoryChunkerMatchesEveryCase() throws {
        let vectors = try loadVectors()
        XCTAssertFalse(vectors.cases.isEmpty)
        for testCase in vectors.cases {
            let data = Self.input(testCase.input)
            XCTAssertEqual(UInt64(data.count), testCase.size, testCase.name)
            let result = FastCDC.chunk(data)
            XCTAssertEqual(result.contentHash, testCase.content_hash, "\(testCase.name): file hash")
            XCTAssertEqual(result.chunks.map(\.offset), testCase.chunks.map(\.offset), "\(testCase.name): offsets")
            XCTAssertEqual(result.chunks.map(\.size), testCase.chunks.map(\.size), "\(testCase.name): sizes")
            XCTAssertEqual(result.chunks.map(\.hash), testCase.chunks.map(\.hash), "\(testCase.name): chunk hashes")
        }
    }

    /// The streaming path (what uploads use) must cut the same as in memory,
    /// including when a read ends in the middle of a chunk.
    func testFileChunkerMatchesEveryCase() throws {
        let vectors = try loadVectors()
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("chunker-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        for testCase in vectors.cases {
            let file = dir.appendingPathComponent("input.bin")
            try Self.input(testCase.input).write(to: file)
            var chunker = FileChunker()
            chunker.readSize = 300_007 // odd size: reads straddle chunk boundaries
            let result = try chunker.chunk(fileAt: file)
            XCTAssertEqual(result.size, testCase.size, testCase.name)
            XCTAssertEqual(result.contentHash, testCase.content_hash, "\(testCase.name): file hash")
            XCTAssertEqual(result.chunks.map(\.offset), testCase.chunks.map(\.offset), "\(testCase.name): offsets")
            XCTAssertEqual(result.chunks.map(\.hash), testCase.chunks.map(\.hash), "\(testCase.name): chunk hashes")
            XCTAssertEqual(try FileChunker.hashFile(at: file), testCase.content_hash, "\(testCase.name): hashFile")
        }
    }

    /// BLAKE3 official test vectors (input byte i = i % 251), first 32 bytes
    /// of the hash output, covering one block, a chunk boundary and the tree.
    func testBlake3OfficialVectors() {
        let expected: [Int: String] = [
            0: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            1: "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
            1023: "10108970eeda3eb932baac1428c7a2163b0e924c9a9e25b35bba72b28f70bd11",
            1024: "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
            1025: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
            2048: "e776b6028c7cd22a4d0ba182a8bf62205d2ef576467e838ed6f2529b85fba24a",
            3072: "b98cb0ff3623be03326b373de6b9095218513e64f1ee2edd2525c7ad1e5cffd2",
            31744: "62b6960e1a44bcc1eb1a611a8d6235b6b4b78f32e7abc4fb4c6cdcce94895c47",
        ]
        for (length, hash) in expected {
            var bytes = [UInt8](repeating: 0, count: length)
            for i in 0..<length { bytes[i] = UInt8(i % 251) }
            let data = Data(bytes)
            XCTAssertEqual(Blake3Hasher.hashHex(data), hash, "length \(length)")
            // Incremental feeding must give the same answer.
            var hasher = Blake3Hasher()
            var offset = 0
            while offset < data.count {
                let end = min(offset + 97, data.count)
                hasher.update(data.subdata(in: offset..<end))
                offset = end
            }
            XCTAssertEqual(hasher.finalizeHex(), hash, "length \(length), incremental")
        }
    }
}
