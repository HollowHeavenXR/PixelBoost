import CoreImage
import CoreML
import Foundation
import UIKit

/// Which bundled Core ML model to use. Each case's `modelName` must match a
/// compiled model in Models/ (see Models/README.md) — resolving a choice
/// whose model isn't actually bundled falls back to `LanczosUpscaler`
/// rather than throwing, same as the original single-model behavior.
/// `.textDocument` isn't bundled yet (no matching `.mlmodelc`) — it exists
/// so the picker honestly shows where the model lineup is headed,
/// degrading the same "not bundled" way any missing model already does
/// rather than hiding the option entirely.
enum UpscaleModelChoice: String, CaseIterable, Identifiable {
    case auto
    case generalPhoto
    case anime
    case portrait
    case lowLight
    case render3D
    case stylizedRender
    case sharp2x
    case animeVideo
    case textDocument

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .auto: return "Auto"
        case .generalPhoto: return "General Photo"
        case .anime: return "Anime / Illustration"
        case .portrait: return "Portrait"
        case .lowLight: return "Fast & Clean"
        case .render3D: return "3D / CG Render"
        case .stylizedRender: return "Toon / Cel-Shaded Render"
        case .sharp2x: return "Native 2×"
        case .animeVideo: return "Anime Video / Line Art"
        case .textDocument: return "Text & Documents"
        }
    }

    var modelName: String {
        switch self {
        case .auto: return ""
        case .generalPhoto: return "RealESRGAN"
        case .anime: return "RealESRGANAnime"
        // RealESRNet_x4plus: same RRDBNet architecture and training data as
        // RealESRGAN, but trained with only L1 loss (no GAN) — noticeably
        // smoother, less prone to the over-sharpened/ringing artifacts GAN
        // training can put on skin and other soft gradients, which is why
        // it's the Portrait pick rather than the general-purpose one.
        case .portrait: return "RealESRNet"
        // realesr-general-x4v3: a much smaller/faster SRVGGNetCompact
        // architecture (32 conv layers vs RRDBNet's 23 dense residual
        // blocks) built for everyday real-world photos — quicker per tile
        // with a cleaner, lower-artifact result than the heavier models.
        case .lowLight: return "RealESRGeneralV3"
        // BSRGAN: same RRDBNet architecture as General Photo, but trained
        // on a much broader/harsher synthetic degradation pipeline (mixed
        // blur kernels, noise, JPEG artifacts, sensor patterns) rather than
        // clean bicubic downsampling — more robust on a render exported at
        // a low sample count or through a lossy render-farm JPEG than a
        // model trained assuming "clean" source images. See
        // Models/README.md.
        case .render3D: return "BSRGAN"
        // Real-CUGAN (up4x, no-denoise): a from-scratch U-Net (not
        // RRDBNet), trained specifically on anime/illustration-style art —
        // clean line structure with noticeably less over-smoothing than
        // waifu2x on toon-shaded/cel-shaded content, the look a Blender
        // NPR/toon-shader render or stylized 2D-flat 3D scene tends to
        // share with hand-drawn anime art. Distinct from Anime/
        // Illustration above (Real-ESRGAN's anime_6B, a photographic GAN
        // trained on a broader illustration mix) — this one specifically
        // targets clean-line toon/cel content. See Models/README.md.
        case .stylizedRender: return "RealCUGAN"
        // Real-ESRGAN x2plus: the only bundled model with a native 2x
        // ratio. Every other model is architecturally 4x, so a 2x request
        // makes them analyze at 4x and resample down; this one produces the
        // requested size directly, which means for the same model-input
        // budget it covers 4x the source area per tile. Best pick when
        // Output Scale is 2x — at 4x its output has to be stretched
        // instead, which is the trade in reverse.
        case .sharp2x: return "RealESRGANx2"
        // realesr-animevideov3: SRVGGNetCompact (16 convs), trained on
        // anime *video* frames — built for the compression artifacts and
        // flat line work of that source, and much lighter than the
        // RRDBNet anime model, which matters now that Detail lets the
        // model see many more pixels.
        case .animeVideo: return "RealESRGANAnimeVideo"
        case .textDocument: return "RealESRGANText"
        }
    }

    /// The model's own architectural output ratio — what `ImageTiler`
    /// plans against and what `CoreMLTileUpscaler.Config.scaleFactor` is
    /// set to. Independent of `UpscaleFactor` (the size the *user* asked
    /// for): see `ScaledOutputUpscaler`.
    var nativeScale: Int {
        switch self {
        case .sharp2x: return 2
        default: return 4
        }
    }

    /// Cheap synchronous existence check (no MLModel load) — lets the UI
    /// show a "not bundled" state without needing a full async resolve.
    /// `.auto` reports bundled as long as at least one real candidate is,
    /// since it never resolves to a model of its own.
    var isBundled: Bool {
        if self == .auto {
            return UpscaleModelChoice.allCases.contains { $0 != .auto && $0.isBundled }
        }
        return Bundle.main.url(forResource: modelName, withExtension: "mlmodelc") != nil
    }
}

