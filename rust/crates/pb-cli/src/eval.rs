//! Test bench: take real images, make a ground-truth crop, shrink it 4x (and
//! optionally JPEG it), upscale it back with every model / post-processing
//! combination, and score each against the ground truth.

use crate::{flag, models_dir};
use pb_core::enhance::sharpen_detail;
use pb_core::fidelity::{refine, Fidelity};
use pb_core::image::Format;
use pb_core::metrics::{edge_similarity, gradient_energy, psnr, ssim};
use pb_core::pipeline::{upscale, Settings};
use pb_core::resample::{resize, shrink_box, Filter};
use pb_core::{models, Engine, Image, TileOptions};
use std::collections::BTreeMap;
use std::time::Instant;

pub fn upscale_cmd(args: &[String]) -> Result<(), String> {
    let dir = models_dir(args);
    let model = flag(args, "--model").unwrap_or("auto");
    let img = Image::open(&args[1])?;
    let model = if model == "auto" { pb_core::classify::pick_model(&img).to_string() } else { model.to_string() };
    let eng = Engine::load(&dir, &model, flag(args, "--denoise").and_then(|v| v.parse().ok()))?;
    let mut st = Settings::default();
    if let Some(s) = flag(args, "--scale").and_then(|v| v.parse().ok()) {
        st.scale = s;
    }
    if let Some(f) = flag(args, "--fidelity").and_then(Fidelity::parse) {
        st.fidelity = f;
    }
    if let Some(d) = flag(args, "--detail").and_then(|v| v.parse().ok()) {
        st.detail = d;
    }
    if let Some(t) = flag(args, "--tile").and_then(|v| v.parse().ok()) {
        st.tile = t;
    }
    let (out, rep) = upscale(&eng, &img, &st, &mut |e| {
        eprint!("\r  {} tile {}/{}   ", model, e.done, e.total);
        true
    })
    .ok_or("cancelled")?;
    eprintln!();
    out.save(&args[2])?;
    println!("{}", serde_json::to_string_pretty(&rep).unwrap());
    Ok(())
}

/// Most detailed `n`x`n` window (by gradient energy) on a coarse grid.
fn best_crop(img: &Image, n: usize) -> (usize, usize) {
    let mut best = (0, 0, -1.0);
    let step = (n / 2).max(16);
    let mut y = 0;
    while y + n <= img.h {
        let mut x = 0;
        while x + n <= img.w {
            let g = gradient_energy(&img.crop(x, y, n, n));
            if g > best.2 {
                best = (x, y, g);
            }
            x += step;
        }
        y += step;
    }
    (best.0, best.1)
}

struct Post {
    name: &'static str,
    fidelity: Fidelity,
    detail: f32,
}

const POSTS: &[Post] = &[
    Post { name: "raw", fidelity: Fidelity::Off, detail: 0.0 },
    Post { name: "nat", fidelity: Fidelity::Natural, detail: 0.0 },
    Post { name: "nat+d", fidelity: Fidelity::Natural, detail: 0.35 },
    Post { name: "faith", fidelity: Fidelity::Faithful, detail: 0.0 },
    Post { name: "faith+d", fidelity: Fidelity::Faithful, detail: 0.35 },
];

#[derive(Default, Clone)]
struct Score {
    psnr: f64,
    ssim: f64,
    edge: f64,
    grad: f64,
    secs: f64,
    n: usize,
}

