import SwiftUI

/// Export: format, quality and size, then save to Photos or share. The
/// encoding and resizing happen in Rust.
struct ExportSheet: View {
    @EnvironmentObject var app: AppModel
    @Environment(\.dismiss) private var dismiss
    @AppStorage("pb.export.format") private var format = "jpg"
    @AppStorage("pb.export.quality") private var quality = 92.0
    @AppStorage("pb.export.size") private var size = 0
    @State private var exported: URL?
    @State private var exportedInfo: ImageInfo?
    @State private var working = false

    private static let sizes: [(Int, String)] = [(0, "Original"), (4096, "4096"), (2048, "2048"), (1080, "1080")]

    var body: some View {
        NavigationStack {
            Form {
                Section("Format") {
                    Picker("Format", selection: $format) {
                        Text("JPEG").tag("jpg")
                        Text("PNG").tag("png")
                        Text("WebP").tag("webp")
                        Text("TIFF").tag("tiff")
                    }
                    .pickerStyle(.segmented)
                    if format == "jpg" {
                        SliderRow(label: "Quality", value: $quality, range: 50...100, format: { "\(Int($0))" })
                    } else {
                        Text(format == "png" ? "Lossless, keeps transparency." : format == "webp" ? "Lossless WebP, keeps transparency." : "16-bit TIFF for further editing.")
                            .font(.caption).foregroundStyle(Theme.dim)
                    }
                }
                Section("Size") {
                    Picker("Long side", selection: $size) {
                        ForEach(Self.sizes.indices, id: \.self) { i in Text(Self.sizes[i].1).tag(Self.sizes[i].0) }
                    }
                    .pickerStyle(.segmented)
                    if let i = app.info {
                        Text(outputLabel(i)).font(.caption.monospacedDigit()).foregroundStyle(Theme.dim)
                    }
                }
                Section {
                    Button { export(thenSave: true) } label: {
                        Label("Save to Photos", systemImage: "square.and.arrow.down")
                    }
                    .disabled(working || format == "webp")
                    if let e = exported {
                        ShareLink(item: e) { Label("Share \(e.pathExtension.uppercased())", systemImage: "square.and.arrow.up") }
                    } else {
                        Button { export(thenSave: false) } label: { Label("Prepare to share", systemImage: "square.and.arrow.up") }
                            .disabled(working)
                    }
                    if working { ProgressView() }
                } footer: {
                    if format == "webp" { Text("Photos can't store WebP — share it to Files instead.") }
                }
            }
            .scrollContentBackground(.hidden)
            .navigationTitle("Export")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .onChange(of: format) { _, _ in exported = nil }
        .onChange(of: quality) { _, _ in exported = nil }
        .onChange(of: size) { _, _ in exported = nil }
    }

    private func outputLabel(_ i: ImageInfo) -> String {
        let long = max(i.width, i.height)
        guard size > 0, long > UInt32(size) else { return "\(i.width) × \(i.height)" }
        let f = Double(size) / Double(long)
        return "\(Int((Double(i.width) * f).rounded())) × \(Int((Double(i.height) * f).rounded()))"
    }

    private func export(thenSave: Bool) {
        guard let src = app.current else { return }
        working = true
        let out = Core.work.appendingPathComponent("PixelBoost-\(Int(Date().timeIntervalSince1970)).\(format)")
        let q = UInt8(quality), maxSide = UInt32(size)
        Task.detached(priority: .userInitiated) {
            let r = Result { try Core.engine.exportFile(inputPath: src.path, outputPath: out.path, quality: q, maxSide: maxSide) }
            await MainActor.run {
                working = false
                switch r {
                case .success(let info):
                    exported = out
                    exportedInfo = info
                    app.log("export", ["format": format, "max_side": size])
                    if thenSave {
                        Task {
                            do { try await PhotoIO.saveToPhotos(out); app.flash("Saved to Photos") } catch { app.message = describe(error) }
                        }
                    }
                case .failure(let e):
                    app.message = describe(e)
                }
            }
        }
    }
}

/// Every edit made to this photo; tap one to go back to it.
struct HistorySheet: View {
    @EnvironmentObject var app: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    row(title: "Original", index: 0, icon: "photo")
                    ForEach(Array(app.steps.enumerated()), id: \.element.id) { i, s in
                        row(title: s.title, index: i + 1, icon: "checkmark.circle.fill")
                    }
                    ForEach(Array(app.undone.enumerated()), id: \.element.id) { i, s in
                        row(title: s.title, index: app.index + i + 1, icon: "circle.dashed").opacity(0.5)
                    }
                } footer: {
                    Text("Tap a step to go back to it. Making a new edit after going back drops the steps after it.")
                }
                Section {
                    Button { app.copyEdits() } label: { Label("Copy Edits", systemImage: "doc.on.doc") }
                    Button {
                        if let r = app.copiedEdits { app.run(r, step: "Pasted edits"); dismiss() }
                    } label: { Label("Paste Edits", systemImage: "doc.on.clipboard") }
                    .disabled(app.copiedEdits == nil)
                } footer: {
                    Text("Copied edits stay available for your next photo. Painted masks, layers and LUT files aren't copied.")
                }
            }
            .scrollContentBackground(.hidden)
            .navigationTitle("History")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }

    private func row(title: String, index: Int, icon: String) -> some View {
        Button { app.jump(to: index) } label: {
            HStack {
                Image(systemName: icon).foregroundStyle(index == app.index ? Theme.amber : Theme.dim)
                Text(title)
                Spacer()
                if index == app.index { Text("Showing").font(.caption).foregroundStyle(Theme.amber) }
            }
        }
        .foregroundStyle(.white)
    }
}
