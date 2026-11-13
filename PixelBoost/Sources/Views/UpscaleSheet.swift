import SwiftUI

/// Everything about an upscale, top to bottom in the order people decide:
/// where it runs, how big, which look, how careful.
struct UpscaleSheet: View {
    @EnvironmentObject var app: AppModel
    @ObservedObject var cloud = CloudStore.shared
    let onDevice: () -> Void
    let onCloud: () -> Void
    let choice = UpscaleChoice()
    @State private var autoPick: String?
    @State private var analysis: PhotoAnalysis?
    @State private var analyzing = false
    @State private var showWhy = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Upscale").font(.title.weight(.bold))
                    Spacer()
                    Button { dismiss() } label: { Image(systemName: "xmark").font(.headline).foregroundStyle(Theme.dim) }
                }
                whereCards
                smartCard
                if !choice.smart { manualCards }
                PrimaryButton(title: choice.whereTo == "cloud" ? "Send to PixelBoost Cloud" : "Upscale on this iPhone",
                              icon: choice.whereTo == "cloud" ? "icloud.and.arrow.up" : "wand.and.stars") {
                    choice.whereTo == "cloud" ? onCloud() : onDevice()
                }
                .padding(.top, 4)
            }
            .padding(18)
        }
        .task {
            if let url = app.current {
                autoPick = await Task.detached { try? Core.engine.autoModel(inputPath: url.path) }.value
            }
        }
        .task(id: "\(choice.whereTo)|\(choice.maxWait)|\(app.current?.path ?? "")") {
            guard let url = app.current else { return }
            analyzing = true
            analysis = await choice.analysis(for: url, cloud: choice.whereTo == "cloud")
            analyzing = false
        }
    }

    // MARK: smart

    private var smartCard: some View {
        Card {
            HStack(spacing: 10) {
                Image(systemName: "sparkles").font(.title3).foregroundStyle(Theme.energy)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Smart settings").font(.subheadline.weight(.bold))
                    Text(choice.smart ? "Reads this photo and picks everything" : "Off — using your settings below")
                        .font(.caption).foregroundStyle(Theme.dim)
                }
                Spacer()
                Toggle("", isOn: choice.$smart).labelsHidden().tint(Theme.amber)
            }
            if choice.smart {
                if let a = analysis {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(a.summary).font(.headline)
                        Text(smartLine(a)).font(.caption.monospacedDigit()).foregroundStyle(Theme.dim)
                        HStack(spacing: 6) {
                            meter("Noise", min(Double(a.noise) / 12, 1))
                            meter("Blocks", Double(a.blocking))
                            meter("Sharp", min(max((Double(a.sharpness) - 0.4) / 0.45, 0), 1))
                        }
                        Button { withAnimation(.snappy) { showWhy.toggle() } } label: {
                            Label(showWhy ? "Hide why" : "Why these settings", systemImage: showWhy ? "chevron.up" : "chevron.down")
                                .font(.caption.weight(.semibold))
                        }
                        .buttonStyle(.plain).foregroundStyle(Theme.amber)
                        if showWhy {
                            VStack(alignment: .leading, spacing: 4) {
                                ForEach(a.reasons, id: \.self) { r in
                                    HStack(alignment: .top, spacing: 6) {
                                        Text("•").foregroundStyle(Theme.amber)
                                        Text(r).font(.caption)
                                    }
                                }
                            }
                        }
                        if choice.whereTo != "cloud" {
                            Text("Longest wait on this iPhone").font(.caption).foregroundStyle(Theme.dim).padding(.top, 2)
                            ChipPicker(options: [(60.0, "1 min"), (180.0, "3 min"), (600.0, "10 min"), (0.0, "No limit")], selection: choice.$maxWait)
                        }
                        Button("Customize these settings") { choice.adopt(a) }
                            .font(.caption.weight(.semibold)).foregroundStyle(Theme.dim).padding(.top, 2)
                    }
                } else if analyzing {
                    HStack(spacing: 8) { ProgressView(); Text("Looking at the photo…").font(.caption).foregroundStyle(Theme.dim) }
                } else {
                    Text("Pick a photo first.").font(.caption).foregroundStyle(Theme.dim)
                }
            }
        }
    }

    private func smartLine(_ a: PhotoAnalysis) -> String {
        let s = a.settings
        let w = Int(Float(a.width) * s.scale), h = Int(Float(a.height) * s.scale)
        var parts = ["\(Int(s.scale))× → \(w) × \(h)", UpscaleChoice.name(s.model), s.fidelity.capitalized, "detail \(Int(s.detail * 100))%"]
        if s.denoise > 0 { parts.append("denoise \(Int(s.denoise * 100))%") }
        if s.ensemble > 1 { parts.append(s.ensemble >= 8 ? "Max" : "High") }
        if choice.whereTo != "cloud", a.estSeconds > 0 { parts.append("≈ \(formatDuration(Double(a.estSeconds)))") }
        return parts.joined(separator: " · ")
    }

    private func meter(_ label: String, _ v: Double) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label).font(.caption2).foregroundStyle(Theme.dim)
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.card2)
                    Capsule().fill(Theme.energy).frame(width: max(4, g.size.width * v))
                }
            }
            .frame(height: 5)
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: manual

    @ViewBuilder
    private var manualCards: some View {
                Card(title: "Size", trailing: outputText) {
                    ChipPicker(options: [(2.0, "2×"), (3.0, "3×"), (4.0, "4×"), (6.0, "6×"), (8.0, "8×")], selection: choice.$scale)
                }
                Card(title: "Look") {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8)], spacing: 8) {
                        ForEach(UpscaleChoice.models) { m in
                            modelTile(m)
                        }
                    }
                    Text(modelBlurb).font(.caption).foregroundStyle(Theme.dim)
                }
                Card(title: "Quality") {
                    ChipPicker(options: [(1, "Standard"), (2, "High · 2 passes"), (8, "Max · 8 passes")], selection: choice.$quality)
                }
                Card(title: "Clarity") {
                    ChipPicker(options: [("off", "Raw AI"), ("natural", "Natural"), ("faithful", "Faithful")], selection: choice.$fidelity)
                    Text(fidelityBlurb).font(.caption).foregroundStyle(Theme.dim)
                    SliderRow(label: "Detail", value: choice.$detail)
                    if choice.model == "fast" { SliderRow(label: "Denoise", value: choice.$denoise) }
                }
    }

    // MARK: where

    private var whereCards: some View {
        HStack(spacing: 10) {
            whereCard("device", "iphone", "This iPhone", deviceEstimate.map { "≈ \(formatDuration($0))" } ?? "", "Stays on your phone")
            whereCard("cloud", "cloud.fill", "PixelBoost Cloud", cloudEstimate.map { "≈ \(formatDuration($0))" } ?? "",
                      cloud.reachable == false ? "Offline right now" : "Close the app — it keeps going")
        }
    }

    private func whereCard(_ id: String, _ icon: String, _ title: String, _ eta: String, _ note: String) -> some View {
        let on = choice.whereTo == id
        return Button { choice.whereTo = id } label: {
            VStack(alignment: .leading, spacing: 6) {
                Image(systemName: icon).font(.title2).foregroundStyle(on ? AnyShapeStyle(Theme.energy) : AnyShapeStyle(Theme.dim))
                Text(title).font(.subheadline.weight(.bold))
                Text(eta).font(.caption.monospacedDigit()).foregroundStyle(on ? .white : Theme.dim)
                Text(note).font(.caption2).foregroundStyle(Theme.dim).lineLimit(2)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(on ? AnyShapeStyle(Theme.energy) : AnyShapeStyle(Theme.line), lineWidth: on ? 2 : 1))
        }
        .buttonStyle(.plain)
    }

    private func modelTile(_ m: UpscaleChoice.Option) -> some View {
        let on = choice.model == m.id
        return Button { choice.model = m.id } label: {
            VStack(spacing: 6) {
                Image(systemName: m.icon).font(.title3)
                Text(m.name).font(.caption.weight(.medium)).lineLimit(1).minimumScaleFactor(0.8)
            }
            .frame(maxWidth: .infinity, minHeight: 66)
            .background(on ? AnyShapeStyle(Theme.amber.opacity(0.18)) : AnyShapeStyle(Theme.card2), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(on ? Theme.amber : .clear, lineWidth: 1.5))
        }
        .buttonStyle(.plain)
    }

    // MARK: text + estimates

    private var effectiveModel: String { choice.model == "auto" ? (autoPick ?? "photo") : choice.model }

    private var outputText: String? {
        guard let i = app.info else { return nil }
        let w = Int(Double(i.width) * choice.scale), h = Int(Double(i.height) * choice.scale)
        let mp = Double(w) * Double(h) / 1e6
        return "\(w) × \(h) · \(String(format: mp >= 10 ? "%.0f" : "%.1f", mp)) MP"
    }

    private func gflop(_ model: String) -> Double? {
        guard let i = app.info else { return nil }
        let p = Core.engine.plan(width: i.width, height: i.height, model: model, scale: Float(choice.scale), tile: 128)
        let twoPass = choice.scale > 6 ? 17.0 : 1.0
        return p.gflop * Double(max(choice.quality, 1)) * twoPass
    }

    private var deviceEstimate: Double? {
        if choice.smart, choice.whereTo != "cloud", let a = analysis { return Double(a.estSeconds) }
        if choice.smart, let a = analysis, let i = app.info {
            let s = a.settings
            let p = Core.engine.plan(width: i.width, height: i.height, model: s.model, scale: s.scale, tile: 128)
            return p.gflop * Double(max(s.ensemble, 1)) * (s.scale > 6 ? 17 : 1) / max(choice.gflops, 1)
        }
        return gflop(effectiveModel).map { $0 / max(choice.gflops, 1) }
    }

    /// The server runs the full-strength model for the content at ~150 GFLOP/s.
    private var cloudEstimate: Double? {
        if choice.smart, let a = analysis, let i = app.info {
            let s = a.settings
            let p = Core.engine.plan(width: i.width, height: i.height, model: s.model, scale: s.scale, tile: 128)
            return p.gflop * Double(max(s.ensemble, 1)) * (s.scale > 6 ? 17 : 1) / 150 + 8
        }
        let m = choice.model == "auto" ? (effectiveModel == "anime" || effectiveModel == "lineart" ? "anime" : "photo") : choice.model
        return gflop(m).map { $0 / 150 + 8 }
    }

    private var modelBlurb: String {
        switch choice.model {
        case "auto": return "Picks per photo — this one looks like \(UpscaleChoice.name(effectiveModel)). The cloud always uses the full-strength model."
        case "anime": return "Drawn art, anime, cel shading: crisp lines, flat colour stays flat."
        case "photo": return "Photos, game screenshots and 3D renders: rebuilds real texture."
        case "clean": return "No invented texture — best for faces."
        case "fast": return "Quick everyday upscale with a denoise slider."
        case "lineart": return "Anime video frames, line art, compressed screenshots. Very fast."
        default: return "Real 2× network for photos."
        }
    }

    private var fidelityBlurb: String {
        switch choice.fidelity {
        case "off": return "The network's output as-is."
        case "faithful": return "Every output pixel pulled to agree with your photo as closely as possible."
        default: return "Keeps the AI's sharp edges, restores colour and shading it flattened, removes halos."
        }
    }
}
