import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers
import Vision

/// Subject segmentation from the system (Vision's on-device subject lift).
/// Produces a greyscale mask file; compositing happens in Rust.
enum SubjectMask {
    /// Mask of all subjects, or only the instance under `point` (normalised,
    /// top-left origin) when given.
    static func make(for imageURL: URL, at point: CGPoint? = nil) throws -> URL {
        let handler = VNImageRequestHandler(url: imageURL)
        let req = VNGenerateForegroundInstanceMaskRequest()
        try handler.perform([req])
        guard let obs = req.results?.first else { throw MaskError.noSubject }
        var instances = obs.allInstances
        if let p = point {
            let buf = obs.instanceMask
            CVPixelBufferLockBaseAddress(buf, .readOnly)
            defer { CVPixelBufferUnlockBaseAddress(buf, .readOnly) }
            let w = CVPixelBufferGetWidth(buf), h = CVPixelBufferGetHeight(buf)
            let x = min(w - 1, max(0, Int(p.x * CGFloat(w)))), y = min(h - 1, max(0, Int(p.y * CGFloat(h))))
            let row = CVPixelBufferGetBaseAddress(buf)!.advanced(by: y * CVPixelBufferGetBytesPerRow(buf))
            let label = Int(row.assumingMemoryBound(to: UInt8.self)[x])
            guard label != 0 else { throw MaskError.noSubject }
            instances = IndexSet(integer: label)
        }
        let mask = try obs.generateScaledMaskForImage(forInstances: instances, from: handler)
        let ci = CIImage(cvPixelBuffer: mask)
        let ctx = CIContext()
        guard let cg = ctx.createCGImage(ci, from: ci.extent) else { throw MaskError.noSubject }
        let url = Core.newFile("png")
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { throw MaskError.noSubject }
        CGImageDestinationAddImage(dest, cg, nil)
        CGImageDestinationFinalize(dest)
        return url
    }

    enum MaskError: LocalizedError {
        case noSubject
        var errorDescription: String? { "No subject found there." }
    }
}
