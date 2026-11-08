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

    var settings: UpscaleSettings {
        UpscaleSettings(model: model, scale: Float(scale), fidelity: fidelity, detail: Float(detail),
                        denoise: Float(denoise), ensemble: UInt32(quality), tile: 128, maxOutputMp: 120)
    }

    /// Settings for the server worker.
    var cloudSettings: [String: Any] {
        ["model": model, "scale": scale, "fidelity": fidelity, "detail": detail, "denoise": denoise, "ensemble": quality, "format": "png"]
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

func formatDuration(_ s: Double) -> String {
    if s < 1 { return "<1s" }
    if s < 90 { return "\(Int(s.rounded()))s" }
    if s < 3600 { return "\(Int(s / 60))m \(Int(s.truncatingRemainder(dividingBy: 60)))s" }
    return "\(Int(s / 3600))h \(Int(s.truncatingRemainder(dividingBy: 3600) / 60))m"
}

func formatBytes(_ b: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .file)
}
