import SwiftUI

@main
struct PixelBoostApp: App {
    @StateObject private var app = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(app)
                .preferredColorScheme(.dark)
                .tint(Theme.accent)
                .onOpenURL { url in app.importFile(url) }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { app.pickUpSharedPhoto() }
        }
    }
}
