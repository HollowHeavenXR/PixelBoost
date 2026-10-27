import PhotosUI
import SwiftUI
import UIKit

/// One bundled model's full-photo result from `UpscalerViewModel.compareModels()`.
///
/// The full-resolution pixels live on disk (see `ComparisonResultStore`),
/// not here: holding every candidate's finished image in memory at once was
/// costing ~86MB per candidate and driving a sweep into memory warnings and
/// thermal throttling. `preview` is the display-sized copy the grid and the
/// zoom view read; `fileURL` is read back only for the one that gets picked.
struct ModelComparisonResult: Identifiable {
    let id = UUID()
    let choice: UpscaleModelChoice
    /// Display-sized (longest side 2048), the only copy kept resident.
    let preview: UIImage
    /// Where the full-resolution result was spilled. `nil` if the write
    /// failed, in which case `preview` holds the full image and is used
    /// as-is — the old in-memory behavior, as a fallback rather than the
    /// default.
    let fileURL: URL?
    let sharpnessScore: Double
}

/// What the last finished upscale did — shown under the result.
struct UpscaleRunSummary {
    let modelName: String
    let seconds: TimeInterval
    let tiles: Int?
    let outputSize: CGSize
    let outputWasCapped: Bool
    let fidelityPSNR: Double?
    let tilesReused: Int
}

/// One model's result on a small crop — see `UpscalerViewModel.previewPatch`.
struct PatchPreviewResult: Identifiable {
    let id = UUID()
    let choice: UpscaleModelChoice?
    let title: String
    let image: UIImage
    let fidelityPSNR: Double?
}

@MainActor
final class UpscalerViewModel: ObservableObject {
    @Published var sourceImage: UIImage? {
        didSet {
            imageVersion += 1
            refreshDisplayCopy(of: sourceImage, into: \.displaySource)
        }
    }
    /// Every editing tab (Filters, Adjust, Crop, Cutout, ...) writes its
    /// "Apply" result straight to this property, so it's the one place
    /// that sees every image change regardless of which tool produced it —
    /// the natural hook for Auto Cloud Backup and for the undo history.
    /// `upscale()`/`pickComparisonResult()` set `skipNextAutoCloudBackup`
    /// first: `UpscaleRunner.log` already uploaded those.
    @Published var resultImage: UIImage? {
        didSet {
            imageVersion += 1
            refreshDisplayCopy(of: resultImage, into: \.displayResult)
            if !isRestoringHistory {
                pushUndo(oldValue)
            }
            if skipNextAutoCloudBackup {
                skipNextAutoCloudBackup = false
            } else {
                autoUploadResultIfEnabled()
            }
        }
    }
    private var skipNextAutoCloudBackup = false

    /// Screen-sized copies of the source/result (longest side 2048),
    /// rebuilt off the main thread once per image change. Every preview in
    /// the app reads these instead of handing a tens-of-megapixel image to
    /// SwiftUI, which used to re-downsample on every body evaluation —
    /// several times a second while a progress bar was moving.
    @Published private(set) var displaySource: UIImage?
    @Published private(set) var displayResult: UIImage?

    private func refreshDisplayCopy(of image: UIImage?, into keyPath: ReferenceWritableKeyPath<UpscalerViewModel, UIImage?>) {
        let isSource = keyPath == \UpscalerViewModel.displaySource
        setDisplayMatches(isSource, false)
        guard let image else {
            self[keyPath: keyPath] = nil
            return
        }
        if max(image.size.width, image.size.height) <= 2048 {
            self[keyPath: keyPath] = image
            setDisplayMatches(isSource, true)
            return
        }
        Task.detached(priority: .userInitiated) { [weak self] in
            let copy = image.downsampledForDisplay(maxDimension: 2048)
            await MainActor.run {
                guard let self else { return }
                // Drop a stale copy if the image changed again meanwhile.
                let current = isSource ? self.sourceImage : self.resultImage
                if current === image {
                    self[keyPath: keyPath] = copy
                    self.setDisplayMatches(isSource, true)
                }
            }
        }
    }

    private func setDisplayMatches(_ isSource: Bool, _ value: Bool) {
        if isSource { displaySourceMatches = value } else { displayResultMatches = value }
    }

