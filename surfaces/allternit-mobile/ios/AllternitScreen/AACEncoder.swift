// Ported from iCTRL (https://github.com/Allternit/allternit-ictrl, iCTRLBroadcast/AACEncoder.swift).
// Copyright iCTRL authors. Licensed under the Apache License, Version 2.0;
// see http://www.apache.org/licenses/LICENSE-2.0. Unmodified apart from this notice.

import AudioToolbox
import CoreMedia
import Foundation

/// App sound for the live screen: ReplayKit `.audioApp` PCM in, raw AAC-LC
/// access units out (no ADTS — the AudioSpecificConfig travels once, in
/// AUDIO_CONFIG, which is what a WebCodecs `mp4a.40.2` decoder wants as its
/// `description`).
///
/// ReplayKit's app audio is not one fixed format: 44.1 or 48 kHz, mono or
/// stereo, interleaved or not, Int16 (sometimes big-endian) or Float32,
/// depending on device and iOS version. The converter is built from the
/// sample buffer's own ASBD and rebuilt whenever that changes, keeping the
/// input rate and channel count (no resampling), so AUDIO_CONFIG reports what
/// is really encoded.
///
/// Synchronous on the caller's queue: `encode` copies the PCM it is given,
/// drains every whole AAC packet that copy completes, and keeps at most the
/// remainder (< 1024 frames) — nothing builds up toward the extension's 50 MB
/// ceiling.
final class AACEncoder {
    /// What AUDIO_CONFIG carries.
    struct Format: Equatable {
        let sampleRate: Int
        let channels: Int
        /// Bare AudioSpecificConfig (2 bytes for AAC-LC), not the esds box.
        let asc: Data
    }

    /// The current output format, set once a converter exists.
    private(set) var format: Format?

    private var converter: AudioConverterRef?
    private var input = AudioStreamBasicDescription()
    private var outputCapacity = 0
    private var output: UnsafeMutableRawPointer?

    // PCM not yet consumed by the converter: one Data per input buffer
    // (1 when interleaved, one per channel when not).
    private var pending: [Data] = []
    private var pendingFrames = 0
    // The bytes handed to the converter in the last input callback. They must
    // stay valid until the next callback, so they live here, not on a stack.
    private var handed: [UnsafeMutableRawPointer] = []
    private var handedCapacity = 0

    /// Status the input callback returns when it has no more PCM for now; the
    /// converter stops and keeps its state for the next `encode`.
    private static let needMoreInput: OSStatus = 0x6E6D6F72 // 'nmor'
    private static let framesPerPacket = 1024

    /// Encodes one ReplayKit audio buffer. Returns the AAC access units it
    /// completed (often one or none), and whether the output format changed
    /// (the caller then resends AUDIO_CONFIG before these packets).
    func encode(_ sample: CMSampleBuffer) -> (packets: [Data], formatChanged: Bool) {
        guard let desc = CMSampleBufferGetFormatDescription(sample),
              let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(desc) else { return ([], false) }
        let asbd = asbdPtr.pointee
        guard asbd.mFormatID == kAudioFormatLinearPCM, asbd.mBytesPerFrame > 0,
              asbd.mChannelsPerFrame > 0, asbd.mSampleRate > 0 else { return ([], false) }

        var changed = false
        if converter == nil || !Self.same(asbd, input) {
            guard prepare(asbd) else { return ([], false) }
            changed = true
        }
        guard append(sample) else { return ([], changed) }
        return (drain(), changed)
    }

    /// Drops buffered PCM and the converter's own state — on pause, so a
    /// resumed stream doesn't open with sound from before it.
    func reset() {
        pending = pending.map { _ in Data() }
        pendingFrames = 0
        if let c = converter { AudioConverterReset(c) }
    }

    func invalidate() {
        if let c = converter { AudioConverterDispose(c) }
        converter = nil
        format = nil
        pending = []
        pendingFrames = 0
        output?.deallocate()
        output = nil
        outputCapacity = 0
        handed.forEach { $0.deallocate() }
        handed = []
        handedCapacity = 0
    }

    deinit { invalidate() }

    // MARK: - Converter

