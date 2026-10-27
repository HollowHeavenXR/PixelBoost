import Accelerate
import CoreGraphics
import Foundation
import UIKit

/// How strictly an upscale is held to the original photo's pixels.
///
/// Super-resolution GANs don't just add detail — on soft or compressed
/// sources (3D renders, screenshots, re-saved JPEGs) they also *repaint*:
/// fine texture becomes smooth painterly patches, and edges pick up dark
/// or bright outlines (overshoot) that were never in the photo. Both are
/// measurable, pixel by pixel, against the original:
///
/// - **Detail Lock** (back-projection): shrink the result back to the
///   source size, subtract it from the source, scale that residual up and
///   add it back. Whatever the model smeared away or invented at the
///   source's own scale is pulled back to the real pixels, while the
///   model's genuinely new high-frequency detail is left alone.
/// - **Halo Guard** (anti-ringing clamp): every output pixel may not be
///   darker or brighter than the 3×3 neighborhood of source pixels it came
///   from (plus a small tolerance). That's exactly the dark/bright outline
///   artifact, removed without blurring anything.
enum UpscaleFidelity: String, CaseIterable, Identifiable {
    case off
    case natural
    case faithful

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .off: return "Off"
        case .natural: return "Natural"
        case .faithful: return "Faithful"
        }
    }

    var footnote: String {
        switch self {
        case .off: return "The model's raw output, artifacts and all."
        case .natural: return "Restores smeared texture and removes edge halos while keeping the model's detail."
        case .faithful: return "Holds the result tightly to the original's pixels — best for renders, art and text."
        }
    }

    /// Back-projection step size, 0...1. `.natural` was 0.55 — strong enough
    /// to correct for a sizable fraction of whatever high-frequency detail
    /// the model added that didn't match a plain downsample of the source,
    /// which in practice undid a visible chunk of the model's own
    /// sharpening along with the artifacts it was meant to catch. Lowered
    /// so Natural still pulls back genuine smearing/invention without
    /// quietly flattening legitimate detail — `.faithful` keeps the full
    /// correction since that's the point of picking it (renders/art/text).
    var detailLock: Double {
        switch self {
        case .off: return 0
        case .natural: return 0.1
        case .faithful: return 1.0
        }
    }

    /// (Natural is now 0.1: Mel's upscale_history showed fidelity-checked
    /// runs averaging ~50 dB PSNR against the source, i.e. the result shrunk
    /// back was nearly identical to the original — little added detail.)
    ///
    /// Halo Guard tolerance in 0-255 levels; nil disables it. `.natural` was
    /// 10 — tight enough to clamp away not just ringing but the edge
    /// contrast that reads as "sharp" to begin with, since this can't tell
    /// the two apart. Raised to let more of that contrast through while
    /// `.faithful` keeps the tight clamp for content where any halo at all
    /// is worse than losing some sharpness.
    var haloTolerance: Int? {
        switch self {
        case .off: return nil
        case .natural: return 40
        case .faithful: return 4
        }
    }
}

