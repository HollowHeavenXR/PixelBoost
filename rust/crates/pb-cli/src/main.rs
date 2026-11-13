//! `pixelboost` — command line front end for the engine and the test bench.

mod eval;

use pb_core::{models, Engine, Image, TileOptions};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "pixelboost <command>
  convert <in.pth> <out.pbnn>          convert a PyTorch checkpoint
  convert-all <pth dir> <models dir>   convert every bundled model's checkpoint
  models                               list bundled models
  upscale <in> <out> [--model id] [--scale s] [--fidelity off|natural|faithful]
          [--detail 0..1] [--denoise 0..1] [--tile n] [--models dir]
  raw <model> <in> <out>               model output only, no post-processing
  bench <model> [size]                 time one forward pass
  eval <image dir> <out dir> [--models dir] [--crop n] [--list a,b] [--only ids]
                                       degrade -> upscale -> compare with original
  edit <in> <out> <op json | @file.json | preset:id>   run an editing op (timed)
  presets                              list built-in presets
  analyze <images...> [--gflops g] [--seconds s] [--cloud] [--degrade soft|noise|jpeg]
                                       measure photos, print the smart settings
  replace <original> <result> <out base>   result + original's EXIF/ICC (JPEG/PNG)
  shape <in> <out.png> <shape> [ring json]  shape crop with rings"
    );
    std::process::exit(2)
}

pub fn models_dir(args: &[String]) -> PathBuf {
    flag(args, "--models").map(PathBuf::from).unwrap_or_else(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        for c in [PathBuf::from("models"), PathBuf::from("../models"), exe.parent().unwrap_or(Path::new(".")).join("../../models")] {
            if c.join("anime-x4.pbnn").exists() {
                return c;
            }
        }
        PathBuf::from("models")
    })
}

