import BackgroundTasks
import SwiftUI
import UIKit
import UserNotifications

/// Server-side upscale jobs: send, watch, download. Rust does the network
/// work (ServerClient); this keeps the list and drives the UI.
@MainActor
final class CloudStore: ObservableObject {
    static let shared = CloudStore()

    @Published private(set) var jobs: [CloudJob] = []
    @Published private(set) var reachable: Bool?
    @Published private(set) var sending: [String] = []
    /// Download progress by job id (0...1).
    @Published private(set) var downloading: [String: Double] = [:]
    @Published private(set) var thumbs: [String: UIImage] = [:]

    private var pollTask: Task<Void, Never>?
    static let refreshID = "com.pixelboost.cloudcheck"

    var readyCount: Int { jobs.filter { $0.status == "done" && localResult($0) == nil }.count }
    var activeCount: Int { jobs.filter { $0.status == "queued" || $0.status == "running" }.count }

    static let folder: URL = {
        let u = Core.documents.appendingPathComponent("Cloud", isDirectory: true)
        try? FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }()

    func localResult(_ j: CloudJob) -> URL? {
        for ext in ["png", "jpg"] {
            let u = Self.folder.appendingPathComponent("\(j.id).\(ext)")
            if FileManager.default.fileExists(atPath: u.path) { return u }
        }
        return nil
    }

    // MARK: refresh

    func refresh() async {
        let r = await Task.detached { Result { try Core.server.listJobs() } }.value
        switch r {
        case .success(let list):
            jobs = list
            reachable = true
            Self.remember(done: list.filter { $0.status == "done" }.map(\.id))
            for j in list where thumbs[j.id] == nil { loadThumb(j) }
        case .failure:
            reachable = false
        }
    }

    /// Poll while something is in flight and the app is open.
    func startPolling() {
        guard pollTask == nil else { return }
        pollTask = Task {
            while !Task.isCancelled {
                await refresh()
                let busy = activeCount > 0
                try? await Task.sleep(nanoseconds: busy ? 3_000_000_000 : 20_000_000_000)
            }
        }
    }