/// How much time to trade for quality. Tile size (128) is baked into the
/// bundled models' fixed compiled input shape and can't vary per preset —
/// the only safely-adjustable axis is context overlap, plus the choice of
/// whether to use a model at all. `.custom` hands that axis directly to the
/// user (Settings' Tile Overlap slider) instead of a fixed preset value —
/// previously the only way to pick an arbitrary overlap was a server-backed
/// Custom Preset; this needs no server.
enum UpscaleQuality: String, CaseIterable, Identifiable {
    case fast
    case standard
    case best
    case custom

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .fast: return "Fast"
        case .standard: return "Standard"
        case .best: return "Best"
        case .custom: return "Custom"
        }
    }

    /// nil means "skip the model entirely" — `.fast` uses `LanczosUpscaler`.
    /// `.custom`'s value comes from the caller (`UpscalerProvider.customOverlap`)
    /// rather than a fixed constant here.
    func overlap(customOverlap: Int) -> Int? {
        switch self {
        case .fast: return nil
        case .standard: return 8
        case .best: return 16
        case .custom: return customOverlap
        }
    }
}

/// Final output size, as a multiple of the source photo's own dimensions.
/// Independent of which model runs — see `ScaledOutputUpscaler` for how a
/// model fixed at a 4x native scale still delivers 2x/3x output.
enum UpscaleFactor: Int, CaseIterable, Identifiable {
    case x2 = 2
    case x3 = 3
    case x4 = 4

    var id: Int { rawValue }
    var displayName: String { "\(rawValue)×" }
}

/// File format saved results are encoded as. `.auto` keeps the original
/// heuristic (PNG for anything with real alpha — a Cutout result, most
/// obviously — JPEG otherwise) rather than forcing one format regardless
/// of transparency. See `PhotoLibrarySaver`.
enum ExportFormat: String, CaseIterable, Identifiable {
    case auto
    case heic
    case jpeg
    case png

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .auto: return "Auto"
        case .heic: return "HEIC"
        case .jpeg: return "JPEG"
        case .png: return "PNG"
        }
    }

    /// PNG is lossless — there's no quality dial to show for it.
    var usesQuality: Bool { self != .png }
}

/// Resolves the `ImageUpscaling` strategy to use for a run, based on the
/// user's model/quality selection, and caches loaded Core ML models by name
/// so switching quality presets back and forth doesn't reload the
/// (expensive) MLModel — only `CoreMLTileUpscaler.updateOverlap(_:)` needs
/// to run for that. Shared by `UpscalerViewModel` and
/// `BatchUpscaleViewModel` via one instance injected at the app level, so a
/// model switch in Settings is visible to whichever screen runs next.
@MainActor
final class UpscalerProvider: ObservableObject {
    private static let modelChoiceDefaultsKey = "com.pixelboost.modelChoice"
    private static let qualityDefaultsKey = "com.pixelboost.quality"
    private static let customOverlapDefaultsKey = "com.pixelboost.customOverlap"
    private static let upscaleStrengthDefaultsKey = "com.pixelboost.upscaleStrength"
    private static let scaleFactorDefaultsKey = "com.pixelboost.scaleFactor"
    private static let powerDefaultsKey = "com.pixelboost.power"
    private static let fidelityDefaultsKey = "com.pixelboost.fidelity"
    private static let exportFormatDefaultsKey = "com.pixelboost.exportFormat"
    private static let exportQualityDefaultsKey = "com.pixelboost.exportQuality"
    private static let denoiseBeforeUpscaleDefaultsKey = "com.pixelboost.denoiseBeforeUpscale"
    private static let antiAliasingAmountDefaultsKey = "com.pixelboost.antiAliasingAmount"
    private static let sharpenAmountDefaultsKey = "com.pixelboost.sharpenAmount"
    private static let autoSaveEnabledDefaultsKey = "com.pixelboost.autoSaveEnabled"
    /// Not `private` — `UpscaleRunner` reads this directly (it has no
    /// `UpscalerProvider` instance to go through) to decide whether an
    /// upscale's source/result images get uploaded alongside its metadata.
    static let autoCloudBackupEnabledDefaultsKey = "com.pixelboost.autoCloudBackupEnabled"
    /// Read by `UpscaleRunner` the same way (no provider instance there).
    /// `nonisolated` because this type is `@MainActor` but that read
    /// happens from a detached background task — these only touch
    /// UserDefaults, which is thread-safe.
    static let temporaryCloudSaveEnabledDefaultsKey = "com.pixelboost.temporaryCloudSaveEnabled"
    static let temporaryCloudTTLHoursDefaultsKey = "com.pixelboost.temporaryCloudTTLHours"

    /// On by default: every result is kept server-side for
    /// `storedTemporaryCloudTTLHours` and then deleted automatically. Only
    /// the *result* — the source photo is the separate, off-by-default Auto
    /// Cloud Backup.
    nonisolated static var storedTemporaryCloudSaveEnabled: Bool {
        UserDefaults.standard.object(forKey: temporaryCloudSaveEnabledDefaultsKey) as? Bool ?? true
    }

    /// Clamped to the server's own 1...168 accepted range.
    nonisolated static var storedTemporaryCloudTTLHours: Int {
        let stored = UserDefaults.standard.object(forKey: temporaryCloudTTLHoursDefaultsKey) as? Int ?? 24
        return min(max(stored, 1), 168)
    }
    private static let preserveOriginalDefaultsKey = "com.pixelboost.preserveOriginal"
    private static let addToAlbumEnabledDefaultsKey = "com.pixelboost.addToAlbumEnabled"
    private static let watermarkEnabledDefaultsKey = "com.pixelboost.watermarkEnabled"
    private static let watermarkTextDefaultsKey = "com.pixelboost.watermarkText"
    private static let watermarkPositionDefaultsKey = "com.pixelboost.watermarkPosition"
    private static let watermarkOpacityDefaultsKey = "com.pixelboost.watermarkOpacity"
    private static let defaultTabDefaultsKey = "com.pixelboost.defaultTab"
    private static let accentThemeDefaultsKey = "com.pixelboost.accentTheme"

