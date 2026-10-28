// Ported from iCTRL (https://github.com/Allternit/allternit-ictrl, iCTRLBroadcast/H264Encoder.swift).
// Copyright iCTRL authors. Licensed under the Apache License, Version 2.0;
// see http://www.apache.org/licenses/LICENSE-2.0. Unmodified apart from this notice.

import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

/// Hardware H.264 for the live screen: ReplayKit pixel buffers in, Annex-B
/// access units out.
///
/// Settings are the ones a WebCodecs decoder on the other end handles with no
/// reordering buffer: Main profile, no B-frames (AllowFrameReordering off), an
/// IDR at least every 2 s so a viewer that missed one recovers on its own, and
/// a rate cap so a busy screen can't outrun the socket. RealTime trades a
/// little quality for latency, which is the whole point of live video.
final class H264Encoder {
    /// One Annex-B access unit and whether it is an IDR. Called on
    /// VideoToolbox's own thread, in decode order.
    var onFrame: ((Data, Bool) -> Void)?
    /// The encoder produced nothing for a submitted frame (dropped or failed).
    /// The caller counts frames in flight, so it has to hear about these too.
    var onDrop: (() -> Void)?

    private var session: VTCompressionSession?
    private(set) var width = 0
    private(set) var height = 0

    private var transfer: VTPixelTransferSession?
    private var pool: CVPixelBufferPool?
    private var poolKey = (w: 0, h: 0, format: OSType(0))

    /// Long side the encoder works at. A 3x iPhone screen is ~2800 px tall;
    /// 1920 keeps it inside the hardware encoder's comfortable range and the
    /// 4 Mbit/s budget, and is still sharper than the phone frame it is drawn in.
    static let maxLongSide = 1920

    /// Size the pixel buffer is encoded at: as delivered, or scaled so the long
    /// side is `maxLongSide`. Even numbers — 4:2:0 can't encode an odd edge.
    static func targetSize(width: Int, height: Int) -> (width: Int, height: Int) {
        let long = max(width, height)
        guard long > maxLongSide else { return (width & ~1, height & ~1) }
        let f = Double(maxLongSide) / Double(long)
        return (Int(Double(width) * f) & ~1, Int(Double(height) * f) & ~1)
    }