pub fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn convert(src: &Path, dst: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let sd = pb_nn::pth::load_pth(&bytes).map_err(|e| e.to_string())?;
    let (arch, cfg) = pb_nn::arch::detect(&sd)?;
    let params: usize = sd.iter().map(|(_, t)| t.data.len()).sum();
    let meta = serde_json::json!({
        "source": src.file_name().unwrap().to_string_lossy(),
        "params": params,
        "license": "BSD-3-Clause (Real-ESRGAN, Xintao Wang et al.)",
    });
    let out = pb_nn::pbm::write(&arch, cfg.clone(), meta, &sd);
    // Round-trip check: the converted file must build.
    let w = pb_nn::pbm::read(&out)?;
    pb_nn::build(&w)?;
    std::fs::write(dst, &out).map_err(|e| e.to_string())?;
    println!("{} -> {} ({arch} {cfg}, {params} params, {:.1} MB)", src.display(), dst.display(), out.len() as f64 / 1e6);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let res = match args[0].as_str() {
        "convert" if args.len() >= 3 => convert(Path::new(&args[1]), Path::new(&args[2])),
        "convert-all" if args.len() >= 3 => {
            let (src, dst) = (Path::new(&args[1]), Path::new(&args[2]));
            std::fs::create_dir_all(dst).ok();
            models::MODELS.iter().try_for_each(|m| convert(&src.join(m.source), &dst.join(m.file)))
        }
        "models" => {
            for m in models::MODELS {
                println!("{:<13} x{}  cost {:>5.2}  {:<28} {}", m.id, m.scale, m.cost, m.name, m.best_for);
            }
            Ok(())
        }
        "raw" if args.len() >= 4 => (|| {
            let dir = models_dir(&args);
            let eng = Engine::load(&dir, &args[1], flag(&args, "--denoise").and_then(|v| v.parse().ok()))?;
            let img = Image::open(&args[2])?;
            let t = Instant::now();
            let tile = flag(&args, "--tile").and_then(|v| v.parse().ok()).unwrap_or(128);
            let out = eng
                .run(&img, TileOptions { tile, pad: 16 }, &mut |e| {
                    eprint!("\r  tile {}/{}", e.done, e.total);
                    true
                })
                .unwrap();
            eprintln!();
            println!("{}x{} -> {}x{} in {:.2}s", img.w, img.h, out.w, out.h, t.elapsed().as_secs_f64());
            out.save(&args[3])
        })(),
        "upscale" if args.len() >= 3 => eval::upscale_cmd(&args),
        "bench" if args.len() >= 2 => (|| {
            let dir = models_dir(&args);
            let eng = Engine::load(&dir, &args[1], None)?;
            let n: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(64);
            let mut img = Image::new(n, n);
            for (i, p) in img.px.iter_mut().enumerate() {
                let (x, y) = ((i % n) as f32, (i / n) as f32);
                *p = [(x * 0.1).sin() * 0.5 + 0.5, (y * 0.07).cos() * 0.5 + 0.5, ((x + y) * 0.05).sin() * 0.5 + 0.5, 1.0];
            }
            println!("kernel: {}", pb_nn::conv::kernel_name());
            for _ in 0..2 {
                let t = Instant::now();
                eng.run(&img, TileOptions { tile: n, pad: 0 }, &mut |_| true);
                let dt = t.elapsed().as_secs_f64();
                let gflop = eng.macs_per_px() * (n * n) as f64 * 2.0 / 1e9;
                println!("{}: {n}x{n} in {dt:.3}s  ({gflop:.1} GFLOP, {:.1} GFLOP/s)", args[1], gflop / dt);
            }
            Ok(())
        })(),
        "eval" if args.len() >= 3 => eval::eval_cmd(&args),
        "edit" if args.len() >= 4 => (|| {
            let spec = &args[3];
            let json = if let Some(id) = spec.strip_prefix("preset:") {
                pb_core::presets::BUILTIN.iter().find(|b| b.id == id).map(|b| b.op.to_string()).ok_or(format!("no preset {id}"))?
            } else if let Some(f) = spec.strip_prefix('@') {
                std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?
            } else {
                spec.clone()
            };
            let op = pb_core::ops::parse(&json)?;
            let img = Image::open(&args[1])?;
            let t = Instant::now();
            let out = pb_core::ops::apply(&img, &op)?;
            println!("{}x{} -> {}x{} in {:.0} ms", img.w, img.h, out.w, out.h, t.elapsed().as_secs_f64() * 1000.0);
            out.save(&args[2])
        })(),
        "presets" => {
            for b in pb_core::presets::BUILTIN {
                println!("{:<16} {}", b.id, b.name);
            }
            Ok(())
        }
        "analyze" if args.len() >= 2 => analyze_cmd(&args),
        "replace" if args.len() >= 4 => (|| {
            let bytes = std::fs::read(&args[1]).map_err(|e| e.to_string())?;
            let m = pb_core::meta::read(&bytes);
            let (w, h, ext) = pb_core::meta::write_with_meta(&args[2], &format!("{}.tmp", args[3]), &m, 95, true)?;
            std::fs::rename(format!("{}.tmp", args[3]), format!("{}.{ext}", args[3])).map_err(|e| e.to_string())?;
            println!("{w}x{h} {ext}  exif: {}  icc: {}", m.exif.as_ref().map(|e| e.len()).unwrap_or(0), m.icc.as_ref().map(|e| e.len()).unwrap_or(0));
            Ok(())
        })(),
        "shape" if args.len() >= 4 => (|| {
            let rings = args.get(4).map(|s| s.as_str()).unwrap_or("[]");
            let json = format!(r#"{{"op":"shape_crop","shape":"{}","rings":{rings}}}"#, args[3]);
            let op = pb_core::ops::parse(&json)?;
            let img = Image::open(&args[1])?;
            let t = Instant::now();
            let out = pb_core::ops::apply(&img, &op)?;
            println!("{}x{} -> {}x{} in {:.0} ms", img.w, img.h, out.w, out.h, t.elapsed().as_secs_f64() * 1000.0);
            out.save(&args[2])
        })(),
        "classify" if args.len() >= 2 => args[1..].iter().try_for_each(|f| {
            let img = Image::open(f)?;
            let s = pb_core::classify::stats(&img);
            println!("{f}: {} {:?}", pb_core::classify::pick_model(&img), s);
            Ok(())
        }),
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn degrade(img: &Image, how: &str) -> Image {
    use pb_core::resample::{resize, Filter};
    match how {
        "soft" => {
            let s = resize(img, (img.w / 3).max(8), (img.h / 3).max(8), Filter::Box);
            resize(&s, img.w, img.h, Filter::CatmullRom)
        }
        "noise" => {
            let mut o = img.clone();
            for (i, p) in o.px.iter_mut().enumerate() {
                // Approximately Gaussian (sum of 4 uniforms), sigma ~ 8/255.
                let n: f32 = (0..4).map(|k| pb_core::filters::hash_noise((i % img.w) as u32, (i / img.w) as u32, k)).sum::<f32>() - 2.0;
                for c in 0..3 {
                    p[c] = (p[c] + n * 0.054).clamp(0.0, 1.0);
                }
            }
            o
        }
        "jpeg" => {
            let bytes = img.encode(pb_core::image::Format::Jpeg, 20).unwrap();
            Image::decode(&bytes).unwrap()
        }
        _ => img.clone(),
    }
}

fn analyze_cmd(args: &[String]) -> Result<(), String> {
    let b = pb_core::analyze::Budget {
        gflops: flag(args, "--gflops").and_then(|v| v.parse().ok()).unwrap_or(40.0),
        max_seconds: flag(args, "--seconds").and_then(|v| v.parse().ok()).unwrap_or(180.0),
        max_output_mp: 120.0,
        cloud: args.iter().any(|a| a == "--cloud"),
    };
    let how = flag(args, "--degrade").unwrap_or("");
    let verbose = args.iter().any(|a| a == "-v");
    let skip: Vec<&str> = ["--gflops", "--seconds", "--degrade"].iter().filter_map(|f| flag(args, f)).collect();
    for f in args[1..].iter().filter(|a| !a.starts_with('-') && !skip.contains(&a.as_str())) {
        let t = Instant::now();
        let m = if how.is_empty() { pb_core::analyze::measure_file(f)? } else { pb_core::analyze::measure(&degrade(&Image::open(f)?, how)) };
        let r = pb_core::analyze::recommend(&m, b);
        let name = Path::new(f).file_name().unwrap().to_string_lossy();
        println!(
            "{name:<10} {:>4}x{:<4} {:<5} noise {:>5.2} block {:.2} sharp {:.3} skin {:.2} | {} {}x {} d{:.2} dn{:.2} e{} ~{:.0}s  ({:.0} ms)",
            m.width, m.height, m.kind, m.noise, m.blocking, m.sharpness, m.skin, r.model, r.scale, r.fidelity, r.detail, r.denoise, r.ensemble, r.est_seconds, t.elapsed().as_secs_f64() * 1000.0
        );
        if verbose {
            for w in &r.reasons {
                println!("    - {w}");
            }
        }
    }
    Ok(())
}