enum FidelityRefiner {
    /// Refines `canvas` (opaque BGRA, the stitched model output) in place
    /// against `source` (BGRA at source size, same color space). Returns
    /// the result's fidelity — PSNR in dB between the original and the
    /// result shrunk back to the original's size — or nil if it couldn't
    /// be measured.
    static func refine(
        canvas: UnsafeMutableRawPointer, width: Int, height: Int, bytesPerRow: Int,
        source: UnsafeMutableRawPointer, sourceWidth: Int, sourceHeight: Int, sourceBytesPerRow: Int,
        fidelity: UpscaleFidelity
    ) -> Double? {
        guard width > sourceWidth || height > sourceHeight else { return nil }
        var canvasBuffer = vImage_Buffer(data: canvas, height: vImagePixelCount(height), width: vImagePixelCount(width), rowBytes: bytesPerRow)
        var sourceBuffer = vImage_Buffer(data: source, height: vImagePixelCount(sourceHeight), width: vImagePixelCount(sourceWidth), rowBytes: sourceBytesPerRow)

        let smallBytes = sourceWidth * 4
        guard let down = malloc(smallBytes * sourceHeight) else { return nil }
        defer { free(down) }
        var downBuffer = vImage_Buffer(data: down, height: vImagePixelCount(sourceHeight), width: vImagePixelCount(sourceWidth), rowBytes: smallBytes)

        if fidelity.detailLock > 0, let residual = malloc(smallBytes * sourceHeight), let up = malloc(width * 4 * height) {
            defer { free(residual); free(up) }
            var residualBuffer = vImage_Buffer(data: residual, height: vImagePixelCount(sourceHeight), width: vImagePixelCount(sourceWidth), rowBytes: smallBytes)
            var upBuffer = vImage_Buffer(data: up, height: vImagePixelCount(height), width: vImagePixelCount(width), rowBytes: width * 4)
            let step = Int(fidelity.detailLock * 256)
            // Two passes converge most of the way; more cost time for
            // little visible change.
            for _ in 0..<2 {
                guard scale(&canvasBuffer, into: &downBuffer) else { break }
                for y in 0..<sourceHeight {
                    let s = source.advanced(by: y * sourceBytesPerRow).assumingMemoryBound(to: UInt8.self)
                    let d = down.advanced(by: y * smallBytes).assumingMemoryBound(to: UInt8.self)
                    let r = residual.advanced(by: y * smallBytes).assumingMemoryBound(to: UInt8.self)
                    for i in 0..<sourceWidth * 4 {
                        // Offset-128 encoding so a signed residual survives
                        // an 8-bit resample; alpha (every 4th byte) is inert.
                        r[i] = i & 3 == 3 ? 128 : UInt8(clamping: 128 + Int(s[i]) - Int(d[i]))
                    }
                }
                guard scale(&residualBuffer, into: &upBuffer) else { break }
                for y in 0..<height {
                    let c = canvas.advanced(by: y * bytesPerRow).assumingMemoryBound(to: UInt8.self)
                    let u = up.advanced(by: y * width * 4).assumingMemoryBound(to: UInt8.self)
                    for i in 0..<width * 4 where i & 3 != 3 {
                        let delta = ((Int(u[i]) - 128) * step) >> 8
                        if delta != 0 { c[i] = UInt8(clamping: Int(c[i]) + delta) }
                    }
                }
            }
        }

        if let tolerance = fidelity.haloTolerance {
            haloGuard(
                canvas: canvas, width: width, height: height, bytesPerRow: bytesPerRow,
                source: &sourceBuffer, tolerance: tolerance
            )
        }

        // Fidelity score: how closely the result, shrunk back down,
        // reproduces the original.
        guard scale(&canvasBuffer, into: &downBuffer) else { return nil }
        var squared = 0.0
        for y in 0..<sourceHeight {
            let s = source.advanced(by: y * sourceBytesPerRow).assumingMemoryBound(to: UInt8.self)
            let d = down.advanced(by: y * smallBytes).assumingMemoryBound(to: UInt8.self)
            var rowSum = 0
            for i in 0..<sourceWidth * 4 where i & 3 != 3 {
                let e = Int(s[i]) - Int(d[i])
                rowSum += e * e
            }
            squared += Double(rowSum)
        }
        let mse = squared / Double(sourceWidth * sourceHeight * 3)
        return mse <= 0 ? 99 : 10 * log10(255 * 255 / mse)
    }

    /// Lanczos (vImage high-quality) resample between two BGRA buffers.
    private static func scale(_ from: inout vImage_Buffer, into to: inout vImage_Buffer) -> Bool {
        vImageScale_ARGB8888(&from, &to, nil, vImage_Flags(kvImageHighQualityResampling)) == kvImageNoError
    }

