import SwiftUI
import UniformTypeIdentifiers

/// Visual language: a dark "lightbox" — near-black canvas, one warm accent,
/// flat panels (no live blur, cheap on battery).
enum Theme {
    static let accent = Color(red: 1.0, green: 0.62, blue: 0.24)
    static let canvas = Color(red: 0.05, green: 0.05, blue: 0.06)
    static let panel = Color(red: 0.11, green: 0.11, blue: 0.13)
    static let panel2 = Color(red: 0.16, green: 0.16, blue: 0.19)
    static let dim = Color.white.opacity(0.55)
}

struct Panel<Content: View>: View {
    var title: String?
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let title {
                Text(title.uppercased()).font(.caption2.weight(.semibold)).tracking(1.2).foregroundStyle(Theme.dim)
            }
            content
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.panel, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

struct SliderRow: View {
    let label: String
    @Binding var value: Double
    var range: ClosedRange<Double> = 0...1
    var format: (Double) -> String = { String(format: "%.2f", $0) }
    var onCommit: () -> Void = {}

    var body: some View {
        VStack(spacing: 4) {
            HStack {
                Text(label).font(.subheadline)
                Spacer()
                Text(format(value)).font(.subheadline.monospacedDigit()).foregroundStyle(Theme.dim)
            }
            Slider(value: $value, in: range, onEditingChanged: { editing in if !editing { onCommit() } })
        }
    }
}

struct ChipPicker<T: Hashable>: View {
    let options: [(T, String)]
    @Binding var selection: T
    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(options.enumerated()), id: \.offset) { _, opt in
                    Button { selection = opt.0 } label: {
                        Text(opt.1)
                            .font(.subheadline.weight(.medium))
                            .padding(.horizontal, 14).padding(.vertical, 8)
                            .background(selection == opt.0 ? Theme.accent : Theme.panel2, in: Capsule())
                            .foregroundStyle(selection == opt.0 ? Color.black : Color.white)
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
            HStack {
                if let icon { Image(systemName: icon) }
                Text(title).fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 14)
            .background(disabled ? Theme.panel2 : Theme.accent, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .foregroundStyle(disabled ? Theme.dim : .black)
        }
        .disabled(disabled)
        .buttonStyle(.plain)
    }
}

/// Shown on any tab when there's no photo yet.
struct EmptyPhotoState: View {
    @EnvironmentObject var app: AppModel
    var title = "Bring in a photo"
    @State private var showFiles = false

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "sparkles.rectangle.stack").font(.system(size: 52)).foregroundStyle(Theme.accent)
            Text(title).font(.title2.weight(.semibold))
            Text("Every pixel runs through PixelBoost's own on-device AI engine.")
                .font(.subheadline).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
            PhotoPickerButton()
            HStack(spacing: 12) {
                Button { app.pasteFromClipboard() } label: { Label("Paste", systemImage: "doc.on.clipboard") }
                Button { showFiles = true } label: { Label("Files", systemImage: "folder") }
            }
            .buttonStyle(.bordered)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.image]) { r in
            if case .success(let url) = r { app.importFile(url) }
        }
    }
}
