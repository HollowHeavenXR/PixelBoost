//! Built-in presets: one-tap "recipes" made of portable ops (no files), so
//! they work on any photo and in batch.

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub op: &'static str,
}

pub const BUILTIN: &[Builtin] = &[
    Builtin {
        id: "clean-pop",
        name: "Clean Pop",
        op: r#"{"op":"chain","ops":[{"op":"auto_enhance"},{"op":"adjust","vibrance":0.35,"clarity":0.25,"texture":0.15,"whites":0.15,"blacks":-0.1}]}"#,
    },
    Builtin {
        id: "bright-airy",
        name: "Bright & Airy",
        op: r#"{"op":"chain","ops":[{"op":"adjust","exposure":0.35,"highlights":-0.4,"shadows":0.5,"contrast":0.92,"vibrance":0.15,"warmth":0.08,"fade":0.15}]}"#,
    },
    Builtin {
        id: "teal-orange",
        name: "Teal & Orange",
        op: r#"{"op":"chain","ops":[{"op":"adjust","contrast":1.1,"clarity":0.15},{"op":"grade","shadows":{"hue":195,"sat":0.55},"highlights":{"hue":32,"sat":0.45},"balance":0.1},{"op":"hsl","sat":[0.1,0.2,0,-0.3,-0.1,0.1,0,0]}]}"#,
    },
    Builtin {
        id: "moody-film",
        name: "Moody Film",
        op: r#"{"op":"chain","ops":[{"op":"adjust","exposure":-0.2,"contrast":1.08,"saturation":0.82,"fade":0.35,"grain":0.35,"grain_size":0.4,"vignette":0.3},{"op":"grade","shadows":{"hue":210,"sat":0.3},"highlights":{"hue":45,"sat":0.2}},{"op":"curves","master":[0.06,0.24,0.5,0.76,0.95]}]}"#,
    },
    Builtin {
        id: "golden-hour",
        name: "Golden Hour",
        op: r#"{"op":"chain","ops":[{"op":"adjust","warmth":0.45,"tint":0.1,"vibrance":0.25,"shadows":0.2},{"op":"grade","highlights":{"hue":38,"sat":0.5,"lum":0.05},"midtones":{"hue":28,"sat":0.2}},{"op":"bloom","amount":0.3,"radius":0.03,"threshold":0.75}]}"#,
    },
    Builtin {
        id: "cinematic",
        name: "Cinematic",
        op: r#"{"op":"chain","ops":[{"op":"adjust","contrast":1.12,"saturation":0.9,"clarity":0.2,"blacks":-0.15,"vignette":0.35},{"op":"grade","shadows":{"hue":185,"sat":0.45},"midtones":{"hue":200,"sat":0.1},"highlights":{"hue":36,"sat":0.35}},{"op":"curves","master":[0.04,0.22,0.5,0.8,0.97]}]}"#,
    },
    Builtin {
        id: "bw-contrast",
        name: "B&W Contrast",
        op: r#"{"op":"chain","ops":[{"op":"hsl","lum":[0.2,0.25,0.15,-0.2,-0.2,-0.35,-0.2,0.1]},{"op":"adjust","saturation":0,"contrast":1.2,"clarity":0.35,"whites":0.2,"blacks":-0.25,"grain":0.2}]}"#,
    },
    Builtin {
        id: "faded-matte",
        name: "Faded Matte",
        op: r#"{"op":"chain","ops":[{"op":"adjust","fade":0.55,"contrast":0.9,"saturation":0.85,"highlights":-0.2},{"op":"grade","shadows":{"hue":160,"sat":0.15},"highlights":{"hue":50,"sat":0.15}}]}"#,
    },
    Builtin {
        id: "vivid-landscape",
        name: "Vivid Landscape",
        op: r#"{"op":"chain","ops":[{"op":"adjust","dehaze":0.35,"clarity":0.3,"vibrance":0.45,"highlights":-0.35,"shadows":0.3},{"op":"hsl","hue":[0,0,-0.1,0.1,0,0,0,0],"sat":[0,0.1,0.15,0.2,0.2,0.25,0,0],"lum":[0,0,0,0,0,-0.15,0,0]}]}"#,
    },
    Builtin {
        id: "soft-portrait",
        name: "Soft Portrait",
        op: r#"{"op":"chain","ops":[{"op":"smooth","amount":0.35,"radius":0.006},{"op":"adjust","exposure":0.15,"warmth":0.1,"clarity":-0.15,"vibrance":0.1,"highlights":-0.15},{"op":"hsl","sat":[0,-0.1,0,0,0,0,0,0],"lum":[0,0.12,0,0,0,0,0,0]}]}"#,
    },
    Builtin {
        id: "neon-night",
        name: "Neon Night",
        op: r#"{"op":"chain","ops":[{"op":"adjust","contrast":1.15,"vibrance":0.5,"blacks":-0.2,"clarity":0.2},{"op":"grade","shadows":{"hue":250,"sat":0.5},"highlights":{"hue":320,"sat":0.35}},{"op":"bloom","amount":0.5,"radius":0.025,"threshold":0.65}]}"#,
    },
    Builtin {
        id: "vintage",
        name: "Vintage",
        op: r#"{"op":"chain","ops":[{"op":"adjust","warmth":0.3,"saturation":0.75,"fade":0.4,"grain":0.45,"grain_size":0.6,"vignette":0.45},{"op":"curves","red":[0.05,0.3,0.55,0.8,0.97],"blue":[0.12,0.3,0.48,0.68,0.85]},{"op":"effect","name":"light_leak","amount":0.35,"size":0.5,"colors":[[1.0,0.45,0.2],[1.0,0.75,0.4]]}]}"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Image;
    #[test]
    fn builtins_parse_run_and_are_portable() {
        let mut img = Image::new(40, 30);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [(i % 40) as f32 / 39.0, (i / 40) as f32 / 29.0, 0.4, 1.0];
        }
        for b in BUILTIN {
            let op = crate::ops::parse(b.op).unwrap_or_else(|e| panic!("{}: {e}", b.id));
            assert!(!op.uses_files(), "{}", b.id);
            let o = crate::ops::apply(&img, &op).unwrap();
            assert!(o.px.iter().all(|p| p.iter().all(|v| v.is_finite())), "{}", b.id);
        }
    }
}