    /// Side of the test region (before the model's own scale factor) run
    /// through each candidate during `BatchUpscaleViewModel`'s unattended
    /// auto-pick. Big enough to span a handful of `CoreMLTileUpscaler`
    /// tiles — one flat 128x128 crop wouldn't reliably tell two models
    /// apart — small enough that testing every bundled candidate still
    /// finishes in a fraction of the real upscale's time.
    private static let autoTestRegionSize = 256

    @Published var modelChoice: UpscaleModelChoice {
        didSet { UserDefaults.standard.set(modelChoice.rawValue, forKey: Self.modelChoiceDefaultsKey); Self.logChange("model_choice", modelChoice.rawValue) }
    }
    @Published var quality: UpscaleQuality {
        didSet { UserDefaults.standard.set(quality.rawValue, forKey: Self.qualityDefaultsKey); Self.logChange("quality", quality.rawValue) }
    }
    /// Only consulted when `quality == .custom` — see `UpscaleQuality.overlap(customOverlap:)`.
    /// 1...32 covers the full range `.fast`(nil)/.standard(8)/.best(16)
    /// already span, plus room past `.best` for anyone who wants to trade
    /// even more time for tile-seam quality.
    @Published var customOverlap: Int {
        didSet { UserDefaults.standard.set(customOverlap, forKey: Self.customOverlapDefaultsKey); Self.logChange("custom_overlap", customOverlap) }
    }
    /// 1.0 (default) is the model's raw output, unchanged. Below that,
    /// `UpscaleRunner` cross-dissolves the model's result with a plain
    /// Lanczos resize of the original at the same final size — a way to
    /// dial back an over-aggressive/artifact-prone model's effect without
    /// switching models entirely, down to 0.0 (the model runs, but its
    /// result is entirely replaced by the plain resize).
    @Published var upscaleStrength: Double {
        didSet { UserDefaults.standard.set(upscaleStrength, forKey: Self.upscaleStrengthDefaultsKey); Self.logChange("upscale_strength", upscaleStrength) }
    }
    @Published var scaleFactor: UpscaleFactor {
        didSet { UserDefaults.standard.set(scaleFactor.rawValue, forKey: Self.scaleFactorDefaultsKey); Self.logChange("scale_factor", scaleFactor.rawValue) }
    }
    /// Where and how hard upscales run — see `UpscalePower`. Replaced the
    /// old Detail setting, which shrank the photo before the model saw it;
    /// every upscale is full resolution now, pixel by pixel.
    /// Post-model refinement against the original's pixels (Detail Lock +
    /// Halo Guard) — see `UpscaleFidelity`. Natural by default: it's what
    /// stops soft sources from coming out smeared or outlined.
    @Published var fidelity: UpscaleFidelity {
        didSet { UserDefaults.standard.set(fidelity.rawValue, forKey: Self.fidelityDefaultsKey); Self.logChange("fidelity", fidelity.rawValue) }
    }
    @Published var power: UpscalePower {
        didSet { UserDefaults.standard.set(power.rawValue, forKey: Self.powerDefaultsKey); Self.logChange("power", power.rawValue) }
    }
    @Published var exportFormat: ExportFormat {
        didSet { UserDefaults.standard.set(exportFormat.rawValue, forKey: Self.exportFormatDefaultsKey); Self.logChange("export_format", exportFormat.rawValue) }
    }
    /// JPEG/HEIC compression quality, 0...1. Meaningless for `.png`
    /// (lossless) — kept as a single shared value rather than one per
    /// format since a user picking between HEIC and JPEG almost certainly
    /// wants "the same tradeoff," not to retune it per format.
    @Published var exportQuality: Double {
        didSet { UserDefaults.standard.set(exportQuality, forKey: Self.exportQualityDefaultsKey); Self.logChange("export_quality", exportQuality) }
    }
    /// Runs `RestoreService.denoise` on the source photo before it's handed
    /// to the upscaler — helps a model avoid amplifying sensor noise into
    /// upscaled speckle on grainy/low-light source photos. Off by default
    /// since it softens fine detail slightly on already-clean photos.
    @Published var denoiseBeforeUpscale: Bool {
        didSet { UserDefaults.standard.set(denoiseBeforeUpscale, forKey: Self.denoiseBeforeUpscaleDefaultsKey); Self.logChange("denoise_before_upscale", denoiseBeforeUpscale) }
    }
    /// 0...1, a subtle Gaussian blur applied to the final result before any
    /// sharpen pass. This is the default anti-aliasing step for upscaled
    /// output — higher values smooth stair-stepped edges more, while 0 keeps
    /// the full model output untouched.
    @Published var antiAliasingAmount: Double {
        didSet { UserDefaults.standard.set(antiAliasingAmount, forKey: Self.antiAliasingAmountDefaultsKey); Self.logChange("anti_aliasing_amount", antiAliasingAmount) }
    }
    /// 0...1, applied via `PostSharpen` right after the upscale finishes
    /// (on the final, already-upscaled image). 0 is off — a model's own
    /// output is usually sharp enough on its own; this is for anyone who
    /// wants an extra edge-crispness pass on top, same idea as Restore's
    /// face-sharpen but applied over the whole frame.
    @Published var sharpenAmount: Double {
        didSet { UserDefaults.standard.set(sharpenAmount, forKey: Self.sharpenAmountDefaultsKey); Self.logChange("sharpen_amount", sharpenAmount) }
    }
    /// When on, a successful single-photo upscale calls
    /// `UpscalerViewModel.saveResultToPhotos()` on its own right after
    /// finishing — for anyone who always taps Save anyway. Doesn't apply to
    /// Batch (already saves every item as it completes) or Compare Models
    /// (nothing to save until a candidate's picked).
    @Published var autoSaveEnabled: Bool {
        didSet { UserDefaults.standard.set(autoSaveEnabled, forKey: Self.autoSaveEnabledDefaultsKey); Self.logChange("auto_save_enabled", autoSaveEnabled) }
    }
    /// When on, every upscale result *and* every edit ("Apply" on any tool
    /// tab, Cutout included) is uploaded to the server's temporary scratch
    /// storage the moment it's produced — see `UpscaleRunner.log` and
    /// `UpscalerViewModel.resultImage`'s `didSet`. Off by default: unlike
    /// `autoSaveEnabled` (a local Photos save) this sends photo bytes over
    /// the network to a server the user may not have configured/trusted,
    /// so it needs an explicit opt-in rather than inheriting "on" just
    /// because a default `ServerConfig.baseURL` happens to be baked in.
    /// Debug metadata logging (`/log/upscale`) is unaffected by this flag
    /// either way — only the image bytes themselves are gated.
    @Published var autoCloudBackupEnabled: Bool {
        didSet { UserDefaults.standard.set(autoCloudBackupEnabled, forKey: Self.autoCloudBackupEnabledDefaultsKey); Self.logChange("auto_cloud_backup_enabled", autoCloudBackupEnabled) }
    }
    /// When on (the default), every upscale result is kept in the server's
    /// expiring scratch storage for `temporaryCloudTTLHours` — a day by
    /// default — and deleted automatically after that. Lets a result be
    /// re-fetched from the Cloud tab (or another device) without re-running
    /// the model. Unlike `autoCloudBackupEnabled` this never uploads the
    /// source photo, only the result the user just produced.
    @Published var temporaryCloudSaveEnabled: Bool {
        didSet { UserDefaults.standard.set(temporaryCloudSaveEnabled, forKey: Self.temporaryCloudSaveEnabledDefaultsKey); Self.logChange("temporary_cloud_save_enabled", temporaryCloudSaveEnabled) }
    }
    /// How long a temporarily-saved result lives server-side. The server
    /// caps this at 168h (7 days) regardless.
    @Published var temporaryCloudTTLHours: Int {
        didSet { UserDefaults.standard.set(temporaryCloudTTLHours, forKey: Self.temporaryCloudTTLHoursDefaultsKey); Self.logChange("temporary_cloud_t_t_l_hours", temporaryCloudTTLHours) }
    }
    /// Master switch for every telemetry event, snapshot and upscale log
    /// (see `TelemetryService.isEnabled`). On by default — this is the
    /// always-on debug logging the README describes — but one switch turns
    /// all of it off.
    @Published var diagnosticsEnabled: Bool {
        didSet { UserDefaults.standard.set(diagnosticsEnabled, forKey: TelemetryService.diagnosticsEnabledDefaultsKey); Self.logChange("diagnostics_enabled", diagnosticsEnabled) }
    }
    /// When on, every save (single photo and Batch) always adds a new
    /// Photos asset instead of overwriting the original in place — the
    /// opt-out for anyone who wants the pre-overwrite-default behavior
    /// back. See `PhotoLibrarySaver`.
    @Published var preserveOriginal: Bool {
        didSet { UserDefaults.standard.set(preserveOriginal, forKey: Self.preserveOriginalDefaultsKey); Self.logChange("preserve_original", preserveOriginal) }
    }
    /// When on (the default), every saved photo is also added to a
    /// "PixelBoost" album in Photos — created on first use via
    /// `PhotoAlbumService` — so upscaled/edited photos are easy to find as a
    /// set instead of mixed into the Camera Roll with everything else.
    @Published var addToAlbumEnabled: Bool {
        didSet { UserDefaults.standard.set(addToAlbumEnabled, forKey: Self.addToAlbumEnabledDefaultsKey); Self.logChange("add_to_album_enabled", addToAlbumEnabled) }
    }
    @Published var watermarkEnabled: Bool {
        didSet { UserDefaults.standard.set(watermarkEnabled, forKey: Self.watermarkEnabledDefaultsKey); Self.logChange("watermark_enabled", watermarkEnabled) }
    }
    @Published var watermarkText: String {
        didSet {
            UserDefaults.standard.set(watermarkText, forKey: Self.watermarkTextDefaultsKey)
            // Length only — the watermark string is user-authored content
            // (often a real name or handle), and a debug log has no reason
            // to hold it.
            Self.logChange("watermark_text_length", watermarkText.count)
        }
    }
    @Published var watermarkPosition: WatermarkPosition {
        didSet { UserDefaults.standard.set(watermarkPosition.rawValue, forKey: Self.watermarkPositionDefaultsKey); Self.logChange("watermark_position", watermarkPosition.rawValue) }
    }
    @Published var watermarkOpacity: Double {
        didSet { UserDefaults.standard.set(watermarkOpacity, forKey: Self.watermarkOpacityDefaultsKey); Self.logChange("watermark_opacity", watermarkOpacity) }
    }
    /// Which tab `RootView` selects on launch. Read once, at app start —
    /// see `AccentTheme`'s doc comment for why settings read only once at
    /// launch are the safe pattern here (every tab stays mounted for the
    /// app's whole lifetime, so there's no later point this would "just
    /// re-apply" on its own without extra plumbing).
    @Published var defaultTab: AppTab {
        didSet { UserDefaults.standard.set(defaultTab.rawValue, forKey: Self.defaultTabDefaultsKey); Self.logChange("default_tab", defaultTab.rawValue) }
    }
    /// Persisted immediately on change, but only actually read by
    /// `PBColor` once, at first access — see `AccentTheme`. Settings shows
    /// the current selection either way (so the picker itself stays
    /// accurate), with a footnote explaining the next-launch delay.
    @Published var accentTheme: AccentTheme {
        didSet { UserDefaults.standard.set(accentTheme.rawValue, forKey: Self.accentThemeDefaultsKey); Self.logChange("accent_theme", accentTheme.rawValue) }
    }
    /// True while a not-yet-cached model is being loaded — lets the UI show
    /// a spinner instead of silently hitching on the first use of a given
    /// model.
    @Published private(set) var isLoadingModel = false
    /// True while `BatchUpscaleViewModel`'s unattended auto-pick is running
    /// its candidate models over the test region — a separate flag from
    /// `isLoadingModel` since it can span loading *multiple* models, not
    /// just one.
    @Published private(set) var isTestingModels = false
    /// Which model batch's auto-pick last landed on, so Settings can show
    /// "Last auto pick" instead of leaving the choice invisible. The
    /// interactive main screen no longer auto-picks silently — see
    /// `UpscalerViewModel.compareModels()` — so this only ever reflects a
    /// batch run.
    @Published private(set) var lastAutoSelectedModel: UpscaleModelChoice?
    /// Set when the last `resolveCurrent` could not use the model the user
    /// asked for — either a different bundled model stood in, or the plain
    /// Lanczos resize did. A Lanczos "upscale" finishes almost instantly and
    /// adds no detail, so it must never happen silently. nil when the
    /// requested model ran (or Fast/Lanczos was chosen on purpose).
    @Published private(set) var lastFallbackNotice: String?