pub fn eval_cmd(args: &[String]) -> Result<(), String> {
    let dir = models_dir(args);
    let src_dir = &args[1];
    let out_dir = &args[2];
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let crop: usize = flag(args, "--crop").and_then(|v| v.parse().ok()).unwrap_or(256);
    let long: usize = flag(args, "--long").and_then(|v| v.parse().ok()).unwrap_or(1600);
    let jpeg: Option<u8> = flag(args, "--jpeg").and_then(|v| v.parse().ok());
    let only: Option<Vec<String>> = flag(args, "--only").map(|s| s.split(',').map(String::from).collect());
    let list: Option<Vec<String>> = flag(args, "--list").map(|s| s.split(',').map(String::from).collect());
    let ids: Vec<&str> = models::MODELS.iter().map(|m| m.id).filter(|id| *id != "fast-denoise" && only.as_ref().map_or(true, |o| o.iter().any(|x| x == id))).collect();
    let engines: Vec<Engine> = ids.iter().map(|id| Engine::load(&dir, id, None)).collect::<Result<_, _>>()?;

    let mut files: Vec<_> = std::fs::read_dir(src_dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    if let Some(l) = &list {
        files.retain(|p| l.iter().any(|n| p.file_stem().map_or(false, |s| s.to_string_lossy() == *n)));
    }
    let mut totals: BTreeMap<String, Score> = BTreeMap::new();
    let mut per_image = String::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let Ok(full) = Image::open(&path.to_string_lossy()) else { continue };
        // Bring the picture to a size where it is genuinely sharp.
        let f = long as f32 / full.w.max(full.h) as f32;
        let base = if f < 1.0 { resize(&full, (full.w as f32 * f).round() as usize, (full.h as f32 * f).round() as usize, Filter::Lanczos3) } else { full };
        let n = crop.min(base.w).min(base.h) / 4 * 4;
        let (cx, cy) = best_crop(&base, n);
        let gt = base.crop(cx, cy, n, n);
        let mut lr = shrink_box(&gt, 4);
        if let Some(q) = jpeg {
            let bytes = lr.encode(Format::Jpeg, q)?;
            lr = Image::decode(&bytes)?;
        }
        let ggt = gradient_energy(&gt);
        let mut sheet: Vec<(String, Image)> = vec![("gt".into(), gt.clone())];
        let mut record = |label: String, img: &Image, secs: f64, totals: &mut BTreeMap<String, Score>, sheet: &mut Vec<(String, Image)>| {
            let s = Score { psnr: psnr(&gt, img, 4), ssim: ssim(&gt, img), edge: edge_similarity(&gt, img), grad: gradient_energy(img) / ggt, secs, n: 1 };
            per_image.push_str(&format!("{name:<8} {label:<18} psnr {:6.2}  ssim {:.4}  edge {:.4}  grad {:.2}  {:.2}s\n", s.psnr, s.ssim, s.edge, s.grad, s.secs));
            let t = totals.entry(label.clone()).or_default();
            t.psnr += s.psnr;
            t.ssim += s.ssim;
            t.edge += s.edge;
            t.grad += s.grad;
            t.secs += s.secs;
            t.n += 1;
            sheet.push((label, img.clone()));
        };
        for (lbl, f) in [("bicubic", Filter::CatmullRom), ("lanczos", Filter::Lanczos3)] {
            let t = Instant::now();
            let r = resize(&lr, n, n, f);
            record(lbl.into(), &r, t.elapsed().as_secs_f64(), &mut totals, &mut sheet);
        }
        for eng in &engines {
            let t = Instant::now();
            let raw = eng.run(&lr, TileOptions { tile: 128, pad: 16 }, &mut |_| true).unwrap();
            let secs = t.elapsed().as_secs_f64();
            let raw = if raw.w != n { resize(&raw, n, n, Filter::Lanczos3) } else { raw };
            for p in POSTS {
                let mut img = raw.clone();
                let s = n / lr.w;
                if s * lr.w == n {
                    if let Some(fp) = p.fidelity.params() {
                        refine(&lr, &mut img, s, fp);
                    }
                }
                sharpen_detail(&mut img, p.detail, 1.0);
                record(format!("{}:{}", eng.info.id, p.name), &img, secs, &mut totals, &mut sheet);
            }
        }
        // Contact sheet: one row per image, columns in `sheet` order.
        let cols = sheet.len();
        let mut big = Image::new(cols * (n + 4), n);
        big.px.iter_mut().for_each(|p| *p = [1.0, 1.0, 1.0, 1.0]);
        for (i, (_, im)) in sheet.iter().enumerate() {
            big.paste(im, i * (n + 4), 0);
        }
        big.save(&format!("{out_dir}/{name}_sheet.png"))?;
        lr.save(&format!("{out_dir}/{name}_lr.png"))?;
        let labels: Vec<_> = sheet.iter().map(|(l, _)| l.clone()).collect();
        std::fs::write(format!("{out_dir}/sheet_columns.txt"), labels.join("\n")).ok();
        eprintln!("{name} done");
    }
    let mut table = String::from("variant              psnr    ssim    edge    grad   secs\n");
    for (k, s) in &totals {
        let n = s.n as f64;
        table.push_str(&format!("{k:<18} {:6.2}  {:.4}  {:.4}  {:.2}  {:.2}\n", s.psnr / n, s.ssim / n, s.edge / n, s.grad / n, s.secs / n));
    }
    std::fs::write(format!("{out_dir}/per_image.txt"), &per_image).ok();
    std::fs::write(format!("{out_dir}/summary.txt"), &table).ok();
    print!("{table}");
    Ok(())
}
