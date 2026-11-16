import AVFoundation
import CoreMedia
import SwiftUI
import UIKit

/// One H.264 access unit in Annex B (what an iCTRL FRAME packet carries),
/// split into the parts a decoder needs: SPS/PPS for the format description
/// and the slices, re-framed with 4-byte lengths (AVCC) for the sample buffer.
struct AnnexBAccessUnit: Equatable {
    var sps: Data?
    var pps: Data?
    /// Slice and SEI NAL units (AUD and parameter sets are dropped).
    var slices: [Data]
    var isKeyframe: Bool

    /// NAL units between start codes (`00 00 01` or `00 00 00 01`).
    static func nalUnits(in data: Data) -> [Data] {
        let bytes = [UInt8](data)
        var starts: [Int] = []
        var i = 0
        while i + 2 < bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                starts.append(i + 3)
                i += 3
            } else {
                i += 1
            }
        }
        var units: [Data] = []
        for (k, start) in starts.enumerated() {
            let hasNext = k + 1 < starts.count
            var end = hasNext ? starts[k + 1] - 3 : bytes.count
            // The zero that makes a 4-byte start code belongs to the next one.
            while hasNext, end > start, bytes[end - 1] == 0 { end -= 1 }
            if end > start { units.append(Data(bytes[start..<end])) }
        }
        return units
    }

    static func parse(_ data: Data) -> AnnexBAccessUnit {
        var unit = AnnexBAccessUnit(sps: nil, pps: nil, slices: [], isKeyframe: false)
        for nal in nalUnits(in: data) {
            guard let first = nal.first else { continue }
            switch first & 0x1f {
            case 7: unit.sps = nal
            case 8: unit.pps = nal
            case 9: break
            case 5:
                unit.isKeyframe = true
                unit.slices.append(nal)
            default: unit.slices.append(nal)
            }
        }
        return unit
    }

    /// The slices as one AVCC buffer: u32 big-endian length, then the NAL.
    var avcc: Data {
        var out = Data()
        for nal in slices {
            let n = UInt32(nal.count)
            out.append(contentsOf: [UInt8(n >> 24 & 0xff), UInt8(n >> 16 & 0xff), UInt8(n >> 8 & 0xff), UInt8(n & 0xff)])
            out.append(nal)
        }
        return out
    }
}

/// What the layer does with one packet: the decision, kept apart from the
/// layer so it can be tested.
enum LiveLayerAction: Equatable {
    /// New CONFIG: drop the decoder state and wait for a keyframe with SPS/PPS.
    case reset(width: Int, height: Int)
    case enqueue(AnnexBAccessUnit)
    case ignore

    /// `hasFormat` = the layer already has a format description.
    static func decide(for packet: LivePacket, hasFormat: Bool) -> LiveLayerAction {
        switch packet {
        case .config(let width, let height, _):
            return width > 0 && height > 0 ? .reset(width: width, height: height) : .ignore
        case .frame(let data):
            let unit = AnnexBAccessUnit.parse(data)
            if unit.slices.isEmpty { return .ignore }
            // Without a format only a keyframe that carries SPS and PPS can start the picture.
            if !hasFormat && !(unit.isKeyframe && unit.sps != nil && unit.pps != nil) { return .ignore }
            return .enqueue(unit)
        default:
            return .ignore
        }
    }
}

/// An AVSampleBufferDisplayLayer fed with Annex B access units. The format
/// description is rebuilt on every new CONFIG (the phone resized or rotated)
/// and whenever a keyframe brings different SPS/PPS.
final class AnnexBDisplayView: UIView {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }

    private var displayLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }
    private var format: CMVideoFormatDescription?
    private var lastSPS: Data?
    private var lastPPS: Data?

    override init(frame: CGRect) {
        super.init(frame: frame)
        displayLayer.videoGravity = .resizeAspect
        backgroundColor = .black
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func apply(_ action: LiveLayerAction) {
        switch action {
        case .reset:
            format = nil
            lastSPS = nil
            lastPPS = nil
            displayLayer.flushAndRemoveImage()
        case .enqueue(let unit):
            enqueue(unit)
        case .ignore:
            break
        }
    }

    var hasFormat: Bool { format != nil }

    private func enqueue(_ unit: AnnexBAccessUnit) {
        if let sps = unit.sps, let pps = unit.pps, sps != lastSPS || pps != lastPPS {
            if let made = Self.makeFormat(sps: sps, pps: pps) {
                format = made
                lastSPS = sps
                lastPPS = pps
            }
        }
        guard let format else { return }
        if displayLayer.status == .failed { displayLayer.flush() }
        guard let buffer = Self.makeSample(avcc: unit.avcc, format: format) else { return }
        displayLayer.enqueue(buffer)
    }

    static func makeFormat(sps: Data, pps: Data) -> CMVideoFormatDescription? {
        var out: CMVideoFormatDescription?
        let status = sps.withUnsafeBytes { (s: UnsafeRawBufferPointer) -> OSStatus in
            pps.withUnsafeBytes { (p: UnsafeRawBufferPointer) -> OSStatus in
                guard let sb = s.bindMemory(to: UInt8.self).baseAddress,
                      let pb = p.bindMemory(to: UInt8.self).baseAddress else { return -1 }
                let pointers: [UnsafePointer<UInt8>] = [sb, pb]
                let sizes = [s.count, p.count]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault,
                    parameterSetCount: 2,
                    parameterSetPointers: pointers,
                    parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4,
                    formatDescriptionOut: &out
                )
            }
        }
        return status == noErr ? out : nil
    }

    static func makeSample(avcc: Data, format: CMVideoFormatDescription) -> CMSampleBuffer? {
        guard !avcc.isEmpty else { return nil }
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: avcc.count,
            blockAllocator: kCFAllocatorDefault, customBlockSource: nil,
            offsetToData: 0, dataLength: avcc.count, flags: 0, blockBufferOut: &block
        ) == kCMBlockBufferNoErr, let block else { return nil }
        let copied = avcc.withUnsafeBytes { raw -> OSStatus in
            guard let base = raw.baseAddress else { return -1 }
            return CMBlockBufferReplaceDataBytes(with: base, blockBuffer: block, offsetIntoDestination: 0, dataLength: avcc.count)
        }
        guard copied == kCMBlockBufferNoErr else { return nil }
        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )
        var size = avcc.count
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
            sampleCount: 1, sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample
        ) == noErr, let sample else { return nil }
        // A live picture: show each frame as it arrives.
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(
                dict,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            )
        }
        return sample
    }
}

/// SwiftUI host for the display view. `onMake` hands the UIKit view to the
/// session, which feeds it.
struct AnnexBDisplayLayer: UIViewRepresentable {
    let onMake: (AnnexBDisplayView) -> Void

    func makeUIView(context: Context) -> AnnexBDisplayView {
        let view = AnnexBDisplayView()
        view.isAccessibilityElement = false
        onMake(view)
        return view
    }

    func updateUIView(_ uiView: AnnexBDisplayView, context: Context) {}
}