    /// Every `@Published` setting above records its own change, so
    /// "which settings was this device actually running with" is
    /// answerable from the log without a separate call at each UI control
    /// (and without missing the ones changed by preset restore or a
    /// settings restore rather than by a tap).
    private static func logChange(_ setting: String, _ value: Any) {
        // Debounced — a slider's property observer fires on every
        // intermediate value during a drag.
        TelemetryService.recordSettingChange(setting, value: value)
    }

    private var cache: [String: CoreMLTileUpscaler] = [:]

    init() {
        // Auto is the default for new installs — it's the whole point of
        // having more than one bundled model, so new users shouldn't have
        // to know to go find it in Settings.
        modelChoice = UserDefaults.standard.string(forKey: Self.modelChoiceDefaultsKey)
            .flatMap(UpscaleModelChoice.init(rawValue:)) ?? .auto
        quality = UserDefaults.standard.string(forKey: Self.qualityDefaultsKey)
            .flatMap(UpscaleQuality.init(rawValue:)) ?? .standard
        let storedCustomOverlap = UserDefaults.standard.object(forKey: Self.customOverlapDefaultsKey) as? Int
        customOverlap = storedCustomOverlap ?? 8
        let storedUpscaleStrength = UserDefaults.standard.object(forKey: Self.upscaleStrengthDefaultsKey) as? Double
        upscaleStrength = storedUpscaleStrength ?? 1.0
        let storedScale = UserDefaults.standard.object(forKey: Self.scaleFactorDefaultsKey) as? Int
        scaleFactor = storedScale.flatMap(UpscaleFactor.init(rawValue:)) ?? .x4
        power = UserDefaults.standard.string(forKey: Self.powerDefaultsKey)
            .flatMap(UpscalePower.init(rawValue:)) ?? .balanced
        fidelity = UserDefaults.standard.string(forKey: Self.fidelityDefaultsKey)
            .flatMap(UpscaleFidelity.init(rawValue:)) ?? .natural
        temporaryCloudSaveEnabled = Self.storedTemporaryCloudSaveEnabled
        temporaryCloudTTLHours = Self.storedTemporaryCloudTTLHours
        diagnosticsEnabled = UserDefaults.standard.object(forKey: TelemetryService.diagnosticsEnabledDefaultsKey) as? Bool ?? true
        exportFormat = UserDefaults.standard.string(forKey: Self.exportFormatDefaultsKey)
            .flatMap(ExportFormat.init(rawValue:)) ?? .auto
        let storedQuality = UserDefaults.standard.object(forKey: Self.exportQualityDefaultsKey) as? Double
        exportQuality = storedQuality ?? 0.9
        denoiseBeforeUpscale = UserDefaults.standard.bool(forKey: Self.denoiseBeforeUpscaleDefaultsKey)
        let storedAntiAliasing = UserDefaults.standard.object(forKey: Self.antiAliasingAmountDefaultsKey) as? Double
        antiAliasingAmount = storedAntiAliasing ?? 0
        let storedSharpen = UserDefaults.standard.object(forKey: Self.sharpenAmountDefaultsKey) as? Double
        sharpenAmount = storedSharpen ?? 0
        autoSaveEnabled = UserDefaults.standard.bool(forKey: Self.autoSaveEnabledDefaultsKey)
        autoCloudBackupEnabled = UserDefaults.standard.bool(forKey: Self.autoCloudBackupEnabledDefaultsKey)
        preserveOriginal = (UserDefaults.standard.object(forKey: Self.preserveOriginalDefaultsKey) as? Bool) ?? false
        addToAlbumEnabled = (UserDefaults.standard.object(forKey: Self.addToAlbumEnabledDefaultsKey) as? Bool) ?? true
        watermarkEnabled = UserDefaults.standard.bool(forKey: Self.watermarkEnabledDefaultsKey)
        watermarkText = UserDefaults.standard.string(forKey: Self.watermarkTextDefaultsKey) ?? ""
        watermarkPosition = UserDefaults.standard.string(forKey: Self.watermarkPositionDefaultsKey)
            .flatMap(WatermarkPosition.init(rawValue:)) ?? .bottomRight
        let storedWatermarkOpacity = UserDefaults.standard.object(forKey: Self.watermarkOpacityDefaultsKey) as? Double
        watermarkOpacity = storedWatermarkOpacity ?? 0.7
        defaultTab = UserDefaults.standard.string(forKey: Self.defaultTabDefaultsKey)
            .flatMap(AppTab.init(rawValue:)) ?? .home
        accentTheme = UserDefaults.standard.string(forKey: Self.accentThemeDefaultsKey)
            .flatMap(AccentTheme.init(rawValue:)) ?? .blue
    }

