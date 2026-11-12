import Foundation
import UIKit

/// The Rust engine (PixelBoostCore) plus the app's folders. Everything that
/// touches pixels goes through `Core.engine`.
enum Core {
    static let engine: PixelBoostCore = {
        let dir = Bundle.main.resourceURL!.appendingPathComponent("pbnn").path
        return PixelBoostCore(modelsDir: dir)
    }()

    static let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    static let work: URL = {
        let u = FileManager.default.temporaryDirectory.appendingPathComponent("work", isDirectory: true)
        try? FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }()

    static let history = HistoryStore(dir: documents.appendingPathComponent("History", isDirectory: true).path)

    /// Built-in and saved presets (Rust-managed JSON in Documents/Presets).
    static let presets = PresetStore(dir: documents.appendingPathComponent("Presets", isDirectory: true).path)

    static let deviceID: String = {
        let k = "pb.deviceID"
        if let v = UserDefaults.standard.string(forKey: k) { return v }
        let v = UUID().uuidString
        UserDefaults.standard.set(v, forKey: k)
        return v
    }()

    static let server: ServerClient = {
        let c = ServerClient(baseUrl: Prefs.serverURL, apiKey: Prefs.apiKey, deviceId: deviceID)
        return c
    }()

    /// A fresh file in the work folder.
    static func newFile(_ ext: String = "png") -> URL {
        work.appendingPathComponent(UUID().uuidString).appendingPathExtension(ext)
    }

    static var appVersion: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
    }
}

/// User preferences (UI state only).
enum Prefs {
    static let defaultServer = "https://upscaler-bridge.xenusanimations.studio"
    static var serverURL: String {
        let v = UserDefaults.standard.string(forKey: "pb.server") ?? defaultServer
        return v
    }
    static var apiKey: String {
        if let v = UserDefaults.standard.string(forKey: "pb.apiKey"), !v.isEmpty { return v }
        return Bundle.main.infoDictionary?["UpscalerBridgeAPIKey"] as? String ?? ""
    }
    static var telemetry: Bool {
        UserDefaults.standard.object(forKey: "pb.telemetry") as? Bool ?? true
    }
}

extension RgbaImage {
    /// Straight-alpha RGBA8 -> UIImage (no pixel work beyond wrapping).
    var uiImage: UIImage? {
        let w = Int(width), h = Int(height)
        guard w > 0, h > 0, rgba.count == w * h * 4,
              let provider = CGDataProvider(data: rgba as CFData),
              let cg = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                               space: CGColorSpace(name: CGColorSpace.sRGB)!,
                               bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                               provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return nil }
        return UIImage(cgImage: cg)
    }
}

extension PbError {
    var text: String {
        switch self {
        case .Failed(let message): return message
        case .Cancelled: return "Cancelled"
        }
    }
}

func describe(_ error: Error) -> String {
    if let e = error as? PbError { return e.text }
    return error.localizedDescription
}

/// Encode an editing operation for the Rust op dispatcher.
func opJSON(_ op: String, _ fields: [String: Any] = [:]) -> String {
    var d = fields
    d["op"] = op
    let data = (try? JSONSerialization.data(withJSONObject: d)) ?? Data("{}".utf8)
    return String(decoding: data, as: UTF8.self)
}

func opDict(_ op: String, _ fields: [String: Any] = [:]) -> [String: Any] {
    var d = fields
    d["op"] = op
    return d
}