    func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
    }

    private func loadThumb(_ j: CloudJob) {
        let id = j.id, status = j.status
        let cache = FileManager.default.temporaryDirectory.appendingPathComponent("cloud-\(id)-\(status == "done" ? "r" : "i").jpg")
        if let img = UIImage(contentsOfFile: cache.path) { thumbs[id] = img; return }
        Task.detached {
            guard (try? Core.server.downloadJobThumb(id: id, destPath: cache.path)) != nil, let img = UIImage(contentsOfFile: cache.path) else { return }
            await MainActor.run { self.thumbs[id] = img }
        }
    }

    // MARK: actions

    /// Upload a photo; keeps running briefly if the app is closed mid-upload.
    /// Library asset each job's photo came from (job id -> local identifier).
    private static let assetsKey = "cloud.assets"

    func sourceAsset(_ j: CloudJob) -> String? {
        (UserDefaults.standard.dictionary(forKey: Self.assetsKey) as? [String: String])?[j.id]
    }

    private static func remember(asset: String, job: String) {
        var d = (UserDefaults.standard.dictionary(forKey: assetsKey) as? [String: String]) ?? [:]
        if d.count > 300 { d.removeAll() }
        d[job] = asset
        UserDefaults.standard.set(d, forKey: assetsKey)
    }

    func send(_ file: URL, settings: [String: Any], label: String, app: AppModel, asset: String? = nil) {
        let key = UUID().uuidString
        sending.append(key)
        let json = String(decoding: (try? JSONSerialization.data(withJSONObject: settings)) ?? Data("{}".utf8), as: UTF8.self)
        var bg = UIBackgroundTaskIdentifier.invalid
        bg = UIApplication.shared.beginBackgroundTask(withName: "cloud-upload") { UIApplication.shared.endBackgroundTask(bg) }
        Self.askForNotifications()
        Task {
            let r = await Task.detached { Result { try Core.server.submitJob(path: file.path, settingsJson: json, label: label) } }.value
            sending.removeAll { $0 == key }
            switch r {
            case .success(let job):
                if let asset { Self.remember(asset: asset, job: job.id) }
                jobs.insert(job, at: 0)
                app.flash("Sent to PixelBoost Cloud — you can close the app")
                app.log("cloud_send", ["w": job.inWidth, "h": job.inHeight])
                startPolling()
                Self.scheduleRefresh()
            case .failure(let e):
                app.message = "Couldn't send: \(describe(e))"
            }
            UIApplication.shared.endBackgroundTask(bg)
        }
    }

    func download(_ j: CloudJob, app: AppModel) {
        guard downloading[j.id] == nil else { return }
        downloading[j.id] = 0
        let id = j.id
        let dest = Self.folder.appendingPathComponent("\(id).png")
        let listener = DownloadProgress { [weak self] got, total in
            Task { @MainActor in self?.downloading[id] = total > 0 ? Double(got) / Double(total) : 0 }
        }
        var bg = UIBackgroundTaskIdentifier.invalid
        bg = UIApplication.shared.beginBackgroundTask(withName: "cloud-download") { UIApplication.shared.endBackgroundTask(bg) }
        Task {
            let r = await Task.detached { Result { try Core.server.downloadJob(id: id, destPath: dest.path, listener: listener) } }.value
            downloading[id] = nil
            if case .failure(let e) = r { app.message = describe(e) }
            objectWillChange.send()
            UIApplication.shared.endBackgroundTask(bg)
        }
    }

    func delete(_ j: CloudJob) {
        jobs.removeAll { $0.id == j.id }
        if let u = localResult(j) { try? FileManager.default.removeItem(at: u) }
        let id = j.id
        Task.detached { try? Core.server.deleteJob(id: id) }
    }

    // MARK: background check -> local notification

    private static let seenKey = "cloud.seenDone"

    /// Records finished jobs; returns the ones not seen before.
    @discardableResult
    nonisolated static func remember(done: [String]) -> [String] {
        let seen = Set(UserDefaults.standard.stringArray(forKey: seenKey) ?? [])
        let new = done.filter { !seen.contains($0) }
        UserDefaults.standard.set(Array(Array(seen.union(done)).suffix(300)), forKey: seenKey)
        return new
    }

    nonisolated static func askForNotifications() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    nonisolated static func register() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: refreshID, using: nil) { task in
            handle(task as! BGAppRefreshTask)
        }
    }

    nonisolated static func scheduleRefresh() {
        let req = BGAppRefreshTaskRequest(identifier: refreshID)
        req.earliestBeginDate = Date(timeIntervalSinceNow: 10 * 60)
        try? BGTaskScheduler.shared.submit(req)
    }

    nonisolated private static func handle(_ task: BGAppRefreshTask) {
        let work = Task.detached {
            guard let jobs = try? Core.server.listJobs() else { return }
            let seen = Set(UserDefaults.standard.stringArray(forKey: seenKey) ?? [])
            let fresh = jobs.filter { $0.status == "done" && !seen.contains($0.id) }
            remember(done: jobs.filter { $0.status == "done" }.map(\.id))
            for j in fresh {
                let c = UNMutableNotificationContent()
                c.title = "Upscale ready"
                c.body = "\(j.label.isEmpty ? j.filename : j.label) — \(j.outWidth)×\(j.outHeight) is waiting in PixelBoost Cloud."
                c.sound = .default
                try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: j.id, content: c, trigger: nil))
            }
            if jobs.contains(where: { $0.status == "queued" || $0.status == "running" }) { scheduleRefresh() }
        }
        task.expirationHandler = { work.cancel() }
        Task { await work.value; task.setTaskCompleted(success: true) }
    }
}

final class DownloadProgress: DownloadListener, @unchecked Sendable {
    private let f: (UInt64, UInt64) -> Void
    init(_ f: @escaping (UInt64, UInt64) -> Void) { self.f = f }
    func onProgress(received: UInt64, total: UInt64) { f(received, total) }
}