    private static func same(_ a: AudioStreamBasicDescription, _ b: AudioStreamBasicDescription) -> Bool {
        a.mSampleRate == b.mSampleRate && a.mFormatID == b.mFormatID && a.mFormatFlags == b.mFormatFlags
            && a.mBytesPerFrame == b.mBytesPerFrame && a.mChannelsPerFrame == b.mChannelsPerFrame
            && a.mBitsPerChannel == b.mBitsPerChannel
    }

    private func prepare(_ asbd: AudioStreamBasicDescription) -> Bool {
        invalidate()
        var source = asbd
        // AAC-LC tops out at 48 kHz in practice and stereo is all the viewer
        // plays; anything else is converted down by the converter itself.
        let rate = (8_000...48_000).contains(asbd.mSampleRate) ? asbd.mSampleRate : 48_000
        let channels = min(Int(asbd.mChannelsPerFrame), 2)
        var target = AudioStreamBasicDescription(
            mSampleRate: rate, mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: AudioFormatFlags(MPEG4ObjectID.AAC_LC.rawValue), mBytesPerPacket: 0,
            mFramesPerPacket: UInt32(Self.framesPerPacket), mBytesPerFrame: 0,
            mChannelsPerFrame: UInt32(channels), mBitsPerChannel: 0, mReserved: 0)

        // Software codec first: the extension runs in the background, where
        // the hardware encoder can be unavailable.
        var c: AudioConverterRef?
        var classes = [
            AudioClassDescription(mType: kAudioEncoderComponentType, mSubType: kAudioFormatMPEG4AAC,
                                  mManufacturer: kAppleSoftwareAudioCodecManufacturer),
            AudioClassDescription(mType: kAudioEncoderComponentType, mSubType: kAudioFormatMPEG4AAC,
                                  mManufacturer: kAppleHardwareAudioCodecManufacturer),
        ]
        var status = AudioConverterNewSpecific(&source, &target, UInt32(classes.count), &classes, &c)
        if status != noErr || c == nil {
            status = AudioConverterNew(&source, &target, &c)
        }
        guard status == noErr, let conv = c else { return false }

        var bitRate: UInt32 = channels == 1 ? 64_000 : 128_000
        AudioConverterSetProperty(conv, kAudioConverterEncodeBitRate,
                                  UInt32(MemoryLayout<UInt32>.size), &bitRate)

        var maxPacket: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        if AudioConverterGetProperty(conv, kAudioConverterPropertyMaximumOutputPacketSize,
                                     &size, &maxPacket) != noErr || maxPacket == 0 {
            maxPacket = 768 * UInt32(channels) * 2
        }

        // The converter may adjust the output description; report what it uses.
        var actual = target
        size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        AudioConverterGetProperty(conv, kAudioConverterCurrentOutputStreamDescription, &size, &actual)
        let outRate = Int(actual.mSampleRate > 0 ? actual.mSampleRate : rate)
        let outChannels = Int(actual.mChannelsPerFrame > 0 ? actual.mChannelsPerFrame : UInt32(channels))

        converter = conv
        input = asbd
        outputCapacity = Int(maxPacket)
        output = UnsafeMutableRawPointer.allocate(byteCount: outputCapacity, alignment: 16)
        let buffers = Self.isNonInterleaved(asbd) ? Int(asbd.mChannelsPerFrame) : 1
        pending = Array(repeating: Data(), count: buffers)
        pendingFrames = 0
        let asc = Self.magicCookie(conv).flatMap(Self.audioSpecificConfig(fromCookie:))
            ?? Self.makeAudioSpecificConfig(sampleRate: outRate, channels: outChannels)
        format = Format(sampleRate: outRate, channels: outChannels, asc: asc)
        return true
    }

    private static func isNonInterleaved(_ asbd: AudioStreamBasicDescription) -> Bool {
        asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 && asbd.mChannelsPerFrame > 1
    }

    // MARK: - PCM in

