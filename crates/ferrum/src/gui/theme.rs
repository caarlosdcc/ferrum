/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Paleta, tipografía y estilo de la interfaz.
//!
//! Identidad de Ferrum ("hierro"): grafito oscuro con acento color óxido.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, vec2,
};

/// Fondo de la barra de pestañas (la parte más oscura).
pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0d, 0x11);
/// Pestaña activa y barra de herramientas.
pub const SURFACE: Color32 = Color32::from_rgb(0x17, 0x1a, 0x21);
/// Pestaña inactiva bajo el ratón.
pub const TAB_HOVER: Color32 = Color32::from_rgb(0x12, 0x15, 0x1a);
/// Barra de direcciones.
pub const FIELD: Color32 = Color32::from_rgb(0x0e, 0x10, 0x15);
pub const FIELD_HOVER: Color32 = Color32::from_rgb(0x11, 0x14, 0x1a);
pub const BORDER: Color32 = Color32::from_rgb(0x25, 0x2a, 0x33);
pub const BORDER_HOVER: Color32 = Color32::from_rgb(0x35, 0x3b, 0x46);
/// Fondo de un botón bajo el ratón (blanco translúcido, premultiplicado).
pub const HOVER: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
pub const PRESSED: Color32 = Color32::from_rgba_premultiplied(34, 34, 34, 34);
pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xea, 0xe7);
pub const MUTED: Color32 = Color32::from_rgb(0x92, 0x99, 0xa3);
pub const FAINT: Color32 = Color32::from_rgb(0x55, 0x5c, 0x66);
/// Color óxido: la marca de Ferrum.
pub const ACCENT: Color32 = Color32::from_rgb(0xe0, 0x70, 0x3a);
/// Óxido translúcido (16 %), para pastillas y selecciones.
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(36, 18, 9, 41);
pub const SECURE: Color32 = Color32::from_rgb(0x5c, 0xc4, 0x89);
pub const SECURE_SOFT: Color32 = Color32::from_rgba_premultiplied(15, 31, 22, 41);
pub const WARNING: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x4c);
pub const WARNING_SOFT: Color32 = Color32::from_rgba_premultiplied(38, 29, 12, 41);
/// Pastilla «Agente»: un azul frío, distinto del óxido de la marca, para que
/// se note que la ventana la controla un agente de IA.
pub const AGENT: Color32 = Color32::from_rgb(0x7c, 0xa8, 0xff);
pub const AGENT_SOFT: Color32 = Color32::from_rgba_premultiplied(20, 27, 41, 41);
/// Botón de cerrar la ventana bajo el ratón (el rojo de Windows).
pub const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);
pub const MESSAGE_BG: Color32 = Color32::from_rgb(0x26, 0x20, 0x14);

/// Tamaño del texto de las pestañas.
pub const TAB_FONT: f32 = 13.0;
/// Tamaño del texto de la barra de direcciones.
pub const ADDRESS_FONT: f32 = 14.0;

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn apply(ctx: &Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals(egui::Visuals::dark());
    ctx.all_styles_mut(|style| {
        // Los textos de la interfaz no se seleccionan al hacer clic.
        style.interaction.selectable_labels = false;
        style.text_styles.insert(TextStyle::Body, font(14.0));
        style.text_styles.insert(TextStyle::Button, font(14.0));
        style.text_styles.insert(TextStyle::Small, font(12.0));

        let visuals = &mut style.visuals;
        visuals.override_text_color = Some(TEXT);
        visuals.panel_fill = BG;
        visuals.window_fill = SURFACE;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(10);
        visuals.menu_corner_radius = CornerRadius::same(10);
        visuals.popup_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        visuals.extreme_bg_color = FIELD;
        visuals.selection.bg_fill = ACCENT.linear_multiply(0.55);
        visuals.selection.stroke = Stroke::new(1.0, TEXT);
        visuals.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        visuals.text_cursor.blink = false;
        visuals.hyperlink_color = ACCENT;

        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = CornerRadius::same(7);
        }
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x22, 0x26, 0x2e);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2b, 0x30, 0x3a);
        visuals.widgets.active.weak_bg_fill = Color32::from_rgb(0x33, 0x39, 0x44);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_HOVER);

        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.item_spacing = vec2(6.0, 4.0);
        style.spacing.interact_size = vec2(28.0, 26.0);
    });
}

/// Tipografía: la del sistema para que Ferrum se sienta nativo (Segoe UI en
/// Windows, San Francisco en macOS, Noto/DejaVu en Linux). Las fuentes de egui
/// quedan detrás para los símbolos y emojis que falten.
///
/// Solo se lee la fuente del disco; no se copia ni se distribuye con Ferrum.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    if let Some(bytes) = system_font_candidates()
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
    {
        fonts
            .font_data
            .insert("sistema".to_owned(), Arc::new(FontData::from_owned(bytes)));
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "sistema".to_owned());
        }
    } else {
        log::debug!("Sin fuente del sistema: se usa la de egui");
    }
    fonts
}

fn system_font_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let windir =
            std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let fonts = windir.join("Fonts");
        vec![fonts.join("segoeui.ttf"), fonts.join("SegUIVar.ttf")]
    } else if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/System/Library/Fonts/SFNS.ttf"),
            PathBuf::from("/System/Library/Fonts/Supplemental/Arial.ttf"),
        ]
    } else {
        [
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }
}