    /// Resolves the upscaler for the *current* model/quality/scale
    /// selection. Call again after any of them changes. Used by the
    /// interactive single-image flow when a specific model (not `.auto`)
    /// is picked, and by `BatchUpscaleViewModel` regardless of model
    /// choice — batch has no one present to pick from a comparison per
    /// photo, so `.auto` there still resolves via the same unattended
    /// heuristic pick `.auto` itself used before Compare All existed.
    /// `sourceImage`, when provided, is used only for that unattended
    /// pick — `nil`-safe (falls back to the first bundled candidate)
    /// since not every caller has an image ready up front.
    func resolveCurrent(for sourceImage: UIImage? = nil) async -> ImageUpscaling {
        guard let overlap = quality.overlap(customOverlap: customOverlap) else {
            return LanczosUpscaler(scaleFactor: Double(scaleFactor.rawValue))
        }

        let choice: UpscaleModelChoice
        if modelChoice == .auto {
            choice = await autoSelectModel(for: sourceImage, overlap: overlap) ?? .generalPhoto
            lastAutoSelectedModel = choice
        } else {
            choice = modelChoice
        }

        lastFallbackNotice = nil
        if let base = await resolvedModel(for: choice, overlap: overlap) {
            return ScaledOutputUpscaler(base: base, targetScale: scaleFactor.rawValue)
        }
        // The requested model is missing or failed to load. Try every other
        // bundled model before giving up on super-resolution altogether.
        for other in UpscaleModelChoice.allCases where other != .auto && other != choice && other.isBundled {
            if let base = await resolvedModel(for: other, overlap: overlap) {
                lastFallbackNotice = "\(choice.displayName) couldn't be loaded, so \(other.displayName) was used instead."
                return ScaledOutputUpscaler(base: base, targetScale: scaleFactor.rawValue)
            }
        }
        lastFallbackNotice = "No super-resolution model could be loaded, so the photo was only resized (no added detail). Reinstall the app or pick a different model."
        return LanczosUpscaler(scaleFactor: Double(scaleFactor.rawValue))
    }