    /// The cheapest image to build an on-screen preview of `image` from:
    /// the cached 2048px display copy when it belongs to `image`, else
    /// `image` itself. Tool tabs downscale from this to their preview
    /// size, so a 48 MP result costs a 2048px resample instead of a full one.
    func previewBase(for image: UIImage) -> UIImage {
        if image === resultImage, let displayResult, displayResultMatches { return displayResult }
        if image === sourceImage, let displaySource, displaySourceMatches { return displaySource }
        return image
    }
    private var displayResultMatches = false
    private var displaySourceMatches = false

    private func autoUploadResultIfEnabled() {
        guard provider.autoCloudBackupEnabled, let image = resultImage else { return }
        Task.detached(priority: .background) {
            do {
                try await ImportExportService.upload(image, kind: .exports)
                await CloudBackupStatus.shared.reportSuccess()
            } catch {
                await CloudBackupStatus.shared.reportFailure(error)
            }
        }
    }
    /// Bumped whenever `sourceImage`/`resultImage` change, so persistent
    /// tool tabs can notice "the current photo changed" — `UIImage` isn't
    /// `Equatable`, so `.onChange(of: resultImage)` isn't possible directly.
    @Published private(set) var imageVersion = 0
    @Published var isUpscaling = false
    @Published var progress: Double = 0
    @Published var errorMessage: String?
    @Published var savedConfirmation = false
    @Published var lastSaveOutcome: PhotoLibrarySaver.SaveOutcome?

    /// Live view of the running upscale (tile grid + progressively filled
    /// preview). nil when nothing is running.
    @Published private(set) var liveFrame: UpscaleLiveFrame?
    @Published private(set) var isPaused = false
    @Published private(set) var runStartedAt: Date?
    @Published private(set) var lastRunSummary: UpscaleRunSummary?
    private var runTask: Task<Void, Never>?
    private var session: UpscaleSession?

    @Published var comparisonResults: [ModelComparisonResult] = []
    /// True while a picked candidate's full-resolution pixels are being read
    /// back off disk — brief, but long enough to be worth not looking frozen.
    @Published private(set) var isLoadingPick = false
    @Published var isComparing = false
    @Published var comparisonProgress: Double = 0

    @Published var isRemovingBackground = false

    let provider: UpscalerProvider

    private var sourceFileSizeBytes: Int?
    private var sourceAssetIdentifier: String?

    init(provider: UpscalerProvider) {
        self.provider = provider
    }

    // MARK: - Undo / redo

    /// Every change to `resultImage` (any tool's Apply, an upscale, a
    /// revert) is undoable. Entries are whole images, so the history is
    /// bounded by memory rather than count: an eighth of physical RAM, so
    /// a few big upscales or dozens of small edits fit.
    @Published private(set) var canUndo = false
    @Published private(set) var canRedo = false
    private var undoStack: [UIImage?] = []
    private var redoStack: [UIImage?] = []
    private var isRestoringHistory = false
    private static let historyByteBudget = Int(ProcessInfo.processInfo.physicalMemory / 8)

    private static func cost(_ image: UIImage?) -> Int {
        guard let cg = image?.cgImage else { return 0 }
        return cg.bytesPerRow * cg.height
    }

    private func pushUndo(_ previous: UIImage?) {
        undoStack.append(previous)
        redoStack.removeAll()
        trimHistory()
    }

    private func trimHistory() {
        while undoStack.count > 1,
              (undoStack + redoStack).reduce(0, { $0 + Self.cost($1) }) > Self.historyByteBudget {
            undoStack.removeFirst()
        }
        while undoStack.count > 30 { undoStack.removeFirst() }
        canUndo = !undoStack.isEmpty
        canRedo = !redoStack.isEmpty
    }

    func undo() {
        guard !isBusy, let previous = undoStack.popLast() else { return }
        redoStack.append(resultImage)
        restore(previous)
        Haptics.lightImpact()
        ActionLoggingService.log("undo", outcome: "success")
    }

    func redo() {
        guard !isBusy, let next = redoStack.popLast() else { return }
        undoStack.append(resultImage)
        restore(next)
        Haptics.lightImpact()
        ActionLoggingService.log("redo", outcome: "success")
    }

    private func restore(_ image: UIImage?) {
        isRestoringHistory = true
        skipNextAutoCloudBackup = true
        resultImage = image
        isRestoringHistory = false
        trimHistory()
    }

    private func clearHistory() {
        undoStack.removeAll()
        redoStack.removeAll()
        lastRunSummary = nil
        trimHistory()
    }

