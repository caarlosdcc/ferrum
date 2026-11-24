/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Logotipo de Ferrum: hexágono color óxido (una tuerca: hierro, solidez) con
//! una "F" de hierro oscuro. Se dibuja por código, sin archivos de imagen.
//!
//! La misma geometría está en `scripts/generate-icons.py` (icono del .exe) y en
//! el SVG de la página de inicio (`pages.rs`): si cambias una, cambia las tres.

use winit::window::Icon;

/// Radio del hexágono, en fracción del lado del icono.
const HEX_RADIUS: f32 = 0.47;
/// Rectángulos que forman la "F": (x0, y0, x1, y1).
const LETTER: [(f32, f32, f32, f32); 3] = [
    (0.36, 0.27, 0.47, 0.73),
    (0.36, 0.27, 0.66, 0.37),
    (0.36, 0.455, 0.60, 0.545),
];
const TOP: [f32; 3] = [242.0, 140.0, 80.0];
const BOTTOM: [f32; 3] = [196.0, 80.0, 29.0];
const IRON: [f32; 3] = [21.0, 23.0, 28.0];

/// Icono de la ventana y de la barra de tareas.
pub fn window_icon() -> Option<Icon> {
    const SIZE: u32 = 64;
    Icon::from_rgba(render(SIZE), SIZE, SIZE).ok()
}

/// RGBA (alfa sin premultiplicar) de `size` × `size`, con suavizado 4×4.
pub fn render(size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let total = (SAMPLES * SAMPLES) as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut hex, mut letter) = (0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if in_hexagon(u, v) {
                        hex += 1;
                        if in_letter(u, v) {
                            letter += 1;
                        }
                    }
                }
            }
            let t = (y as f32 + 0.5) / size as f32;
            let base = mix(TOP, BOTTOM, t);
            let share = if hex > 0 {
                letter as f32 / hex as f32
            } else {
                0.0
            };
            let color = mix(base, IRON, share);
            rgba.extend(color.map(|c| c.round() as u8));
            rgba.push((hex as f32 / total * 255.0).round() as u8);
        }
    }
    rgba
}

/// Hexágono con un vértice arriba, centrado en (0.5, 0.5).
fn in_hexagon(u: f32, v: f32) -> bool {
    let dx = (u - 0.5).abs();
    let dy = (v - 0.5).abs();
    dx <= HEX_RADIUS * 0.866_025_4 && dy <= HEX_RADIUS - dx / 3f32.sqrt()
}

fn in_letter(u: f32, v: f32) -> bool {
    LETTER
        .iter()
        .any(|&(x0, y0, x1, y1)| (x0..=x1).contains(&u) && (y0..=y1).contains(&v))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_transparent_and_center_is_opaque() {
        let size = 32;
        let image = render(size);
        let alpha = |x: u32, y: u32| image[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(size - 1, size - 1), 0);
        assert_eq!(alpha(size / 2, size / 2), 255);
    }
}
