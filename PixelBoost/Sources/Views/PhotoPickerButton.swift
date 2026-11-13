import PhotosUI
import SwiftUI

struct PhotoPickerButton: View {
    @EnvironmentObject var app: AppModel
    var title = "Choose Photo"
    @State private var item: PhotosPickerItem?

    var body: some View {
        PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) {
            Label(title, systemImage: "photo.on.rectangle.angled")
                .fontWeight(.bold)
                .frame(maxWidth: 300)
                .padding(.vertical, 15)
                .background(Theme.energy, in: Capsule())
                .foregroundStyle(.black)
        }
        .onChange(of: item) { _, new in
            guard let new else { return }
            Task {
                do { app.setPhoto(try await PhotoIO.importItem(new), asset: new.itemIdentifier) } catch { app.message = describe(error) }
                item = nil
            }
        }
    }
}