    var isBusy: Bool { isUpscaling || isComparing || isRemovingBackground }

    // MARK: - Loading

    func load(from item: PhotosPickerItem) async {
        errorMessage = nil
        do {
            guard let data = try await item.loadTransferable(type: Data.self),
                  let image = UIImage(data: data),
                  let cgImage = image.cgImage else {
                errorMessage = UpscaleError.invalidImage.errorDescription
                return
            }
            resultImage = nil
            sourceAssetIdentifier = item.itemIdentifier
            // Normalize to scale 1 / .up orientation up front — every tiling
            // and drawing calculation downstream assumes 1 point == 1 pixel
            // and no rotation, matching the raw cgImage's pixel grid.
            sourceImage = UIImage(cgImage: cgImage, scale: 1, orientation: .up)
            sourceFileSizeBytes = data.count
            clearHistory()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// For a photo handed in directly (Share Extension, Files, clipboard)
    /// rather than picked from Photos — no asset to overwrite on save.
    func loadSharedImage(_ image: UIImage) {
        guard let cgImage = image.cgImage else { return }
        errorMessage = nil
        resultImage = nil
        sourceAssetIdentifier = nil
        sourceImage = UIImage(cgImage: cgImage, scale: 1, orientation: .up)
        sourceFileSizeBytes = nil
        clearHistory()
    }

    var pasteboardHasImage: Bool { UIPasteboard.general.hasImages }

    /// Pulls a copied image straight off the clipboard — the quickest way
    /// in from Safari, Messages or a screenshot.
    func loadFromPasteboard() {
        guard let image = UIPasteboard.general.image else {
            errorMessage = "There's no image on the clipboard."
            Haptics.error()
            return
        }
        // A UIImage from the pasteboard can carry a non-.up orientation;
        // redraw upright before normalizing to the raw pixel grid.
        let upright: UIImage
        if image.imageOrientation == .up {
            upright = image
        } else {
            let format = UIGraphicsImageRendererFormat()
            format.scale = 1
            upright = UIGraphicsImageRenderer(size: image.size, format: format).image { _ in
                image.draw(in: CGRect(origin: .zero, size: image.size))
            }
        }
        loadSharedImage(upright)
        Haptics.success()
        ActionLoggingService.log("paste_import", outcome: "success")
    }

    // MARK: - Planning

    /// Exact output size, tile count and time estimate for the current
    /// photo and settings — shown before Upscale is tapped.
    func plan() -> UpscalePlan? {
        guard let cg = (resultImage ?? sourceImage)?.cgImage else { return nil }
        let overlap = provider.quality.overlap(customOverlap: provider.customOverlap)
        let model = provider.modelChoice == .auto ? UpscaleModelChoice.generalPhoto.modelName : provider.modelChoice.modelName
        return UpscaleEstimator.plan(
            sourceWidth: cg.width, sourceHeight: cg.height, scale: provider.scaleFactor.rawValue,
            overlap: overlap, model: provider.modelChoice.isBundled || provider.modelChoice == .auto ? model : nil,
            power: provider.power
        )
    }

    // MARK: - Upscale

    /// Upscales the *current* image (the latest edit if there is one, else
    /// the original) so tools chain the same way everywhere.
    func upscale() {
        guard let input = resultImage ?? sourceImage, !isBusy else { return }
        isUpscaling = true
        progress = 0
        errorMessage = nil
        startSession()

        let session = self.session
        let backgroundTask = Self.beginBackgroundWork()
        runTask = Task {
            let startedAt = Date()
            // Resolve once so this run finishes with the upscaler it
            // started with, even if settings change mid-flight.
            let upscaler = await provider.resolveCurrent(for: input)
            if let notice = provider.lastFallbackNotice { errorMessage = notice }
            let outcome = await UpscaleRunner.run(
                input, using: upscaler, sourceFileSizeBytes: sourceFileSizeBytes,
                denoiseAmount: provider.denoiseBeforeUpscale ? 0.5 : 0,
                antiAliasingAmount: provider.antiAliasingAmount,
                sharpenAmount: provider.sharpenAmount,
                blendAmount: provider.upscaleStrength,
                detailLevel: "full_res:\(provider.power.rawValue)",
                requestedScale: provider.scaleFactor.rawValue,
                runKind: "single",
                sourceNoiseSigma: NoiseEstimator.sigma(of: input),
                session: session
            ) { [weak self] value in
                Task { @MainActor in self?.progress = value }
            }
            if let result = outcome.result {
                let seconds = Date().timeIntervalSince(startedAt)
                UpscaleEstimator.record(model: upscaler.techniqueInfo.modelName, power: provider.power, tiles: result.tileCount, seconds: seconds)
                lastRunSummary = UpscaleRunSummary(
                    modelName: upscaler.techniqueInfo.modelName.flatMap { name in
                        UpscaleModelChoice.allCases.first { $0.modelName == name }?.displayName
                    } ?? "Lanczos",
                    seconds: seconds, tiles: result.tileCount,
                    outputSize: CGSize(width: result.image.cgImage?.width ?? 0, height: result.image.cgImage?.height ?? 0),
                    outputWasCapped: result.outputWasCapped,
                    fidelityPSNR: result.fidelityPSNR,
                    tilesReused: result.tilesReused
                )
                skipNextAutoCloudBackup = true
                self.resultImage = result.image
                Haptics.success()
                if provider.autoSaveEnabled {
                    saveResultToPhotos()
                }
            } else if let error = outcome.error {
                handleRunError(error)
            }
            self.isUpscaling = false
            self.finishSession()
            Self.endBackgroundWork(backgroundTask)
        }
    }

    private func startSession() {
        isPaused = false
        runStartedAt = Date()
        liveFrame = nil
        session = UpscaleSession(power: provider.power, fidelity: provider.fidelity) { [weak self] frame in
            Task { @MainActor in
                guard let self, self.runStartedAt != nil else { return }
                self.liveFrame = frame
            }
        }
    }

    private func finishSession() {
        session = nil
        runTask = nil
        liveFrame = nil
        runStartedAt = nil
        isPaused = false
    }

    private func handleRunError(_ error: Error) {
        if error is CancellationError {
            Haptics.lightImpact()
        } else {
            errorMessage = error.localizedDescription
            Haptics.error()
        }
    }

    /// Parks the tile loop between batches — the phone cools and the
    /// battery rests, and nothing already processed is lost.
    func togglePause() {
        guard let session else { return }
        isPaused.toggle()
        session.setPaused(isPaused)
        Haptics.lightImpact()
        ActionLoggingService.log(isPaused ? "upscale_pause" : "upscale_resume", outcome: "success")
    }

    func cancelRun() {
        session?.setPaused(false)
        runTask?.cancel()
        ActionLoggingService.log("upscale_cancel", outcome: "success")
    }

    /// Lets a run in progress keep going for the short grace period iOS
    /// gives a backgrounded app, instead of being frozen mid-tile the
    /// instant the user switches away.
    private static func beginBackgroundWork() -> UIBackgroundTaskIdentifier {
        var identifier: UIBackgroundTaskIdentifier = .invalid
        identifier = UIApplication.shared.beginBackgroundTask(withName: "Upscale") {
            UIApplication.shared.endBackgroundTask(identifier)
            identifier = .invalid
        }
        return identifier
    }

    private static func endBackgroundWork(_ identifier: UIBackgroundTaskIdentifier) {
        guard identifier != .invalid else { return }
        UIApplication.shared.endBackgroundTask(identifier)
    }

    /// Clears the current result so `resultImage ?? sourceImage` falls
    /// back to the untouched original — itself undoable.
    func revertToOriginal() {
        guard resultImage != nil else { return }
        resultImage = nil
        Haptics.lightImpact()
    }

    /// Runs the *entire* current photo through every bundled model in turn
    /// and collects every result so the user can pick by eye.
    func compareModels() {
        guard let input = resultImage ?? sourceImage, !isBusy, provider.quality.overlap(customOverlap: provider.customOverlap) != nil else { return }
        isComparing = true
        comparisonProgress = 0
        comparisonResults = []
        errorMessage = nil
        startSession()
        let session = self.session
        let backgroundTask = Self.beginBackgroundWork()

        runTask = Task {
            defer {
                isComparing = false
                finishSession()
                Self.endBackgroundWork(backgroundTask)
            }
            let candidates = await provider.resolveAllBundled()
            guard !candidates.isEmpty else {
                errorMessage = "No bundled models available to compare."
                Haptics.error()
                return
            }

            // Groups this sweep's per-candidate `upscale_history` rows (each
            // tagged run_kind: "compare") with the `model_comparisons` row
            // posted at the end, and is what `ModelComparisonView` later
            // reports the user's pick against. Generated up front precisely
            // so the candidate rows can carry it as they're written.
            let comparisonID = UUID().uuidString
            comparisonSweepID = comparisonID
            // Anything still on disk belongs to a previous sweep (or to one
            // interrupted by a crash) and is unreachable now.
            ComparisonResultStore.clear()
            let sweepStartedAt = Date()
            let thermalStateStart = TelemetryService.thermalStateName

            // Render denoise only helps a render that's actually noisy; on a
            // clean one it erases texture before the model ever sees it.
            let sourceNoiseSigma = NoiseEstimator.sigma(of: input)
            let sourceIsNoisy = (sourceNoiseSigma ?? 0) > NoiseEstimator.renderDenoiseThreshold
            var results: [ModelComparisonResult] = []
            var candidatePayloads: [ComparisonCandidatePayload] = []
            for (index, candidate) in candidates.enumerated() {
                if Task.isCancelled { break }
                // The render-tuned candidates expect pre-cleaned input, so
                // they get the render denoise pass first (as Batch's auto
                // pick does); the others run on the photo as-is.
                let autoRenderDenoise = sourceIsNoisy && (candidate.choice == .render3D || candidate.choice == .stylizedRender)
                let candidateStartedAt = Date()
                let outcome = await UpscaleRunner.run(
                    input, using: candidate.upscaler, sourceFileSizeBytes: sourceFileSizeBytes,
                    antiAliasingAmount: provider.antiAliasingAmount,
                    autoRenderDenoise: autoRenderDenoise,
                    detailLevel: "full_res:\(provider.power.rawValue)",
                    requestedScale: provider.scaleFactor.rawValue,
                    runKind: "compare",
                    comparisonID: comparisonID,
                    sourceNoiseSigma: sourceNoiseSigma,
                    session: session
                ) { [weak self] tileProgress in
                    Task { @MainActor in
                        self?.comparisonProgress = (Double(index) + tileProgress) / Double(candidates.count)
                    }
                }
                let candidateMS = Int(Date().timeIntervalSince(candidateStartedAt) * 1000)
                if let result = outcome.result {
                    let sharpness = UpscalerProvider.sharpnessScore(result.image)
                    // Spill to disk and keep only a display-sized preview
                    // before the next model runs, so the sweep's footprint
                    // stays flat instead of growing by a finished image per
                    // candidate. Both the encode and the downsample go off
                    // the main actor — each is hundreds of milliseconds on a
                    // full-resolution result.
                    let entry = ModelComparisonResult(
                        choice: candidate.choice,
                        preview: await Task.detached(priority: .userInitiated) {
                            result.image.downsampledForDisplay(maxDimension: 2048)
                        }.value,
                        fileURL: await Task.detached(priority: .userInitiated) {
                            ComparisonResultStore.write(result.image, id: UUID())
                        }.value,
                        sharpnessScore: sharpness
                    )
                    results.append(entry)
                    candidatePayloads.append(ComparisonCandidatePayload(
                        model_name: candidate.choice.rawValue,
                        candidate_index: index,
                        succeeded: true,
                        processing_ms: candidateMS,
                        sharpness_score: sharpness,
                        fidelity_psnr: result.fidelityPSNR,
                        tile_count: result.tileCount,
                        tiles_reused: result.tilesReused,
                        render_denoise_applied: autoRenderDenoise
                    ))
                } else {
                    // A failed candidate is recorded too — "this model
                    // never produces a result on this device" is the most
                    // useful thing this sweep can tell us, and dropping
                    // the row would make it look like the model was simply
                    // never tried.
                    candidatePayloads.append(ComparisonCandidatePayload(
                        model_name: candidate.choice.rawValue,
                        candidate_index: index,
                        succeeded: false,
                        processing_ms: candidateMS,
                        render_denoise_applied: autoRenderDenoise,
                        error_message: outcome.error?.localizedDescription
                    ))
                    if outcome.error is CancellationError { break }
                }
            }

            comparisonResults = results
            if Task.isCancelled {
                Haptics.lightImpact()
            } else if results.isEmpty {
                errorMessage = "Every model failed to produce a result."
                Haptics.error()
            } else {
                Haptics.success()
            }
            ActionLoggingService.log(
                "compare_models",
                detail: [
                    "comparison_id": comparisonID,
                    "candidate_count": candidates.count,
                    "result_count": results.count,
                    "source_noise_sigma": sourceNoiseSigma,
                ],
                outcome: Task.isCancelled ? "cancelled" : (results.isEmpty ? "failed" : "success"),
                durationMS: Int(Date().timeIntervalSince(sweepStartedAt) * 1000)
            )

            let payload = ComparisonLogPayload(
                comparison_id: comparisonID,
                device_id: DeviceIdentity.current,
                source_width: Int(input.size.width),
                source_height: Int(input.size.height),
                cancelled: Task.isCancelled,
                total_ms: Int(Date().timeIntervalSince(sweepStartedAt) * 1000),
                // The sweep's own winner by the metric the UI ranks on,
                // recorded next to the user's eventual pick so the two can
                // be compared directly. If they disagree often, the metric
                // is not measuring what people actually prefer.
                auto_pick_model: results.max(by: { $0.sharpnessScore < $1.sharpnessScore })?.choice.rawValue,
                source_noise_sigma: sourceNoiseSigma,
                thermal_state_start: thermalStateStart,
                thermal_state_end: TelemetryService.thermalStateName,
                os_version: UIDevice.current.systemVersion,
                device_model: UIDevice.current.model,
                candidates: candidatePayloads
            )
            Task.detached(priority: .background) {
                await ComparisonLoggingService.log(payload)
            }
        }
    }

    /// The sweep `comparisonResults` came from, so `ModelComparisonView`
    /// can report which result the user picked against the right
    /// `model_comparisons` row. Replaced at the start of each sweep, so a
    /// pick is always reported against the sweep currently on screen.
    private(set) var comparisonSweepID: String?

    /// Called when the user chooses one of the compared results — the one
    /// ground-truth quality signal this app has (every other is a
    /// heuristic standing in for "looks better").
    func recordComparisonPick(_ choice: UpscaleModelChoice) {
        ActionLoggingService.log("comparison_pick", detail: [
            "model": choice.rawValue,
            "comparison_id": comparisonSweepID,
            "candidate_count": comparisonResults.count,
        ], outcome: "success")
        guard let comparisonSweepID else { return }
        let modelName = choice.rawValue
        Task.detached(priority: .background) {
            await ComparisonLoggingService.recordPick(comparisonID: comparisonSweepID, modelName: modelName)
        }
    }

    // MARK: - Patch preview

    @Published private(set) var isPreviewingPatch = false

    /// Runs the current settings — or, in Auto, every bundled model — on a
    /// small crop around `center` (normalized 0...1), with the same
    /// fidelity pass as a full run. A few tiles per model instead of a
    /// thousand: seconds and almost no battery to find out which model
    /// suits this photo before committing to the full upscale. Not logged
    /// to History.
    func previewPatch(at center: CGPoint, side: Int = 160) async -> (before: UIImage, results: [PatchPreviewResult])? {
        guard let input = resultImage ?? sourceImage, let cg = input.cgImage, !isBusy, !isPreviewingPatch else { return nil }
        isPreviewingPatch = true
        defer { isPreviewingPatch = false }
        let size = min(side, cg.width, cg.height)
        let x = min(max(0, Int(center.x * CGFloat(cg.width)) - size / 2), cg.width - size)
        let y = min(max(0, Int(center.y * CGFloat(cg.height)) - size / 2), cg.height - size)
        guard let crop = cg.cropping(to: CGRect(x: x, y: y, width: size, height: size)) else { return nil }
        let patch = UIImage(cgImage: crop, scale: 1, orientation: .up)
        let session = UpscaleSession(power: provider.power, fidelity: provider.fidelity)

        var candidates: [(choice: UpscaleModelChoice?, upscaler: ImageUpscaling)] = []
        if provider.modelChoice == .auto, provider.quality != .fast {
            candidates = await provider.resolveAllBundled().map { (choice: Optional($0.choice), upscaler: $0.upscaler) }
        } else {
            let upscaler = await provider.resolveCurrent(for: input)
            let choice = provider.modelChoice == .auto ? provider.lastAutoSelectedModel : provider.modelChoice
            candidates = [(provider.quality == .fast ? nil : choice, upscaler)]
        }
        var results: [PatchPreviewResult] = []
        for candidate in candidates {
            guard let result = try? await candidate.upscaler.upscale(patch, session: session, progress: { _ in }) else { continue }
            results.append(PatchPreviewResult(
                choice: candidate.choice,
                title: candidate.choice?.displayName ?? "Lanczos",
                image: result.image,
                fidelityPSNR: result.fidelityPSNR
            ))
        }
        ActionLoggingService.log("patch_preview", detail: ["candidates": candidates.count, "results": results.count])
        return results.isEmpty ? nil : (patch, results)
    }

    /// Called when the user taps "Use This" on one of `comparisonResults`.
    func pickComparisonResult(_ result: ModelComparisonResult) {
        // Before clearing `comparisonResults` — the pick record includes
        // how many results the choice was made from, which is the
        // difference between "best of 6" and "the only one that worked".
        recordComparisonPick(result.choice)
        let input = sourceImage
        let choice = result.choice
        let preview = result.preview
        let fileURL = result.fileURL
        comparisonResults = []
        guard let fileURL else {
            // The spill failed for this candidate, so `preview` is the full
            // image rather than a downsample — use it directly.
            applyPick(preview, choice: choice, source: input)
            return
        }
        // Read the full-resolution pixels back off the main actor; decoding
        // a 20MP PNG there would visibly stall the dismiss animation.
        isLoadingPick = true
        Task {
            let full = await Task.detached(priority: .userInitiated) {
                ComparisonResultStore.load(fileURL)
            }.value
            isLoadingPick = false
            // Falling back to the preview would silently hand back a
            // 2048px image in place of the real result, so say so instead.
            guard let full else {
                errorMessage = "Couldn't load that result — try running Compare Models again."
                Haptics.error()
                ComparisonResultStore.clear()
                return
            }
            applyPick(full, choice: choice, source: input)
            ComparisonResultStore.clear()
        }
    }

    private func applyPick(_ image: UIImage, choice: UpscaleModelChoice, source: UIImage?) {
        skipNextAutoCloudBackup = true
        resultImage = image
        uploadComparisonPick(image, choice: choice, source: source)
    }

    /// Dismissing the grid without picking — drops the spilled candidates,
    /// which nothing can reach once `comparisonResults` is empty.
    func dismissComparison() {
        comparisonResults = []
        ComparisonResultStore.clear()
    }

    /// A sweep's candidate runs deliberately skip the per-run cloud upload
    /// (see `UpscaleRunner.log`) — otherwise one Compare Models run pushed
    /// the source once per candidate and every candidate's result, for a
    /// set of images the user is about to discard all but one of. The
    /// upload happens here instead, once, for the result actually chosen.
    private func uploadComparisonPick(_ image: UIImage, choice: UpscaleModelChoice, source: UIImage?) {
        let temporarySave = provider.temporaryCloudSaveEnabled
        let autoBackup = provider.autoCloudBackupEnabled
        guard temporarySave || autoBackup else { return }
        let ttlHours = provider.temporaryCloudTTLHours
        let label = [choice.modelName, "\(provider.scaleFactor.rawValue)x", "compare_pick"]
            .joined(separator: " · ")
        // Same assertion the per-run upload takes: this starts right as the
        // user finishes choosing, which is exactly when they're likely to
        // leave the app.
        let assertion = Self.beginBackgroundWork()
        Task.detached(priority: .utility) {
            defer { Task { @MainActor in Self.endBackgroundWork(assertion) } }
            var lastError: Error?
            if autoBackup, let source {
                do { try await ImportExportService.upload(source, kind: .imports, isAuto: true) } catch { lastError = error }
            }
            do {
                try await ImportExportService.upload(
                    image, kind: .exports, ttlHours: ttlHours, isAuto: true, label: label
                )
            } catch { lastError = error }
            if let lastError {
                await CloudBackupStatus.shared.reportFailure(lastError)
            } else {
                await CloudBackupStatus.shared.reportSuccess()
            }
        }
    }

    /// Cuts the subject out of the *current* image — the most recent
    /// result if there is one (an upscale, a previous cutout, a crop...),
    /// otherwise the original photo — so tools chain onto each other the
    /// same way Adjust and Crop do, rather than Cutout alone always
    /// reaching back to the untouched original. `resultImage` is reused
    /// as where the cutout lands since every downstream action (Save/
    /// Share/Copy/the compare slider) already just works with whatever
    /// image is there, transparency included.
    func removeBackground() {
        guard let workingImage = resultImage ?? sourceImage, !isRemovingBackground else { return }
        isRemovingBackground = true
        errorMessage = nil

        Task {
            do {
                let cutout = try await BackgroundRemovalService.removeBackground(from: workingImage)
                self.resultImage = cutout
                Haptics.success()
                ActionLoggingService.log("cutout", detail: ["outcome": "success"])
            } catch {
                self.errorMessage = error.localizedDescription
                Haptics.error()
                ActionLoggingService.log("cutout", detail: ["outcome": "failed", "error": error.localizedDescription])
            }
            self.isRemovingBackground = false
        }
    }

    /// Overwrites the original photo in place by default (see
    /// `PhotoLibrarySaver`) rather than adding a second, duplicate asset
    /// next to it — applies no matter which tool(s) produced `resultImage`
    /// (Upscale, Cutout, Adjust, Crop, Filters, Overlays, Erase all share
    /// this one property), since they all funnel through the same "current
    /// result" state.
    func saveResultToPhotos() {
        guard let resultImage else { return }
        let imageToSave = provider.watermarkEnabled
            ? Watermark.apply(
                text: provider.watermarkText, position: provider.watermarkPosition,
                opacity: provider.watermarkOpacity, to: resultImage
            )
            : resultImage
        Task {
            do {
                let outcome = try await PhotoLibrarySaver.save(
                    imageToSave, overwriting: sourceAssetIdentifier,
                    format: provider.exportFormat, quality: provider.exportQuality,
                    forceNewAsset: provider.preserveOriginal, addToAlbum: provider.addToAlbumEnabled
                )
                lastSaveOutcome = outcome
                // The replace is delete-original-and-create-new under the
                // hood (see PhotoLibrarySaver), not a true in-place edit —
                // sourceAssetIdentifier now points at a deleted asset, so
                // it needs to track the replacement instead, or a second
                // save later in this same session (e.g. after another
                // edit) would degrade to "asset not found" every time.
                if case .overwroteOriginal(let newAssetIdentifier) = outcome {
                    sourceAssetIdentifier = newAssetIdentifier
                }
                savedConfirmation = true
                Haptics.success()
                ActionLoggingService.log("save", detail: outcome.logDetail.merging(
                    ["model_choice": provider.modelChoice.rawValue, "had_source_asset_identifier": sourceAssetIdentifier != nil],
                    uniquingKeysWith: { _, new in new }
                ))
            } catch {
                errorMessage = error.localizedDescription
                Haptics.error()
                ActionLoggingService.log("save", detail: ["outcome": "threw", "error": error.localizedDescription])
            }
        }
    }

    /// Saves every current comparison result to Photos in one action, for
    /// anyone who'd rather decide later (or keep more than one) than pick
    /// a single winner on the spot. Deliberately always adds new assets
    /// rather than going through `PhotoLibrarySaver`'s overwrite-in-place
    /// default — there's no single "the" result here to overwrite the
    /// original with, that's the whole point of saving every candidate.
    func saveAllComparisonResultsToPhotos() {
        guard !comparisonResults.isEmpty else { return }
        // One at a time off disk — `map(\.image)` would pull every
        // candidate back into memory simultaneously, which is the exact
        // thing spilling them was meant to avoid.
        let entries = comparisonResults.map { ($0.fileURL, $0.preview) }
        let watermarkEnabled = provider.watermarkEnabled
        let watermarkText = provider.watermarkText
        let watermarkPosition = provider.watermarkPosition
        let watermarkOpacity = provider.watermarkOpacity
        Task {
            do {
                for (fileURL, preview) in entries {
                    let full = fileURL.flatMap { url in
                        ComparisonResultStore.load(url)
                    }
                    // No spill for this candidate means `preview` is the
                    // full image (see ModelComparisonResult.fileURL).
                    let image = full ?? preview
                    let imageToSave = watermarkEnabled
                        ? Watermark.apply(text: watermarkText, position: watermarkPosition, opacity: watermarkOpacity, to: image)
                        : image
                    try await PhotoLibrarySaver.saveAsNewAsset(
                        imageToSave, format: provider.exportFormat, quality: provider.exportQuality,
                        addToAlbum: provider.addToAlbumEnabled
                    )
                }
                savedConfirmation = true
                Haptics.success()
            } catch {
                errorMessage = error.localizedDescription
                Haptics.error()
            }
        }
    }
}