    private func append(_ sample: CMSampleBuffer) -> Bool {
        let frames = CMSampleBufferGetNumSamples(sample)
        guard frames > 0 else { return false }
        var needed = 0
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: &needed, bufferListOut: nil, bufferListSize: 0,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0,
            blockBufferOut: nil) == noErr, needed > 0 else { return false }
        let raw = UnsafeMutableRawPointer.allocate(byteCount: needed, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { raw.deallocate() }
        let ablPtr = raw.bindMemory(to: AudioBufferList.self, capacity: 1)
        var block: CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: nil, bufferListOut: ablPtr, bufferListSize: needed,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil,
            flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
            blockBufferOut: &block) == noErr else { return false }
        let list = UnsafeMutableAudioBufferListPointer(ablPtr)
        guard list.count == pending.count else { return false }

        let bytes = frames * Int(input.mBytesPerFrame)
        for (i, buffer) in list.enumerated() {
            guard let data = buffer.mData, Int(buffer.mDataByteSize) >= bytes else { return false }
            pending[i].append(data.assumingMemoryBound(to: UInt8.self), count: bytes)
        }
        pendingFrames += frames
        // Should never trip (every call drains); a ceiling all the same.
        if pendingFrames > Int(input.mSampleRate) { reset() }
        return true
    }

    /// Input callback body: hands the converter up to `requested` frames.
    fileprivate func provide(_ requested: UInt32, into ioData: UnsafeMutablePointer<AudioBufferList>) -> (UInt32, OSStatus) {
        guard pendingFrames > 0 else { return (0, Self.needMoreInput) }
        let frames = min(Int(requested), pendingFrames)
        let bytes = frames * Int(input.mBytesPerFrame)
        if bytes > handedCapacity || handed.count != pending.count {
            handed.forEach { $0.deallocate() }
            handedCapacity = max(bytes, Self.framesPerPacket * Int(input.mBytesPerFrame))
            handed = pending.map { _ in UnsafeMutableRawPointer.allocate(byteCount: handedCapacity, alignment: 16) }
        }
        let list = UnsafeMutableAudioBufferListPointer(ioData)
        guard list.count == pending.count else { return (0, Self.needMoreInput) }
        for i in 0..<pending.count {
            pending[i].copyBytes(to: handed[i].assumingMemoryBound(to: UInt8.self), count: bytes)
            pending[i].removeFirst(bytes)
            list[i].mData = handed[i]
            list[i].mDataByteSize = UInt32(bytes)
            list[i].mNumberChannels = pending.count == 1 ? input.mChannelsPerFrame : 1
        }
        pendingFrames -= frames
        return (UInt32(frames), noErr)
    }

    // MARK: - AAC out

    private func drain() -> [Data] {
        guard let conv = converter, let out = output else { return [] }
        var packets: [Data] = []
        let me = Unmanaged.passUnretained(self).toOpaque()
        // One packet per call so each comes back with its own size; bounded
        // because a buffer never completes more than a handful.
        for _ in 0..<16 {
            var count: UInt32 = 1
            var desc = AudioStreamPacketDescription()
            var list = AudioBufferList(
                mNumberBuffers: 1,
                mBuffers: AudioBuffer(mNumberChannels: UInt32(format?.channels ?? 2),
                                      mDataByteSize: UInt32(outputCapacity), mData: out))
            let status = AudioConverterFillComplexBuffer(conv, aacInputProc, me, &count, &list, &desc)
            let size = Int(list.mBuffers.mDataByteSize)
            if count > 0, size > 0 {
                packets.append(Data(bytes: out, count: size))
            }
            if status == Self.needMoreInput || count == 0 { break }
            if status != noErr {
                // A broken converter: start over on the next buffer.
                invalidate()
                break
            }
        }
        return packets
    }

    // MARK: - AudioSpecificConfig

    private static func magicCookie(_ conv: AudioConverterRef) -> Data? {
        var size: UInt32 = 0
        guard AudioConverterGetPropertyInfo(conv, kAudioConverterCompressionMagicCookie, &size, nil) == noErr,
              size > 0 else { return nil }
        var bytes = [UInt8](repeating: 0, count: Int(size))
        guard AudioConverterGetProperty(conv, kAudioConverterCompressionMagicCookie, &size, &bytes) == noErr
        else { return nil }
        return Data(bytes.prefix(Int(size)))
    }

    /// The bare AudioSpecificConfig out of an encoder magic cookie. Apple's
    /// AAC cookie is an MPEG-4 ES_Descriptor (as inside an `esds` box, which
    /// may itself be present); the ASC is its DecoderSpecificInfo (tag 5).
    /// A cookie that already is a bare ASC is returned as is.
    static func audioSpecificConfig(fromCookie cookie: Data) -> Data? {
        var bytes = [UInt8](cookie)
        // Full `esds` box: size(4) 'esds' version+flags(4), then the descriptor.
        if let at = find([0x65, 0x73, 0x64, 0x73], in: bytes) {   // "esds"
            let start = at + 8
            guard start < bytes.count else { return nil }
            bytes = Array(bytes[start...])
        }
        if bytes.first == 0x03 {
            return esdsASC(bytes)
        }
        // Bare ASC: object type in the top 5 bits (1...31; 2 = AAC-LC), short.
        if (2...16).contains(bytes.count), (1...31).contains(bytes[0] >> 3) {
            return Data(bytes)
        }
        return nil
    }

    private static func find(_ needle: [UInt8], in hay: [UInt8]) -> Int? {
        guard hay.count >= needle.count else { return nil }
        for i in 0...(hay.count - needle.count) where Array(hay[i..<i + needle.count]) == needle { return i }
        return nil
    }

    /// Walks ES_Descriptor(3) → DecoderConfigDescriptor(4) → DecoderSpecificInfo(5).
    private static func esdsASC(_ b: [UInt8]) -> Data? {
        var i = 0
        func header(_ tag: UInt8) -> Int? {   // returns payload length, advances past the header
            guard i < b.count, b[i] == tag else { return nil }
            i += 1
            var len = 0
            for _ in 0..<4 {
                guard i < b.count else { return nil }
                let byte = b[i]
                i += 1
                len = len << 7 | Int(byte & 0x7f)
                if byte & 0x80 == 0 { break }
            }
            return len
        }
        guard header(0x03) != nil, i + 3 <= b.count else { return nil }
        i += 2                                  // ES_ID
        let flags = b[i]
        i += 1
        if flags & 0x80 != 0 { i += 2 }         // dependsOn_ES_ID
        if flags & 0x40 != 0 {                  // URL
            guard i < b.count else { return nil }
            i += 1 + Int(b[i])
        }
        if flags & 0x20 != 0 { i += 2 }         // OCR_ES_Id
        guard header(0x04) != nil else { return nil }
        i += 13                                 // objectType, streamType, bufferSize, max/avg bitrate
        guard let len = header(0x05), len > 0, i + len <= b.count else { return nil }
        return Data(b[i..<i + len])
    }

    /// AAC-LC ASC from first principles, if a cookie can't be read:
    /// 5 bits object type (2), 4 bits sampling-frequency index, 4 bits channels.
    static func makeAudioSpecificConfig(sampleRate: Int, channels: Int) -> Data {
        let rates = [96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350]
        let index = rates.firstIndex(of: sampleRate) ?? 3
        let value = UInt16(2) << 11 | UInt16(index) << 7 | UInt16(channels & 0xf) << 3
        return Data([UInt8(value >> 8), UInt8(value & 0xff)])
    }
}

/// AudioConverter's input callback: a plain C function, so the encoder comes
/// through `userData`.
private func aacInputProc(
    _ converter: AudioConverterRef,
    _ ioNumberDataPackets: UnsafeMutablePointer<UInt32>,
    _ ioData: UnsafeMutablePointer<AudioBufferList>,
    _ outPacketDescriptions: UnsafeMutablePointer<UnsafeMutablePointer<AudioStreamPacketDescription>?>?,
    _ userData: UnsafeMutableRawPointer?
) -> OSStatus {
    guard let userData = userData else { ioNumberDataPackets.pointee = 0; return -1 }
    let encoder = Unmanaged<AACEncoder>.fromOpaque(userData).takeUnretainedValue()
    let (frames, status) = encoder.provide(ioNumberDataPackets.pointee, into: ioData)
    ioNumberDataPackets.pointee = frames
    return status
}
