import Foundation
import ScreenCaptureKit
import CoreMedia
import CoreVideo
import AppKit

enum CaptureError: Error, CustomStringConvertible {
    case noWindow
    case noFirstFrame

    var description: String {
        switch self {
        case .noWindow: return "no on-screen window for the target process"
        case .noFirstFrame: return "ScreenCaptureKit delivered no frame"
        }
    }
}

/// A ScreenCaptureKit stream on one region of the target's window. It answers one question:
/// when did the pixels in that region first differ from what they were when the watcher was armed.
/// Frames are stamped by the capture pipeline on the mach clock, the same clock as `Clock.nowNs()`,
/// so "event to photon" is a plain subtraction.
final class FrameWatcher: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "qhbench.capture", qos: .userInteractive)

    private var last: [UInt32]?
    private var baseline: [UInt32]?
    private var armedAt: UInt64 = 0
    private var hit: UInt64?
    private var frameTimes: [UInt64] = []
    private var recording = false
    private let hitSem = DispatchSemaphore(value: 0)
    private var gotFirst = false
    private var collect = false
    private var collected: [(pts: UInt64, sample: [UInt32])] = []

    /// A frame counts as "changed" when at least this many sampled pixels differ.
    var minChangedPixels = 4

    static func start(pid: pid_t, region: CGRect, fps: Int = 120) async throws -> FrameWatcher {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let candidates = content.windows.filter { $0.owningApplication?.processID == pid && $0.windowLayer == 0 && $0.frame.width > 100 }
        guard let window = candidates.max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height })
        else { throw CaptureError.noWindow }

        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let local = CGRect(x: region.minX - window.frame.minX, y: region.minY - window.frame.minY,
                           width: region.width, height: region.height)
            .intersection(CGRect(origin: .zero, size: window.frame.size))
        let cfg = SCStreamConfiguration()
        cfg.sourceRect = local
        cfg.width = max(8, Int(local.width * scale))
        cfg.height = max(8, Int(local.height * scale))
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        cfg.queueDepth = 8
        cfg.showsCursor = false

        let w = FrameWatcher()
        let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: window), configuration: cfg, delegate: w)
        try s.addStreamOutput(w, type: .screen, sampleHandlerQueue: w.queue)
        try await s.startCapture()
        w.stream = s
        for _ in 0..<200 where !w.hasFrame {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        if !w.hasFrame {
            try? await s.stopCapture()
            throw CaptureError.noFirstFrame
        }
        return w
    }

    var hasFrame: Bool { lock.lock(); defer { lock.unlock() }; return gotFirst }

    func stop() async { try? await stream?.stopCapture() }

    /// Remember the current pixels as "before" and start waiting for a different frame.
    func arm(collect: Bool = false) {
        lock.lock()
        self.collect = collect
        collected = []
        baseline = last
        armedAt = Clock.nowNs()
        hit = nil
        lock.unlock()
        while hitSem.wait(timeout: .now()) == .success {}
    }

    /// The capture timestamp (mach ns) of the first changed frame, or nil on timeout.
    func waitForChange(timeoutMs: Double) -> UInt64? {
        guard hitSem.wait(timeout: .now() + timeoutMs / 1000) == .success else { return nil }
        lock.lock(); defer { lock.unlock() }
        return hit
    }

    /// Frames seen since `arm(collect: true)`; stops collecting.
    func collectedFrames() -> [(pts: UInt64, sample: [UInt32])] {
        lock.lock(); defer { lock.unlock() }
        collect = false
        return collected
    }

    func startRecording() {
        lock.lock(); frameTimes = []; recording = true; lock.unlock()
    }

    func stopRecording() -> [UInt64] {
        lock.lock(); defer { lock.unlock() }
        recording = false
        return frameTimes
    }

    // MARK: SCStreamOutput

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sb.isValid else { return }
        // Idle frames carry no pixels; only `complete` frames are content.
        if let arr = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = arr.first?[.status] as? Int, raw != SCFrameStatus.complete.rawValue {
            return
        }
        guard let pb = sb.imageBuffer else { return }
        let pts = UInt64(max(0, CMTimeGetSeconds(sb.presentationTimeStamp)) * 1e9)
        let sample = FrameWatcher.sample(pb)

        lock.lock()
        if recording { frameTimes.append(pts) }
        if collect, armedAt != 0, pts >= armedAt, collected.count < 2000 { collected.append((pts, sample)) }
        if let base = baseline, armedAt != 0, hit == nil, pts >= armedAt,
           FrameWatcher.differing(sample, base) >= minChangedPixels {
            hit = pts
            hitSem.signal()
        }
        last = sample
        gotFirst = true
        lock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        logErr("qhbench: capture stopped: \(error.localizedDescription)")
    }

    // MARK: Pixel sampling

    /// At most about 40k sampled pixels per frame, so 120 fps stays cheap on the capture queue.
    private static func sample(_ pb: CVPixelBuffer) -> [UInt32] {
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return [] }
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        let bpr = CVPixelBufferGetBytesPerRow(pb)
        let step = max(1, Int((Double(w * h) / 40_000).squareRoot()))
        var out: [UInt32] = []
        out.reserveCapacity((w / step + 1) * (h / step + 1))
        var y = 0
        while y < h {
            let row = base.advanced(by: y * bpr).assumingMemoryBound(to: UInt32.self)
            var x = 0
            while x < w {
                out.append(row[x])
                x += step
            }
            y += step
        }
        return out
    }

    static func differing(_ a: [UInt32], _ b: [UInt32]) -> Int {
        guard a.count == b.count else { return Int.max }
        var n = 0
        for i in 0..<a.count where a[i] != b[i] {
            // Ignore one-step colour noise so a dithered or blended edge is not a "change".
            let d0 = abs(Int(a[i] & 0xFF) - Int(b[i] & 0xFF))
            let d1 = abs(Int((a[i] >> 8) & 0xFF) - Int((b[i] >> 8) & 0xFF))
            let d2 = abs(Int((a[i] >> 16) & 0xFF) - Int((b[i] >> 16) & 0xFF))
            if d0 + d1 + d2 > 6 { n += 1 }
        }
        return n
    }
}
