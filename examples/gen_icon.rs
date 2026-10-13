//! Génère l'icône de Quarry : une pile de disques (base de données) bleue
//! sur un carré arrondi sombre, aux proportions des icônes macOS.
//!
//! ```bash
//! cargo run --release --example gen_icon -- assets/icon.png 1024
//! ```

use std::fs::File;
use std::io::BufWriter;

type Rgba = [f32; 4];

const BG_TOP: Rgba = rgb(0x24, 0x2f, 0x4a);
const BG_BOTTOM: Rgba = rgb(0x0e, 0x11, 0x16);
const SIDE: Rgba = rgb(0x7a, 0xa2, 0xf7);
const TOP: Rgba = rgb(0xc0, 0xd2, 0xff);
const RIM: Rgba = rgb(0x4d, 0x6f, 0xc9);

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn shade(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

fn in_ellipse(x: f32, y: f32, cy: f32, rx: f32, ry: f32) -> bool {
    (x / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0
}

/// Carré arrondi centré, demi-côté `half`, rayon `r`.
fn in_rounded_square(x: f32, y: f32, half: f32, r: f32) -> bool {
    let qx = (x.abs() - (half - r)).max(0.0);
    let qy = (y.abs() - (half - r)).max(0.0);
    x.abs() <= half && y.abs() <= half && qx * qx + qy * qy <= r * r
}

/// Couleur d'un point en coordonnées normalisées [-1, 1] (y vers le bas).
fn sample(x: f32, y: f32) -> Rgba {
    // Grille macOS : contenu de 824 px sur 1024, rayon d'environ 185 px.
    if !in_rounded_square(x, y, 0.805, 0.36) {
        return [0.0; 4];
    }
    let mut color = mix(BG_TOP, BG_BOTTOM, (y + 0.805) / 1.61);

    // Trois disques, du bas vers le haut : le disque du dessus recouvre
    // le précédent et laisse voir un croissant de sa face supérieure.
    let (rx, ry, height, gap) = (0.46, 0.14, 0.20, 0.07);
    for i in (0..3).rev() {
        let top = -0.38 + i as f32 * (height + gap);
        let in_side = (x.abs() <= rx && y >= top && y <= top + height)
            || in_ellipse(x, y, top + height, rx, ry);
        if in_side {
            // Éclairage cylindrique : plus clair au centre, plus sombre aux bords.
            let light = (1.0 - (x / rx).powi(2)).max(0.0).sqrt();
            color = shade(SIDE, 0.62 + 0.38 * light);
        }
        if in_ellipse(x, y, top, rx, ry) {
            color = mix(TOP, SIDE, ((y - top) / ry + 1.0) / 2.0 * 0.35);
        }
        // Liseré au bord de la face supérieure.
        let d = (x / rx).powi(2) + ((y - top) / ry).powi(2);
        if (0.88..=1.0).contains(&d) && y > top {
            color = RIM;
        }
    }
    color
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "assets/icon.png".into());
    let size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1024);

    // Suréchantillonnage 4×4 pour l'anticrénelage.
    const SS: u32 = 4;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = ((px * SS + sx) as f32 + 0.5) / (size * SS) as f32 * 2.0 - 1.0;
                    let y = ((py * SS + sy) as f32 + 0.5) / (size * SS) as f32 * 2.0 - 1.0;
                    let c = sample(x, y);
                    // Accumulation prémultipliée par l'alpha.
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let n = (SS * SS) as f32;
            let alpha = acc[3] / n;
            for i in 0..3 {
                let v = if acc[3] > 0.0 { acc[i] / acc[3] } else { 0.0 };
                pixels.push((v * 255.0).round() as u8);
            }
            pixels.push((alpha * 255.0).round() as u8);
        }
    }

    let file = BufWriter::new(File::create(&path).expect("création du fichier"));
    let mut encoder = png::Encoder::new(file, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
    println!("{path} ({size}×{size})");
}