    /// Resolves every *actually bundled* real model at once, each wrapped
    /// to the current scale selection — the interactive counterpart to
    /// `resolveCurrent`'s single pick, for `UpscalerViewModel.compareModels()`
    /// to run the full photo through every one of them and let the user
    /// choose by eye instead of a heuristic choosing for them.
    func resolveAllBundled() async -> [(choice: UpscaleModelChoice, upscaler: ImageUpscaling)] {
        guard let overlap = quality.overlap(customOverlap: customOverlap) else { return [] }
        let candidates = UpscaleModelChoice.allCases.filter { $0 != .auto && $0.isBundled }
        var resolved: [(UpscaleModelChoice, ImageUpscaling)] = []
        for candidate in candidates {
            guard let base = await resolvedModel(for: candidate, overlap: overlap) else { continue }
            resolved.append((candidate, ScaledOutputUpscaler(base: base, targetScale: scaleFactor.rawValue)))
        }
        return resolved
    }

    /// Loads (or returns the already-cached) `CoreMLTileUpscaler` for
    /// `choice`, or `nil` if it isn't bundled / fails to load — every
    /// caller in this file that needs one concrete model goes through
    /// here so the cache stays a single source of truth.
    private func resolvedModel(for choice: UpscaleModelChoice, overlap: Int) async -> CoreMLTileUpscaler? {
        // Compute units are fixed at model load, so the power mode is part
        // of the cache key; switching modes loads a second instance once.
        let units = power.effective.computeUnits
        let key = "\(choice.modelName)|\(units.rawValue)"
        if let cached = cache[key] {
            cached.updateOverlap(overlap)
            return cached
        }
        guard let loaded = await loadUpscaler(named: choice.modelName, overlap: overlap, nativeScale: choice.nativeScale, computeUnits: units) else { return nil }
        // Only keep one compute-unit variant of each model resident.
        cache = cache.filter { !$0.key.hasPrefix(choice.modelName + "|") }
        cache[key] = loaded
        return loaded
    }