    /// (Re)creates the session when the size changes (rotation, a different
    /// screen). Returns false when VideoToolbox refuses — the caller skips the
    /// frame and tries again on the next one.
    @discardableResult
    func prepare(width: Int, height: Int) -> Bool {
        if session != nil, width == self.width, height == self.height { return true }
        invalidate()
        var s: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height),
            codecType: kCMVideoCodecType_H264, encoderSpecification: nil,
            imageBufferAttributes: nil, compressedDataAllocator: nil,
            outputCallback: nil, refcon: nil, compressionSessionOut: &s)
        guard status == noErr, let session = s else { return false }
        let props: [CFString: Any] = [
            kVTCompressionPropertyKey_RealTime: kCFBooleanTrue as Any,
            kVTCompressionPropertyKey_ProfileLevel: kVTProfileLevel_H264_Main_AutoLevel,
            kVTCompressionPropertyKey_AllowFrameReordering: kCFBooleanFalse as Any,
            kVTCompressionPropertyKey_ExpectedFrameRate: 30 as NSNumber,
            kVTCompressionPropertyKey_MaxKeyFrameInterval: 60 as NSNumber,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: 2 as NSNumber,
            kVTCompressionPropertyKey_AverageBitRate: 4_000_000 as NSNumber,
            // [bytes, seconds]: a hard ceiling of ~1 MB in any one second.
            kVTCompressionPropertyKey_DataRateLimits: [1_000_000, 1] as [NSNumber] as CFArray,
        ]
        for (key, value) in props {
            VTSessionSetProperty(session, key: key, value: value as CFTypeRef)
        }
        VTCompressionSessionPrepareToEncodeFrames(session)
        self.session = session
        self.width = width
        self.height = height
        return true
    }

    /// Scales `source` to the session size. Returns `source` itself when it is
    /// already that size. Buffers come from a pool, so steady state allocates
    /// nothing — the extension has a 50 MB ceiling.
    func scaled(_ source: CVPixelBuffer) -> CVPixelBuffer? {
        let sw = CVPixelBufferGetWidth(source), sh = CVPixelBufferGetHeight(source)
        if sw == width, sh == height { return source }
        let format = CVPixelBufferGetPixelFormatType(source)
        if pool == nil || poolKey.w != width || poolKey.h != height || poolKey.format != format {
            let attrs: [CFString: Any] = [
                kCVPixelBufferPixelFormatTypeKey: format,
                kCVPixelBufferWidthKey: width,
                kCVPixelBufferHeightKey: height,
                kCVPixelBufferIOSurfacePropertiesKey: [:] as [String: Any],
            ]
            var p: CVPixelBufferPool?
            CVPixelBufferPoolCreate(nil, nil, attrs as CFDictionary, &p)
            pool = p
            poolKey = (width, height, format)
        }
        if transfer == nil {
            var t: VTPixelTransferSession?
            VTPixelTransferSessionCreate(allocator: nil, pixelTransferSessionOut: &t)
            transfer = t
        }
        guard let pool = pool, let transfer = transfer else { return nil }
        var out: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &out) == kCVReturnSuccess, let dest = out,
              VTPixelTransferSessionTransferImage(transfer, from: source, to: dest) == noErr else { return nil }
        return dest
    }

    /// Submits one frame. Returns false if it was not accepted (no output will
    /// follow for it).
    func encode(_ buffer: CVPixelBuffer, pts: CMTime, forceKeyframe: Bool) -> Bool {
        guard let session = session else { return false }
        let frameProps: CFDictionary? = forceKeyframe
            ? [kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanTrue] as CFDictionary : nil
        let status = VTCompressionSessionEncodeFrame(
            session, imageBuffer: buffer, presentationTimeStamp: pts, duration: .invalid,
            frameProperties: frameProps, infoFlagsOut: nil
        ) { [weak self] status, flags, sample in
            guard let self = self else { return }
            guard status == noErr, !flags.contains(.frameDropped), let sample = sample,
                  let (au, key) = Self.annexB(sample) else {
                self.onDrop?()
                return
            }
            self.onFrame?(au, key)
        }
        return status == noErr
    }

    func invalidate() {
        if let s = session {
            VTCompressionSessionCompleteFrames(s, untilPresentationTimeStamp: .invalid)
            VTCompressionSessionInvalidate(s)
        }
        session = nil
        width = 0
        height = 0
    }

    deinit {
        invalidate()
        if let t = transfer { VTPixelTransferSessionInvalidate(t) }
    }

    // MARK: - AVCC → Annex-B

    private static let startCode: [UInt8] = [0, 0, 0, 1]

    /// VideoToolbox hands out AVCC (length-prefixed NAL units, parameter sets
    /// in the format description). WebCodecs in Annex-B mode wants start codes
    /// and in-band SPS/PPS — so keyframes get both parameter sets in front, and
    /// a viewer can start decoding from any IDR without an out-of-band config.
    static func annexB(_ sample: CMSampleBuffer) -> (Data, Bool)? {
        guard let block = CMSampleBufferGetDataBuffer(sample) else { return nil }
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
            as? [[CFString: Any]]
        let notSync = attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool ?? false
        let isKey = !notSync

        var out = Data()
        var nalHeaderLength: Int32 = 4
        if let format = CMSampleBufferGetFormatDescription(sample) {
            var count = 0
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil,
                parameterSetCountOut: &count, nalUnitHeaderLengthOut: &nalHeaderLength)
            if isKey {
                for i in 0..<count {
                    var ptr: UnsafePointer<UInt8>?
                    var size = 0
                    if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        format, parameterSetIndex: i, parameterSetPointerOut: &ptr, parameterSetSizeOut: &size,
                        parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil) == noErr, let p = ptr {
                        out.append(contentsOf: startCode)
                        out.append(p, count: size)
                    }
                }
            }
        }

        // Copied out rather than read in place: a block buffer need not be
        // contiguous, and one access unit is small.
        let total = CMBlockBufferGetDataLength(block)
        var avcc = [UInt8](repeating: 0, count: total)
        guard CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: total, destination: &avcc) == noErr
        else { return nil }
        let hl = Int(nalHeaderLength)
        var i = 0
        while i + hl <= total {
            var len = 0
            for b in avcc[i..<i + hl] { len = len << 8 | Int(b) }
            i += hl
            guard len > 0, i + len <= total else { break }
            out.append(contentsOf: startCode)
            out.append(contentsOf: avcc[i..<i + len])
            i += len
        }
        return out.isEmpty ? nil : (out, isKey)
    }
}
