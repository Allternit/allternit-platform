import Foundation

/// BLAKE3-256 (unkeyed hash mode), a port of the BLAKE3 reference
/// implementation (github.com/BLAKE3-team/BLAKE3, `reference_impl`).
/// Chunk and file hashes in `files/2` are BLAKE3-256 in lower-case hex
/// (SPEC section 5). Incremental: feed any number of `update` calls, then
/// `finalizeHex()`. No heap allocation per block.
public struct Blake3Hasher: Sendable {
    private static let iv: SIMD8<UInt32> = [
        0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A,
        0x510E_527F, 0x9B05_688C, 0x1F83_D9AB, 0x5BE0_CD19,
    ]
    private static let chunkStart: UInt32 = 1 << 0
    private static let chunkEnd: UInt32 = 1 << 1
    private static let parent: UInt32 = 1 << 2
    private static let root: UInt32 = 1 << 3
    private static let blockLen = 64
    private static let chunkLen = 1024

    /// Message word order for each of the 7 rounds: round 0 is the identity,
    /// each next round applies the BLAKE3 permutation to the previous one.
    private static let schedule: [[Int]] = {
        let permutation = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8]
        var rounds: [[Int]] = [Array(0..<16)]
        for _ in 1..<7 {
            let previous = rounds[rounds.count - 1]
            rounds.append(permutation.map { previous[$0] })
        }
        return rounds
    }()

    // Current chunk.
    private var chunkCV: SIMD8<UInt32>
    private var chunkCounter: UInt64 = 0
    private var block = SIMD64<UInt8>()
    private var blockLength = 0
    private var blocksCompressed = 0
    // Chaining values of completed subtrees (at most 54 for 2^64 bytes).
    private var cvStack: [SIMD8<UInt32>] = []

    public init() {
        chunkCV = Self.iv
        cvStack.reserveCapacity(54)
    }

    /// Hashes `data` in one call.
    public static func hashHex(_ data: Data) -> String {
        var hasher = Blake3Hasher()
        hasher.update(data)
        return hasher.finalizeHex()
    }

    public mutating func update(_ data: Data) {
        data.withUnsafeBytes { update(UnsafeRawBufferPointer($0)) }
    }

    public mutating func update(_ bytes: UnsafeRawBufferPointer) {
        guard var base = bytes.baseAddress else { return }
        var remaining = bytes.count
        while remaining > 0 {
            if chunkLength == Self.chunkLen {
                let cv = chunkOutput().chainingValue()
                let totalChunks = chunkCounter + 1
                addChunkChainingValue(cv, totalChunks: totalChunks)
                chunkCV = Self.iv
                chunkCounter = totalChunks
                block = SIMD64<UInt8>()
                blockLength = 0
                blocksCompressed = 0
            }
            let take = min(Self.chunkLen - chunkLength, remaining)
            chunkUpdate(base, count: take)
            base += take
            remaining -= take
        }
    }

    /// The 32-byte digest as lower-case hex. Does not change the hasher.
    public func finalizeHex() -> String {
        var output = chunkOutput()
        var index = cvStack.count
        while index > 0 {
            index -= 1
            output = Self.parentOutput(left: cvStack[index], right: output.chainingValue())
        }
        let words = output.rootWords()
        var hex = ""
        hex.reserveCapacity(64)
        let digits = Array("0123456789abcdef")
        for i in 0..<8 {
            var word = words[i]
            for _ in 0..<4 {
                let byte = Int(word & 0xFF)
                hex.append(digits[byte >> 4])
                hex.append(digits[byte & 0x0F])
                word >>= 8
            }
        }
        return hex
    }

    // MARK: Chunk state

    private var chunkLength: Int { Self.blockLen * blocksCompressed + blockLength }

    private var startFlag: UInt32 { blocksCompressed == 0 ? Self.chunkStart : 0 }

    private mutating func chunkUpdate(_ pointer: UnsafeRawPointer, count: Int) {
        var p = pointer
        var remaining = count
        while remaining > 0 {
            if blockLength == Self.blockLen {
                let words = Self.words(block)
                let out = Self.compress(cv: chunkCV, block: words, counter: chunkCounter,
                                        blockLength: UInt32(Self.blockLen), flags: startFlag)
                chunkCV = out.lowHalf
                blocksCompressed += 1
                block = SIMD64<UInt8>()
                blockLength = 0
            }
            let take = min(Self.blockLen - blockLength, remaining)
            for i in 0..<take {
                block[blockLength + i] = p.load(fromByteOffset: i, as: UInt8.self)
            }
            blockLength += take
            p += take
            remaining -= take
        }
    }

    private func chunkOutput() -> Output {
        Output(inputCV: chunkCV, block: Self.words(block), counter: chunkCounter,
               blockLength: UInt32(blockLength), flags: startFlag | Self.chunkEnd)
    }

    private mutating func addChunkChainingValue(_ cv: SIMD8<UInt32>, totalChunks: UInt64) {
        var newCV = cv
        var total = totalChunks
        while total & 1 == 0 {
            let left = cvStack.removeLast()
            newCV = Self.parentOutput(left: left, right: newCV).chainingValue()
            total >>= 1
        }
        cvStack.append(newCV)
    }

    // MARK: Compression

    private struct Output {
        let inputCV: SIMD8<UInt32>
        let block: SIMD16<UInt32>
        let counter: UInt64
        let blockLength: UInt32
        let flags: UInt32

        func chainingValue() -> SIMD8<UInt32> {
            Blake3Hasher.compress(cv: inputCV, block: block, counter: counter,
                                  blockLength: blockLength, flags: flags).lowHalf
        }

        /// First 32 bytes of the root output (output block counter 0).
        func rootWords() -> SIMD8<UInt32> {
            Blake3Hasher.compress(cv: inputCV, block: block, counter: 0,
                                  blockLength: blockLength, flags: flags | Blake3Hasher.root).lowHalf
        }
    }

    private static func parentOutput(left: SIMD8<UInt32>, right: SIMD8<UInt32>) -> Output {
        Output(inputCV: iv, block: SIMD16(lowHalf: left, highHalf: right), counter: 0,
               blockLength: UInt32(blockLen), flags: parent)
    }

    private static func words(_ bytes: SIMD64<UInt8>) -> SIMD16<UInt32> {
        var words = SIMD16<UInt32>()
        for i in 0..<16 {
            let o = i * 4
            words[i] = UInt32(bytes[o]) | UInt32(bytes[o + 1]) << 8
                | UInt32(bytes[o + 2]) << 16 | UInt32(bytes[o + 3]) << 24
        }
        return words
    }

    @inline(__always)
    private static func rotr(_ x: UInt32, _ n: UInt32) -> UInt32 { (x >> n) | (x << (32 - n)) }

    @inline(__always)
    private static func g(_ s: inout SIMD16<UInt32>, _ a: Int, _ b: Int, _ c: Int, _ d: Int, _ mx: UInt32, _ my: UInt32) {
        s[a] = s[a] &+ s[b] &+ mx
        s[d] = rotr(s[d] ^ s[a], 16)
        s[c] = s[c] &+ s[d]
        s[b] = rotr(s[b] ^ s[c], 12)
        s[a] = s[a] &+ s[b] &+ my
        s[d] = rotr(s[d] ^ s[a], 8)
        s[c] = s[c] &+ s[d]
        s[b] = rotr(s[b] ^ s[c], 7)
    }

    private static func compress(cv: SIMD8<UInt32>, block m: SIMD16<UInt32>, counter: UInt64,
                                 blockLength: UInt32, flags: UInt32) -> SIMD16<UInt32> {
        var s = SIMD16<UInt32>(
            cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7],
            iv[0], iv[1], iv[2], iv[3],
            UInt32(truncatingIfNeeded: counter), UInt32(truncatingIfNeeded: counter >> 32),
            blockLength, flags
        )
        for r in 0..<7 {
            let o = schedule[r]
            g(&s, 0, 4, 8, 12, m[o[0]], m[o[1]])
            g(&s, 1, 5, 9, 13, m[o[2]], m[o[3]])
            g(&s, 2, 6, 10, 14, m[o[4]], m[o[5]])
            g(&s, 3, 7, 11, 15, m[o[6]], m[o[7]])
            g(&s, 0, 5, 10, 15, m[o[8]], m[o[9]])
            g(&s, 1, 6, 11, 12, m[o[10]], m[o[11]])
            g(&s, 2, 7, 8, 13, m[o[12]], m[o[13]])
            g(&s, 3, 4, 9, 14, m[o[14]], m[o[15]])
        }
        for i in 0..<8 {
            s[i] ^= s[i + 8]
            s[i + 8] ^= cv[i]
        }
        return s
    }
}
