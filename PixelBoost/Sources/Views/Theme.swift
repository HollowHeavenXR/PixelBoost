import SwiftUI
import UniformTypeIdentifiers

/// "Studio" visual language: an ink-black canvas the photo sits on, one
/// amber→magenta energy gradient for primary actions, soft graphite cards.
enum Theme {
    static let ink = Color(red: 0.03, green: 0.03, blue: 0.04)
    static let card = Color(red: 0.09, green: 0.09, blue: 0.11)
    static let card2 = Color(red: 0.14, green: 0.14, blue: 0.17)
    static let line = Color.white.opacity(0.08)
    static let dim = Color.white.opacity(0.55)
    static let faint = Color.white.opacity(0.35)
    static let amber = Color(red: 1.0, green: 0.70, blue: 0.24)
    static let magenta = Color(red: 1.0, green: 0.30, blue: 0.55)
    static let accent = amber
    static let good = Color(red: 0.35, green: 0.85, blue: 0.55)
    static let energy = LinearGradient(colors: [amber, magenta], startPoint: .leading, endPoint: .trailing)
    // Old names used across views.
    static let canvas = ink
    static let panel = card
    static let panel2 = card2
}

struct Card<Content: View>: View {
    var title: String?
    var trailing: String?
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if title != nil || trailing != nil {
                HStack {
                    if let title { Text(title).font(.footnote.weight(.semibold)).foregroundStyle(Theme.dim) }
                    Spacer()
                    if let trailing { Text(trailing).font(.footnote.monospacedDigit()).foregroundStyle(Theme.faint) }
                }
            }
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).stroke(Theme.line))
    }
}

/// Kept for the edit tools.
struct Panel<Content: View>: View {
    var title: String?
    @ViewBuilder var content: Content
    var body: some View { Card(title: title) { content } }
}

struct SliderRow: View {
    let label: String
    @Binding var value: Double
    var range: ClosedRange<Double> = 0...1
    var format: (Double) -> String = { String(format: "%.2f", $0) }
    var onCommit: () -> Void = {}

    var body: some View {
        VStack(spacing: 2) {
            HStack {
                Text(label).font(.subheadline)
                Spacer()
                Text(format(value)).font(.subheadline.monospacedDigit()).foregroundStyle(Theme.dim)
            }
            Slider(value: $value, in: range, onEditingChanged: { editing in if !editing { onCommit() } })
                .tint(Theme.amber)
        }
    }
}

/// Segmented capsule picker.
struct ChipPicker<T: Hashable>: View {
    let options: [(T, String)]
    @Binding var selection: T
    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(options.enumerated()), id: \.offset) { _, opt in
                    let on = selection == opt.0
                    Button { selection = opt.0 } label: {
                        Text(opt.1)
                            .font(.subheadline.weight(on ? .semibold : .regular))
                            .padding(.horizontal, 14).padding(.vertical, 8)
                            .background(on ? AnyShapeStyle(Theme.energy) : AnyShapeStyle(Theme.card2), in: Capsule())
                            .foregroundStyle(on ? Color.black : Color.white)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

struct PrimaryButton: View {
    let title: String
    var icon: String?
    var disabled = false
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let icon { Image(systemName: icon) }
                Text(title).fontWeight(.bold)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(disabled ? AnyShapeStyle(Theme.card2) : AnyShapeStyle(Theme.energy), in: Capsule())
            .foregroundStyle(disabled ? Theme.dim : .black)
        }
        .disabled(disabled)
        .buttonStyle(.plain)
    }
}

struct CircleButton: View {
    let icon: String
    var badge: Int = 0
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 16, weight: .semibold))
                .frame(width: 42, height: 42)
                .background(Theme.card2, in: Circle())
                .overlay(alignment: .topTrailing) {
                    if badge > 0 {
                        Text("\(badge)").font(.caption2.bold()).padding(4).background(Theme.magenta, in: Circle()).offset(x: 4, y: -4)
                    }
                }
        }
        .buttonStyle(.plain)
    }
}

/// Shown when there's no photo yet.
struct EmptyPhotoState: View {
    @EnvironmentObject var app: AppModel
    var title = "Bring in a photo"
    @State private var showFiles = false

    var body: some View {
        VStack(spacing: 20) {
            ZStack {
                Circle().fill(Theme.energy).frame(width: 110, height: 110).blur(radius: 40).opacity(0.6)
                Image(systemName: "sparkles").font(.system(size: 54, weight: .light)).foregroundStyle(Theme.energy)
            }
            Text(title).font(.title.weight(.bold))
            Text("Upscale on this iPhone, or send it to PixelBoost Cloud and close the app — the full-resolution result waits for you.")
                .font(.subheadline).foregroundStyle(Theme.dim).multilineTextAlignment(.center).padding(.horizontal, 12)
            PhotoPickerButton()
            HStack(spacing: 12) {
                Button { app.pasteFromClipboard() } label: { Label("Paste", systemImage: "doc.on.clipboard") }
                Button { showFiles = true } label: { Label("Files", systemImage: "folder") }
            }
            .buttonStyle(.bordered).tint(.white)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.image]) { r in
            if case .success(let url) = r { app.importFile(url) }
        }
    }
}
