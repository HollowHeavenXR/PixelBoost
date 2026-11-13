import SwiftUI

struct SettingsScreen: View {
    @EnvironmentObject var app: AppModel
    @AppStorage("pb.server") private var server = Prefs.defaultServer
    @AppStorage("pb.apiKey") private var apiKey = ""
    @AppStorage("pb.telemetry") private var telemetry = true
    @AppStorage("pb.replaceMethod") private var replaceMethod = PhotoLibrary.Method.edit.rawValue
    @State private var health: String?
    @State private var exports: [StoredImage] = []
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Engine") {
                    LabeledContent("Inference", value: "PixelBoost NN (Rust)")
                    LabeledContent("Kernel", value: Core.engine.kernel())
                    ForEach(app.models, id: \.id) { m in
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(m.name)  ·  \(m.scale)×").font(.subheadline)
                            Text(m.bestFor).font(.caption).foregroundStyle(Theme.dim)
                        }
                    }
                }
                Section {
                    Picker("Replace method", selection: $replaceMethod) {
                        ForEach(PhotoLibrary.Method.allCases) { m in Text(m.title).tag(m.rawValue) }
                    }
                } header: { Text("Replacing originals in Photos") } footer: {
                    Text("Edit in place keeps the photo where it is (albums, favourite, date, place) and Photos can revert it. New photo + delete adds the result with the original's date, place and albums, then iOS asks before moving the original to Recently Deleted. Either way the original's camera details and colour profile are carried over.")
                }
                Section {
                    TextField("Server URL", text: $server).textInputAutocapitalization(.never).autocorrectionDisabled()
                    SecureField("API key (optional)", text: $apiKey)
                    Toggle("Send anonymous diagnostics", isOn: $telemetry)
                    Button("Test connection") { test() }
                    if let health { Text(health).font(.caption).foregroundStyle(Theme.dim) }
                } header: { Text("Server") } footer: {
                    Text("PixelBoost Cloud upscales run on the PixelBoost server through this address; results are kept 7 days. On-device upscales never leave the phone.")
                }
                Section("Temporary uploads (24h)") {
                    Button("Refresh") { refreshExports() }
                    ForEach(exports, id: \.id) { e in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(e.label.isEmpty ? e.filename : e.label).font(.subheadline)
                                Text("expires \(e.expiresAt)").font(.caption2).foregroundStyle(Theme.dim)
                            }
                            Spacer()
                            Button { download(e) } label: { Image(systemName: "arrow.down.circle") }
                        }
                        .swipeActions {
                            Button("Delete", role: .destructive) { try? Core.server.deleteExport(id: e.id); refreshExports() }
                        }
                    }
                }
                Section("About") {
                    LabeledContent("Version", value: Core.appVersion)
                    Text("Upscaling, fidelity refinement and every editing tool run in PixelBoost's own Rust engine. Models: Real-ESRGAN (BSD-3-Clause), converted to PixelBoost's .pbnn format.")
                        .font(.caption).foregroundStyle(Theme.dim)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .navigationTitle("Settings")
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            .onChange(of: server) { _, _ in Core.server.configure(baseUrl: server, apiKey: Prefs.apiKey) }
            .onChange(of: apiKey) { _, _ in Core.server.configure(baseUrl: server, apiKey: Prefs.apiKey) }
        }
    }

    private func test() {
        health = "Checking…"
        Task.detached {
            let r = Result { try Core.server.health() }
            await MainActor.run {
                switch r {
                case .success(let v): health = "Connected · server \(v)"
                case .failure(let e): health = describe(e)
                }
            }
        }
    }

    private func refreshExports() {
        Task.detached {
            let r = Result { try Core.server.listExports() }
            await MainActor.run {
                switch r {
                case .success(let v): exports = v
                case .failure(let e): app.message = describe(e)
                }
            }
        }
    }

    private func download(_ e: StoredImage) {
        Task.detached {
            let dest = Core.newFile(URL(fileURLWithPath: e.filename).pathExtension.isEmpty ? "png" : URL(fileURLWithPath: e.filename).pathExtension)
            let r = Result { try Core.server.downloadExport(id: e.id, destPath: dest.path) }
            await MainActor.run {
                switch r {
                case .success: app.setPhoto(dest); app.mode = .upscale; dismiss()
                case .failure(let err): app.message = describe(err)
                }
            }
        }
    }
}
