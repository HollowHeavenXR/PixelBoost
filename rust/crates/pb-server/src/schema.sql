-- upscaler-bridge schema (PostgreSQL)
--
-- One append-only row per upscale attempt (success or failure) — this is a
-- debugging/analytics log, not a sync mechanism, so nothing here is ever
-- updated in place after insert.

CREATE TABLE IF NOT EXISTS upscale_history (
    id VARCHAR(36) PRIMARY KEY,
    -- Anonymous per-install identifier (UUID persisted in UserDefaults) —
    -- this app has no user accounts, so there's no real user_id to key on.
    device_id VARCHAR(64) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Source image.
    source_width INT NOT NULL,
    source_height INT NOT NULL,
    source_file_size_bytes INT,

    -- Technique actually used for this run — 'coreml_tile' or
    -- 'lanczos_fallback' (see UpscaleResult.technique on the iOS side).
    -- model_name/tile_size/overlap are NULL for the Lanczos fallback, which
    -- has no model or tiling.
    technique VARCHAR(30) NOT NULL,
    model_name VARCHAR(100),
    tile_size INT,
    overlap INT,
    scale_factor INT NOT NULL,
    tile_count INT,

    -- Result.
    output_width INT,
    output_height INT,
    processing_ms INT NOT NULL,
    success BOOLEAN NOT NULL,
    error_message TEXT,

    -- Device context — helps tell "this model is slow on all devices" apart
    -- from "this one device/OS version is the problem".
    app_version VARCHAR(20),
    os_version VARCHAR(20),
    device_model VARCHAR(50)
);
CREATE INDEX IF NOT EXISTS idx_device_history ON upscale_history (device_id, created_at);
CREATE INDEX IF NOT EXISTS idx_technique ON upscale_history (technique, created_at);
CREATE INDEX IF NOT EXISTS idx_failures ON upscale_history (success, created_at);

-- General-purpose action log — anything that isn't a full upscale attempt
-- (Save, Compare Models, Cutout, a Settings change, ...) but is still worth
-- having a record of when debugging a report we can't reproduce locally.
-- `detail` is a free-form JSON blob per action rather than a wide sparse
-- column set, since what's worth recording varies a lot by action.
CREATE TABLE IF NOT EXISTS action_log (
    id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    action VARCHAR(50) NOT NULL,
    detail TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    app_version VARCHAR(20),
    os_version VARCHAR(20),
    device_model VARCHAR(50)
);
CREATE INDEX IF NOT EXISTS idx_device_action ON action_log (device_id, action, created_at);

-- ---------------------------------------------------------------------------
-- Temporary image storage (imports/exports) — expiring, not permanent
-- ---------------------------------------------------------------------------