    /// Clamps every output pixel to [min, max] of the 3×3 source pixels it
    /// maps onto, ± `tolerance`. Needs only two source-sized planes; the
    /// output is walked once, pixel by pixel.
    private static func haloGuard(
        canvas: UnsafeMutableRawPointer, width: Int, height: Int, bytesPerRow: Int,
        source: inout vImage_Buffer, tolerance: Int
    ) {
        let sourceWidth = Int(source.width), sourceHeight = Int(source.height)
        let rowBytes = sourceWidth * 4
        guard let minData = malloc(rowBytes * sourceHeight), let maxData = malloc(rowBytes * sourceHeight) else { return }
        defer { free(minData); free(maxData) }
        var minBuffer = vImage_Buffer(data: minData, height: source.height, width: source.width, rowBytes: rowBytes)
        var maxBuffer = vImage_Buffer(data: maxData, height: source.height, width: source.width, rowBytes: rowBytes)
        guard vImageMin_ARGB8888(&source, &minBuffer, nil, 0, 0, 3, 3, vImage_Flags(kvImageNoFlags)) == kvImageNoError,
              vImageMax_ARGB8888(&source, &maxBuffer, nil, 0, 0, 3, 3, vImage_Flags(kvImageNoFlags)) == kvImageNoError
        else { return }

        // Output column -> source byte offset, computed once.
        let columnOffset = (0..<width).map { min(sourceWidth - 1, $0 * sourceWidth / width) * 4 }
        for y in 0..<height {
            let sy = min(sourceHeight - 1, y * sourceHeight / height)
            let lo = minData.advanced(by: sy * rowBytes).assumingMemoryBound(to: UInt8.self)
            let hi = maxData.advanced(by: sy * rowBytes).assumingMemoryBound(to: UInt8.self)
            let c = canvas.advanced(by: y * bytesPerRow).assumingMemoryBound(to: UInt8.self)
            for x in 0..<width {
                let s = columnOffset[x]
                let p = x * 4
                for channel in 0..<3 {
                    let value = Int(c[p + channel])
                    let floor = Int(lo[s + channel]) - tolerance
                    let ceiling = Int(hi[s + channel]) + tolerance
                    if value < floor {
                        c[p + channel] = UInt8(clamping: floor)
                    } else if value > ceiling {
                        c[p + channel] = UInt8(clamping: ceiling)
                    }
                }
            }
        }
    }
}

/// Estimates a photo's noise level so render denoise only runs on renders
/// that actually need it. On a clean (often JPEG-compressed) render, the
/// denoiser has nothing to remove but texture — one of the ways an
/// upscale came out smeared.
enum NoiseEstimator {
    /// Gaussian noise sigma in 0-255 levels (Immerkær's fast estimator)
    /// over a ≤512px center crop of the luma channel.
    static func sigma(of image: UIImage) -> Double? {
        guard let cg = image.cgImage else { return nil }
        let side = min(512, cg.width, cg.height)
        guard side >= 16,
              let crop = cg.cropping(to: CGRect(x: (cg.width - side) / 2, y: (cg.height - side) / 2, width: side, height: side)),
              let context = CGContext(data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: side,
                                      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue),
              let data = context.data
        else { return nil }
        context.draw(crop, in: CGRect(x: 0, y: 0, width: side, height: side))
        let p = data.assumingMemoryBound(to: UInt8.self)
        let rowBytes = context.bytesPerRow
        var sum = 0
        for y in 1..<(side - 1) {
            for x in 1..<(side - 1) {
                func v(_ dx: Int, _ dy: Int) -> Int { Int(p[(y + dy) * rowBytes + x + dx]) }
                let response = v(-1, -1) - 2 * v(0, -1) + v(1, -1)
                    - 2 * v(-1, 0) + 4 * v(0, 0) - 2 * v(1, 0)
                    + v(-1, 1) - 2 * v(0, 1) + v(1, 1)
                sum += abs(response)
            }
        }
        let interior = Double((side - 2) * (side - 2))
        return Double(sum) * sqrt(0.5 * Double.pi) / (6 * interior)
    }

    /// Above this, a render is noisy enough that denoising first helps
    /// more than it smears.
    static let renderDenoiseThreshold = 3.0
}
