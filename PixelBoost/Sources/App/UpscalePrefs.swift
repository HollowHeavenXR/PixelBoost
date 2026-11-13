import SwiftUI

/// The upscale choices, persisted. Shared by the sheet, batch and cloud.
struct UpscaleChoice: DynamicProperty {
    @AppStorage("up.model") var model = "auto"
    @AppStorage("up.scale") var scale = 4.0
    @AppStorage("up.fidelity") var fidelity = "natural"
    @AppStorage("up.detail") var detail = 0.35
    @AppStorage("up.quality") var quality = 1
    @AppStorage("up.denoise") var denoise = 0.0
    @AppStorage("up.where") var whereTo = "device"
    /// Learned speed of this phone (GFLOP/s).
    @AppStorage("up.gflops") var gflops = 40.0
    /// Let the Rust analyzer choose every setting per photo.
    @AppStorage("up.smart") var smart = true
    /// Longest on-device run Smart may plan for (seconds, 0 = no limit).
    @AppStorage("up.maxWait") var maxWait = 180.0

    var settings: UpscaleSettings {
        UpscaleSettings(model: model, scale: Float(scale), fidelity: fidelity, detail: Float(detail),
                        denoise: Float(denoise), ensemble: UInt32(quality), tile: 128, maxOutputMp: 120)
    }

    /// Settings for the server worker.
    var cloudSettings: [String: Any] { Self.cloudSettings(settings) }

    static func cloudSettings(_ s: UpscaleSettings) -> [String: Any] {
        ["model": s.model, "scale": Double(s.scale), "fidelity": s.fidelity, "detail": Double(s.detail), "denoise": Double(s.denoise),
         "ensemble": Int(s.ensemble), "format": "png"]
    }

    /// Smart analysis of a photo for where it will run (cached per photo).
    func analysis(for url: URL, cloud: Bool) async -> PhotoAnalysis? {
        await SmartCache.shared.analysis(for: url, cloud: cloud, gflops: gflops, maxWait: maxWait)
    }

    /// Settings to run with: Smart's for this photo, else the manual ones.
    func resolved(for url: URL, cloud: Bool) async -> UpscaleSettings {
        if smart, let a = await analysis(for: url, cloud: cloud) { return a.settings }
        return settings
    }

    /// Copy Smart's choices into the manual controls (to tweak them).
    func adopt(_ a: PhotoAnalysis) {
        let s = a.settings
        model = s.model
        scale = Double(s.scale)
        fidelity = s.fidelity
        detail = Double(s.detail)
        denoise = Double(s.denoise)
        quality = Int(s.ensemble)
        smart = false
    }

    struct Option: Identifiable {
        let id: String
        let name: String
        let icon: String
    }

    static let models: [Option] = [
        Option(id: "auto", name: "Auto", icon: "wand.and.stars"),
        Option(id: "anime", name: "Anime & Art", icon: "paintpalette"),
        Option(id: "photo", name: "Photo & Render", icon: "camera"),
        Option(id: "clean", name: "Clean", icon: "face.smiling"),
        Option(id: "fast", name: "Fast", icon: "hare"),
        Option(id: "lineart", name: "Line Art", icon: "scribble.variable"),
        Option(id: "native2x", name: "Native 2×", icon: "2.square"),
    ]

    static func name(_ id: String) -> String { models.first { $0.id == id }?.name ?? id }
}

/// Smart analyses, one per (photo, where, budget). The analysis reads the
/// whole photo, so it runs once and off the main thread.
@MainActor
final class SmartCache {
    static let shared = SmartCache()
    private var done: [String: PhotoAnalysis] = [:]
    private var running: [String: Task<PhotoAnalysis?, Never>] = [:]

    func analysis(for url: URL, cloud: Bool, gflops: Double, maxWait: Double) async -> PhotoAnalysis? {
        let key = "\(url.path)|\(cloud)|\(Int(gflops / 5))|\(Int(maxWait))"
        if let a = done[key] { return a }
        if let t = running[key] { return await t.value }
        let path = url.path
        let budget = Float(maxWait <= 0 ? 1e9 : maxWait)
        let t = Task.detached(priority: .userInitiated) { () -> PhotoAnalysis? in
            try? Core.engine.analyze(inputPath: path, deviceGflops: Float(gflops), maxSeconds: budget, maxOutputMp: 120, cloud: cloud)
        }
        running[key] = t
        let a = await t.value
        running[key] = nil
        if let a {
            if done.count > 40 { done.removeAll() }
            done[key] = a
        }
        return a
    }
}

func formatDuration(_ s: Double) -> String {
    if s < 1 { return "<1s" }
    if s < 90 { return "\(Int(s.rounded()))s" }
    if s < 3600 { return "\(Int(s / 60))m \(Int(s.truncatingRemainder(dividingBy: 60)))s" }
    return "\(Int(s / 3600))h \(Int(s.truncatingRemainder(dividingBy: 3600) / 60))m"
}

func formatBytes(_ b: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .file)
}