    private func loadUpscaler(named modelName: String, overlap: Int, nativeScale: Int, computeUnits: MLComputeUnits) async -> CoreMLTileUpscaler? {
        isLoadingModel = true
        defer { isLoadingModel = false }

        let config = CoreMLTileUpscaler.Config(tileSize: 128, scaleFactor: nativeScale, overlap: overlap, computeUnits: computeUnits)
        // Model failed to load (not bundled, corrupt, etc.) — the caller
        // doesn't cache a nil under this key, so a later retry (e.g. after
        // an app update that adds the model) can succeed.
        return await Task.detached(priority: .userInitiated) {
            try? CoreMLTileUpscaler(modelName: modelName, config: config)
        }.value
    }

    /// Runs every bundled real model over one shared crop of `sourceImage`
    /// and keeps whichever produced the sharpest, most-detailed result,
    /// with a content-aware nudge (see `contentAffinity`) from real
    /// pixel statistics of the *source* crop itself layered on top —
    /// used only by `BatchUpscaleViewModel`, where nobody is present to
    /// pick per photo across a queue of up to 20. Returns `nil` (caller
    /// falls back to `.generalPhoto`) if there's no image to test against
    /// or fewer than two real candidates to choose between.
    private func autoSelectModel(for sourceImage: UIImage?, overlap: Int) async -> UpscaleModelChoice? {
        let baseCandidates = UpscaleModelChoice.allCases.filter { $0 != .auto && $0.isBundled }
        let candidates: [UpscaleModelChoice]
        if let sourceImage,
           sourceImage.size.width * sourceImage.size.height > 24_000_000 {
            candidates = baseCandidates.filter { $0 == .generalPhoto || $0 == .lowLight || $0 == .portrait }
        } else {
            candidates = baseCandidates
        }

        guard candidates.count > 1,
              let sourceImage,
              let testRegion = Self.centerTestRegion(of: sourceImage, maxSize: Self.autoTestRegionSize) else {
            return candidates.first
        }

        isTestingModels = true
        defer { isTestingModels = false }

        // Measured once, from the same test crop every candidate runs
        // against — every bonus below is grounded in this one real read
        // of the source's own pixels, not a per-candidate guess.
        let contentStats = ImageStatistics.measure(testRegion)

        var best: (choice: UpscaleModelChoice, score: Double)?
        var scored: [(UpscaleModelChoice, Double, Double, Double)] = []
        for candidate in candidates {
            guard let upscaler = await resolvedModel(for: candidate, overlap: overlap) else { continue }
            guard let result = try? await upscaler.upscale(testRegion, progress: { _ in }) else { continue }
            let measured = Self.sharpnessScore(result.image)
            // Affinities are proportions of the candidate's own measured
            // score, not points added to it. As fixed point values they
            // were silently deciding the whole pick: real scores on this
            // metric land around 1-5, so a "+10 bonus" wasn't breaking a
            // tie, it was overruling the measurement it was meant to
            // supplement.
            var affinity = Self.scaleAffinity(for: candidate, requestedScale: scaleFactor.rawValue)
            if let contentStats {
                affinity += Self.contentAffinity(for: candidate, stats: contentStats)
            }
            // Raw edge energy rewards a GAN repainting fake texture into a
            // blank patch exactly as much as it rewards genuinely
            // sharpening a real edge — this is the one check that tells
            // the two apart, by grounding the candidate's edges against
            // where the *source* actually had any to begin with. A
            // candidate whose "detail" is mostly fabricated loses most of
            // its score here rather than winning the pick for looking
            // busiest.
            let hallucination = ImageStatistics.hallucinationFraction(source: testRegion, output: result.image, scale: candidate.nativeScale)
            let score = measured * (1 + affinity) * (1 - hallucination)
            scored.append((candidate, measured, hallucination, score))
            if best == nil || score > best!.score {
                best = (candidate, score)
            }
        }

        // The thresholds below are calibrated from a handful of images on
        // one machine (see Models/testbench/README.md). Logging what was
        // measured, and what every candidate scored, is what lets them be
        // re-calibrated from real photos on real devices instead of
        // re-guessed.
        ActionLoggingService.log("auto_model_pick", detail: [
            "chosen": best?.choice.rawValue,
            "requested_scale": scaleFactor.rawValue,
            "edge_energy": contentStats.map { round($0.edgeEnergy * 100) / 100 },
            "channel_spread": contentStats.map { round($0.channelSpread * 1000) / 1000 },
            "mean_luma": contentStats.map { round($0.meanLuma * 1000) / 1000 },
            "scores": scored.map { "\($0.0.rawValue):\(round($0.1 * 100) / 100)-fake\(round($0.2 * 100) / 100)->\(round($0.3 * 100) / 100)" }
                .joined(separator: ","),
        ], outcome: best == nil ? "failed" : "success")
        return best?.choice
    }