-- A device's original (pre-upscale) photos, uploaded on request — NOT part
-- of the normal on-device upscale flow (which never leaves the phone). This
-- exists purely as an opt-in backup/handoff mechanism: e.g. queue a photo
-- here before a batch job, or move a photo to another device without
-- AirDrop. Rows are deleted automatically once `expires_at` passes (see
-- main.py's cleanup loop) — this is temporary scratch storage, not a photo
-- library.
CREATE TABLE IF NOT EXISTS image_imports (
    id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    filename VARCHAR(255),
    content_type VARCHAR(100) DEFAULT 'image/jpeg',
    width INT,
    height INT,
    file_size_bytes INT NOT NULL,
    image_data BYTEA NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_device_imports ON image_imports (device_id, created_at);
CREATE INDEX IF NOT EXISTS idx_import_expiry ON image_imports (expires_at);

-- A device's upscaled results, uploaded on request — lets a result be
-- re-fetched (e.g. after local storage was cleared, or from a second
-- device) without re-running the model. Optionally linked back to the
-- upscale_history row that produced it. Same auto-expiry as image_imports.
CREATE TABLE IF NOT EXISTS image_exports (
    id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    history_id VARCHAR(36) REFERENCES upscale_history(id) ON DELETE SET NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    filename VARCHAR(255),
    content_type VARCHAR(100) DEFAULT 'image/png',
    width INT,
    height INT,
    file_size_bytes INT NOT NULL,
    image_data BYTEA NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_device_exports ON image_exports (device_id, created_at);
CREATE INDEX IF NOT EXISTS idx_export_expiry ON image_exports (expires_at);

-- ---------------------------------------------------------------------------
-- Customization
-- ---------------------------------------------------------------------------

-- User-named model+overlap combinations beyond the built-in Fast/Standard/
-- Best presets — e.g. "Portrait" = anime model + overlap 12. Permanent
-- (not expiring) — these are deliberate user configuration, not scratch data.
CREATE TABLE IF NOT EXISTS custom_presets (
    id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    name VARCHAR(100) NOT NULL,
    model_name VARCHAR(100) NOT NULL,
    overlap INT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (device_id, name)
);
CREATE INDEX IF NOT EXISTS idx_device_presets ON custom_presets (device_id);

-- Per-device settings backup. This app has no accounts, so "sync" here
-- really means "manually-triggered backup/restore to/from one server
-- record keyed by device_id" (see Settings' Backup/Restore actions),
-- not automatic multi-device sync. `updated_at` is set explicitly by the
-- app's own upsert query (see main.py's upsert_device_settings) — MySQL's
-- `ON UPDATE CURRENT_TIMESTAMP` column attribute this table used to declare
-- has no Postgres equivalent short of a trigger, and an explicit
-- `updated_at = CURRENT_TIMESTAMP` in the one query that ever touches this
-- row is simpler than adding one.
CREATE TABLE IF NOT EXISTS device_settings (
    device_id VARCHAR(64) PRIMARY KEY,
    haptics_enabled BOOLEAN DEFAULT TRUE,
    model_choice VARCHAR(50) DEFAULT 'generalPhoto',
    quality VARCHAR(50) DEFAULT 'standard',
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Metadata about bundled models — richer, server-editable descriptions
-- than hardcoding strings in the Swift client, and a natural place to add
-- new model entries' documentation ahead of an app update that bundles
-- them. `is_active` lets a model be listed without yet being recommended.
CREATE TABLE IF NOT EXISTS model_registry (
    model_name VARCHAR(100) PRIMARY KEY,
    display_name VARCHAR(100) NOT NULL,
    description TEXT,
    license VARCHAR(100),
    tile_size INT NOT NULL,
    scale_factor INT NOT NULL,
    is_active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- NOTE: no semicolons inside these string literals — db.py's init_db
-- splits schema.sql on ';' (after stripping comments), so one inside a
-- literal slices the statement in half and startup fails outright.
INSERT INTO model_registry (model_name, display_name, description, license, tile_size, scale_factor, is_active) VALUES
    ('RealESRGAN', 'General Photo', 'Real-ESRGAN x4plus — general-purpose photo upscaling, 23 RRDB blocks.', 'BSD-3-Clause', 128, 4, TRUE),
    ('RealESRGANAnime', 'Anime / Illustration', 'Real-ESRGAN x4plus anime_6B — optimized for anime/illustration art, 6 RRDB blocks (faster).', 'BSD-3-Clause', 128, 4, TRUE),
    ('RealESRNet', 'Portrait', 'RealESRNet_x4plus — same architecture as x4plus, trained without GAN loss: smoother, fewer artifacts on skin.', 'BSD-3-Clause', 128, 4, TRUE),
    ('RealESRGeneralV3', 'Fast & Clean', 'realesr-general-x4v3 — SRVGGNetCompact, much smaller/faster than any RRDBNet model.', 'BSD-3-Clause', 128, 4, TRUE),
    ('BSRGAN', '3D / CG Render', 'BSRGAN — trained on a harsher synthetic degradation pipeline, robust on renders and lossily-compressed source.', 'Apache-2.0', 128, 4, TRUE),
    ('RealCUGAN', 'Toon / Cel-Shaded Render', 'Real-CUGAN up4x — U-Net trained on anime art, holds clean line structure on flat-shaded content.', 'MIT', 128, 4, TRUE),
    ('RealESRGANx2', 'Native 2x', 'Real-ESRGAN x2plus — the only bundled model with a native 2x ratio, so a 2x request is produced directly instead of resampled down from 4x.', 'BSD-3-Clause', 128, 2, TRUE),
    ('RealESRGANAnimeVideo', 'Anime Video / Line Art', 'realesr-animevideov3 — SRVGGNetCompact trained on anime video frames, light, tuned for compression artifacts and flat line work.', 'BSD-3-Clause', 128, 4, TRUE)
ON CONFLICT (model_name) DO UPDATE SET
    display_name = EXCLUDED.display_name,
    description = EXCLUDED.description,
    license = EXCLUDED.license,
    scale_factor = EXCLUDED.scale_factor;

-- ---------------------------------------------------------------------------
-- Telemetry (added 2026-09-22)
-- ---------------------------------------------------------------------------
--
-- Run-context columns on upscale_history. The v3.26.13-v3.26.17 "weak
-- upscale" regression (an output pixel budget being applied to the model's
-- *input*, so a 12MP photo reached the model at ~1MP) was invisible in this
-- log: source/output dimensions both looked right, because the shrink
-- happened in between. model_input_width/height record what the model
-- actually saw, which is the number that would have shown it immediately.
-- Thermal/memory columns are here for the other half of that story — the
-- regression was introduced as a thermal fix, with no data on whether
-- thermal pressure was actually occurring.
--
-- ADD COLUMN IF NOT EXISTS, not a new table: this file is re-run on every
-- boot (see db.py's init_db) against a live database with existing rows.
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS session_id VARCHAR(36);
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS detail_level VARCHAR(20);
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS model_input_width INT;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS model_input_height INT;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS requested_scale INT;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS upscale_strength REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS anti_aliasing REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS sharpen REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS denoise_before BOOLEAN;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS was_batch BOOLEAN;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS cancelled BOOLEAN;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS thermal_state_start VARCHAR(20);
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS thermal_state_end VARCHAR(20);
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS low_power_mode BOOLEAN;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS battery_level REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS physical_memory_mb INT;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS peak_memory_mb INT;
CREATE INDEX IF NOT EXISTS idx_history_model_detail ON upscale_history (model_name, detail_level, created_at);

-- Same idea for the action log: which session an action belongs to, how it
-- turned out, and how long it took, without having to parse them back out
-- of the free-form `detail` JSON every time.
ALTER TABLE action_log ADD COLUMN IF NOT EXISTS session_id VARCHAR(36);
ALTER TABLE action_log ADD COLUMN IF NOT EXISTS outcome VARCHAR(30);
ALTER TABLE action_log ADD COLUMN IF NOT EXISTS duration_ms INT;
ALTER TABLE action_log ADD COLUMN IF NOT EXISTS thermal_state VARCHAR(20);
CREATE INDEX IF NOT EXISTS idx_action_name ON action_log (action, created_at);

-- Ambient device telemetry, sampled periodically and at app
-- foreground/background rather than tied to any one action — the "was the
-- phone already hot / low on memory / in Low Power Mode when this happened"
-- context that no per-action log can supply on its own.
CREATE TABLE IF NOT EXISTS device_snapshots (
    id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    session_id VARCHAR(36),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    reason VARCHAR(30),
    thermal_state VARCHAR(20),
    low_power_mode BOOLEAN,
    battery_level REAL,
    battery_state VARCHAR(20),
    physical_memory_mb INT,
    used_memory_mb INT,
    free_disk_mb INT,
    session_uptime_s INT,
    app_version VARCHAR(20),
    os_version VARCHAR(20),
    device_model VARCHAR(50)
);
CREATE INDEX IF NOT EXISTS idx_device_snapshots ON device_snapshots (device_id, created_at);

-- Temporary export storage: which uploads were automatic (every result,
-- when Temporary Cloud Save is on) vs. a deliberate one-off, so the two
-- can be told apart when reasoning about storage use. Expiry itself is
-- unchanged — expires_at + the cleanup loop already handle it.
ALTER TABLE image_exports ADD COLUMN IF NOT EXISTS is_auto BOOLEAN DEFAULT FALSE;
ALTER TABLE image_exports ADD COLUMN IF NOT EXISTS label VARCHAR(120);
ALTER TABLE image_imports ADD COLUMN IF NOT EXISTS is_auto BOOLEAN DEFAULT FALSE;
ALTER TABLE image_imports ADD COLUMN IF NOT EXISTS label VARCHAR(120);

-- ---------------------------------------------------------------------------
-- Per-model run telemetry (added 2026-10-04)
-- ---------------------------------------------------------------------------
--
-- `run_kind` is the column that was most conspicuously missing: Auto mode
-- and Compare Models both run EVERY bundled model over the full photo, and
-- each of those candidate runs went into upscale_history as a row
-- indistinguishable from a deliberate single upscale. So every aggregate
-- over this table — success_rate, avg_processing_ms, per-model timing —
-- silently mixed one user-requested upscale in with six sweep runs the user
-- never asked for individually, which both drowns the real signal and makes
-- a slow model look slow in a context nobody cares about. 'single' |
-- 'batch' | 'compare' | 'live'. `was_batch` is kept and still set for
-- compatibility with existing rows and queries.
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS run_kind VARCHAR(20);
-- Set on compare/auto-sweep rows, joining them to the model_comparisons row
-- below so one sweep's candidate runs can be pulled back out together.
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS comparison_id VARCHAR(36);
-- These four were already being computed per run on the client and then
-- thrown away (see UpscaleResult) — the fidelity pass reports its PSNR, the
-- duplicate-tile cache counts its hits, and the memory guard records when it
-- had to shrink the output. All three are exactly the "what did this run
-- actually do" signal the log was missing.
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS fidelity_psnr REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS tiles_reused INT;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS output_was_capped BOOLEAN;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS render_denoise_applied BOOLEAN;
-- The estimated noise level that DECIDED whether the render-denoise pass
-- ran at all. Without it, `render_denoise_applied` says what happened but
-- not why, so a bad threshold is invisible.
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS source_noise_sigma REAL;
ALTER TABLE upscale_history ADD COLUMN IF NOT EXISTS free_disk_mb INT;
CREATE INDEX IF NOT EXISTS idx_history_run_kind ON upscale_history (run_kind, model_name, created_at);
CREATE INDEX IF NOT EXISTS idx_history_comparison ON upscale_history (comparison_id);

-- One row per Compare Models / Auto sweep. The individual candidate runs
-- still land in upscale_history (joined via comparison_id); this table is
-- the sweep itself — what was compared, what the heuristic picked, and,
-- crucially, what the USER picked afterwards.
--
-- That last column is the only ground truth this app has about model
-- quality. Every other signal is a proxy: sharpness_score is a heuristic,
-- and the "is this model any good" question was previously answerable only
-- by guessing. Which result a human chose when shown all of them side by
-- side is the actual answer, and it was not being recorded anywhere.
--
-- Unlike upscale_history this table is NOT append-only — picked_model is
-- filled in later, when the user taps a result in ModelComparisonView
-- (which can be well after the sweep finished, or never, if they back out
-- without choosing; a NULL picked_model with a non-NULL finished_at is
-- itself a meaningful outcome worth being able to count).
CREATE TABLE IF NOT EXISTS model_comparisons (
    comparison_id VARCHAR(36) PRIMARY KEY,
    device_id VARCHAR(64) NOT NULL,
    session_id VARCHAR(36),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    source_width INT,
    source_height INT,
    candidate_count INT,
    succeeded_count INT,
    cancelled BOOLEAN DEFAULT FALSE,
    total_ms INT,
    -- The app's own ranking winner, stored next to the user's actual pick
    -- so the two can be compared directly. For an interactive Compare
    -- Models sweep this is the highest-sharpness_score candidate (the
    -- metric the comparison UI itself ranks and badges on); for a batch
    -- auto-pick it is the content-affinity heuristic's choice (see
    -- UpscalerProvider.autoSelectModel). If this disagrees with
    -- picked_model often, the ranking is not measuring what people prefer.
    auto_pick_model VARCHAR(100),
    picked_model VARCHAR(100),
    picked_at TIMESTAMP,
    source_noise_sigma REAL,
    thermal_state_start VARCHAR(20),
    thermal_state_end VARCHAR(20),
    app_version VARCHAR(20),
    os_version VARCHAR(20),
    device_model VARCHAR(50)
);
CREATE INDEX IF NOT EXISTS idx_comparisons_device ON model_comparisons (device_id, created_at);
CREATE INDEX IF NOT EXISTS idx_comparisons_picked ON model_comparisons (picked_model, created_at);

-- One row per candidate per sweep: the per-model detail that makes
-- "which model wins, and by how much" a single query instead of a join
-- against upscale_history plus a reconstruction of the ordering.
CREATE TABLE IF NOT EXISTS model_comparison_candidates (
    id VARCHAR(36) PRIMARY KEY,
    comparison_id VARCHAR(36) NOT NULL REFERENCES model_comparisons(comparison_id) ON DELETE CASCADE,
    model_name VARCHAR(100) NOT NULL,
    candidate_index INT NOT NULL,
    succeeded BOOLEAN NOT NULL,
    processing_ms INT,
    -- The variance-of-Laplacian crop score Compare Models ranks by (see
    -- UpscalerProvider.sharpnessScore). Recorded per candidate so the
    -- heuristic's ranking can be checked against the user's pick instead
    -- of being trusted.
    sharpness_score REAL,
    fidelity_psnr REAL,
    tile_count INT,
    tiles_reused INT,
    render_denoise_applied BOOLEAN,
    error_message TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_candidates_comparison ON model_comparison_candidates (comparison_id, candidate_index);
CREATE INDEX IF NOT EXISTS idx_candidates_model ON model_comparison_candidates (model_name, created_at);

-- PixelBoost 5 (Rust engine): the bundled .pbnn models. The Core ML era
-- entries stay in the table (old history rows reference them) but are no
-- longer active.
INSERT INTO model_registry (model_name, display_name, description, license, tile_size, scale_factor, is_active) VALUES
    ('anime', 'Anime & Illustration', 'RRDBNet 6 blocks (Real-ESRGAN anime_6B weights) on the PixelBoost Rust engine.', 'BSD-3-Clause', 128, 4, TRUE),
    ('photo', 'Photo & Render', 'RRDBNet 23 blocks (Real-ESRGAN x4plus weights) on the PixelBoost Rust engine.', 'BSD-3-Clause', 128, 4, TRUE),
    ('clean', 'Clean / Faithful', 'RRDBNet 23 blocks (RealESRNet x4plus weights), no GAN texture.', 'BSD-3-Clause', 128, 4, TRUE),
    ('fast', 'Fast General', 'SRVGGNetCompact 32 convs (realesr-general-x4v3), blendable denoise weights.', 'BSD-3-Clause', 128, 4, TRUE),
    ('native2x', 'Native 2x', 'RRDBNet 23 blocks with pixel-unshuffle input (x2plus).', 'BSD-3-Clause', 128, 2, TRUE),
    ('lineart', 'Anime Video / Line Art', 'SRVGGNetCompact 16 convs (realesr-animevideov3).', 'BSD-3-Clause', 128, 4, TRUE)
ON CONFLICT (model_name) DO UPDATE SET
    display_name = EXCLUDED.display_name,
    description = EXCLUDED.description,
    license = EXCLUDED.license,
    scale_factor = EXCLUDED.scale_factor,
    is_active = TRUE;
UPDATE model_registry SET is_active = FALSE
WHERE model_name IN ('RealESRGAN', 'RealESRGANAnime', 'RealESRNet', 'RealESRGeneralV3', 'BSRGAN', 'RealCUGAN', 'RealESRGANx2', 'RealESRGANAnimeVideo');