    /// A heuristic nudge from the *source* crop's own measured pixel
    /// statistics — on top of (never instead of) the empirical
    /// output-sharpness trial above, which actually runs the real models
    /// and measures their real results. This only breaks ties/close calls
    /// toward the model family whose intended content actually matches
    /// what's measured in the photo:
    ///
    /// - `.lowLight`: the source crop itself reads as genuinely dark.
    /// - `.animeVideo`: dense hard edge structure — the strongest
    ///   line-art signal, and the model that earns the largest nudge for
    ///   it. On measured line-art content it lands within ~15% of the
    ///   heavier anime model's edge energy while costing roughly a tenth
    ///   as much (1.6s vs 16.7s for the same crop on the test bench,
    ///   against 50s for General Photo), which matters a great deal now
    ///   that Detail can hand a model 4-16x as many pixels as it used to
    ///   get. On a close call for this kind of content, it should win.
    /// - `.anime`/`.stylizedRender`: the same signature, smaller nudge.
    /// - `.textDocument`: near-grayscale (`channelSpread` close to 0)
    ///   with a very wide luminance range — a page of dark text on a
    ///   light background.
    /// - everything else: no bonus: `.portrait`/`.generalPhoto`/`.render3D`
    ///   don't have a comparably distinct, reliably-measurable pixel
    ///   signature to key off of, so they're left to the sharpness trial
    ///   alone rather than a guess dressed up as a measurement.
    /// Returned as a *fraction* of the candidate's own measured score
    /// (0.1 = +10%), never as points — see `autoSelectModel`.
    ///
    /// `lineArtEnergy` is on `ImageStatistics.edgeEnergy`'s 0...255 scale.
    /// The previous threshold here read `edgeDensity > 20`, comparing a
    /// 0...255-scale constant against a 0...1-scale value, so this branch
    /// could never fire: the anime/toon affinity has never once applied
    /// since it was written. Measured references are in
    /// Models/testbench/README.md — an ordinary photo sits near 4, this
    /// app's own render screenshots 9-13, a dense poster 26 — so the
    /// threshold is deliberately low: it marks "has real line structure",
    /// and the sharpness trial still decides the rest.
    ///
    /// The colour condition that used to accompany it
    /// (`channelSpread > 0.12`) is gone: `channelSpread` is the spread of
    /// the three channel *means*, which is near zero for a balanced
    /// colourful image and large for one with a colour cast — it measures
    /// tint, not saturation, so as a "this is colourful line art" test it
    /// was wrong on its own terms. It still gates `.textDocument`, where
    /// "no colour cast at all" is genuinely the signal.
    private static let lineArtEnergy = 8.0

    private static func contentAffinity(for choice: UpscaleModelChoice, stats: ImageStatistics) -> Double {
        let isLineArt = stats.edgeEnergy > lineArtEnergy
        switch choice {
        case .lowLight:
            return stats.meanLuma < 0.35 ? 0.10 : 0
        case .animeVideo:
            return isLineArt ? 0.14 : 0
        case .anime, .stylizedRender:
            return isLineArt ? 0.08 : 0
        case .textDocument:
            return (stats.channelSpread < 0.05 && stats.maxLuma - stats.minLuma > 0.6) ? 0.12 : 0
        // `.sharp2x` is deliberately not given a *content* affinity: what
        // makes it the right pick is the requested output scale, not
        // anything measurable in the pixels — see `scaleAffinity`.
        case .portrait, .generalPhoto, .render3D, .sharp2x, .auto:
            return 0
        }
    }

    /// The one preference that isn't about the photo's content at all: a
    /// model whose native ratio already matches the requested output scale
    /// delivers it directly, while a mismatched one has its output
    /// resampled to get there (see `ScaledOutputUpscaler`). A fraction of the
    /// candidate's own score, like the content affinities above — enough
    /// to win a close call, not to override a clearly sharper result.
    private static func scaleAffinity(for choice: UpscaleModelChoice, requestedScale: Int) -> Double {
        choice.nativeScale == requestedScale ? 0.08 : 0
    }

    /// A center crop, not a resize — auto-selection needs to see the model
    /// operating at native tiling resolution on real detail, not a
    /// downsampled stand-in that would blur away the texture differences
    /// between candidates.
    private static func centerTestRegion(of image: UIImage, maxSize: Int) -> UIImage? {
        guard let cgImage = image.cgImage else { return nil }
        let width = cgImage.width
        let height = cgImage.height
        guard width >= 32, height >= 32 else { return nil }

        let size = min(maxSize, width, height)
        let normalized = UIImage(cgImage: cgImage, scale: 1, orientation: .up)
        let rect = CGRect(
            x: CGFloat((width - size) / 2), y: CGFloat((height - size) / 2),
            width: CGFloat(size), height: CGFloat(size)
        )
        return normalized.cropped(to: rect)
    }

    /// No-reference sharpness/detail proxy: desaturate, run a Laplacian
    /// (edge-detection) convolution, and average the response over the
    /// whole image — a crisper, more-detailed upscale has a stronger mean
    /// edge response than a smoother or blurrier one. The same family of
    /// metric autofocus systems use to score "is this in focus," repurposed
    /// here to compare candidate models instead of camera lens positions.
    /// Not `private` — `UpscalerViewModel.compareModels()` reuses it to
    /// show a sharpness figure alongside each full comparison result too.
    /// How much hard edge detail a candidate's output actually carries,
    /// 0...255 — mean absolute Laplacian, the same measurement
    /// `ImageStatistics.edgeEnergy` makes of a source, so a source's
    /// reading and a candidate's score are directly comparable.
    static func sharpnessScore(_ image: UIImage) -> Double {
        ImageStatistics.edgeEnergy(of: image)
    }
}
