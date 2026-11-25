/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Controles que una página pide mostrar: listas desplegables, selector de
//! color y menú de clic derecho.

use egui::{
    Align, Align2, Button, Color32, CornerRadius, Label, Layout, RichText, ScrollArea, Sense,
    Stroke, Ui, pos2, vec2,
};
use servo::{
    ContextMenu, ContextMenuAction, ContextMenuItem, SelectElement, SelectElementOption,
    SelectElementOptionOrOptgroup,
};

use super::icons::{self, Icon};
use super::theme::{self, BORDER, FAINT, HOVER, MUTED, PRESSED, TEXT};

/// Ancho del menú de clic derecho.
pub const MENU_WIDTH: f32 = 264.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Open,
    Submit,
    Cancel,
    /// Una acción del menú de clic derecho.
    Choose(ContextMenuAction),
}

pub fn select_list(ui: &mut Ui, select: &mut SelectElement) -> Outcome {
    let multiple = select.allow_select_multiple();
    let mut selected = select.selected_options();
    let mut chosen = None;

    ui.spacing_mut().item_spacing.y = 2.0;
    ScrollArea::vertical()
        .max_height(360.0)
        .auto_shrink([true, true])
        .show(ui, |ui| {
            // Cada opción ocupa todo el ancho, con el texto a la izquierda.
            ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                for item in select.options() {
                    match item {
                        SelectElementOptionOrOptgroup::Option(option) => {
                            option_row(ui, option, &mut selected, multiple, &mut chosen, false);
                        },
                        SelectElementOptionOrOptgroup::Optgroup { label, options } => {
                            ui.add_space(4.0);
                            ui.label(RichText::new(label).size(12.0).strong().color(MUTED));
                            for option in options {
                                option_row(ui, option, &mut selected, multiple, &mut chosen, true);
                            }
                        },
                    }
                }
            });
        });

    if multiple {
        select.select(selected);
        ui.separator();
        if ui.button("Aceptar").clicked() {
            return Outcome::Submit;
        }
        return Outcome::Open;
    }
    match chosen {
        Some(id) => {
            select.select(vec![id]);
            Outcome::Submit
        },
        None => Outcome::Open,
    }
}

fn option_row(
    ui: &mut Ui,
    option: &SelectElementOption,
    selected: &mut Vec<usize>,
    multiple: bool,
    chosen: &mut Option<usize>,
    indented: bool,
) {
    let is_selected = selected.contains(&option.id);
    let label = if indented {
        format!("   {}", option.label)
    } else {
        option.label.clone()
    };
    let text = RichText::new(label).color(if option.is_disabled { MUTED } else { TEXT });
    let response = ui.add_enabled(
        !option.is_disabled,
        Button::selectable(is_selected, text).wrap_mode(egui::TextWrapMode::Truncate),
    );
    if response.clicked() {
        if multiple {
            if is_selected {
                selected.retain(|id| *id != option.id);
            } else {
                selected.push(option.id);
            }
        } else {
            *chosen = Some(option.id);
        }
    }
}

pub fn color_chooser(ui: &mut Ui, color: &mut Color32) -> Outcome {
    ui.spacing_mut().slider_width = 220.0;
    egui::color_picker::color_picker_color32(ui, color, egui::color_picker::Alpha::Opaque);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Aceptar").clicked() {
            return Outcome::Submit;
        }
        if ui.button("Cancelar").clicked() {
            return Outcome::Cancel;
        }
        Outcome::Open
    })
    .inner
}

/// Menú de clic derecho con las acciones que ofrece Servo para ese punto de la
/// página. Si hay un enlace o una imagen, arriba se ve su dirección: así se sabe
/// a dónde lleva antes de abrirlo.
pub fn context_menu(ui: &mut Ui, menu: &ContextMenu) -> Outcome {
    let info = menu.element_info();
    if let Some(url) = info.link_url.as_ref().or(info.image_url.as_ref()) {
        let shown = match url.host_str() {
            Some(host) => format!("{host}{}", url.path()),
            None => url.as_str().to_owned(),
        };
        ui.add_sized(
            vec2(MENU_WIDTH, 22.0),
            Label::new(RichText::new(shown).size(12.0).color(MUTED)).truncate(),
        );
        separator(ui);
    }

    let mut outcome = Outcome::Open;
    for item in menu.items() {
        match item {
            ContextMenuItem::Separator => separator(ui),
            ContextMenuItem::Item {
                action, enabled, ..
            } => {
                let (icon, label, shortcut) = describe(*action);
                if menu_row(ui, icon, label, shortcut, *enabled) {
                    outcome = Outcome::Choose(*action);
                }
            },
        }
    }
    outcome
}

/// Icono, texto y atajo de cada acción (Servo solo trae el texto en inglés).
fn describe(action: ContextMenuAction) -> (Icon, &'static str, &'static str) {
    match action {
        ContextMenuAction::GoBack => (Icon::Back, "Atrás", "Alt+←"),
        ContextMenuAction::GoForward => (Icon::Forward, "Adelante", "Alt+→"),
        ContextMenuAction::Reload => (Icon::Reload, "Recargar", "F5"),
        ContextMenuAction::OpenLinkInNewWebView => {
            (Icon::OpenNew, "Abrir enlace en pestaña nueva", "")
        },
        ContextMenuAction::CopyLink => (Icon::Link, "Copiar dirección del enlace", ""),
        ContextMenuAction::OpenImageInNewView => (Icon::Image, "Abrir imagen en pestaña nueva", ""),
        ContextMenuAction::CopyImageLink => (Icon::Link, "Copiar dirección de la imagen", ""),
        ContextMenuAction::Cut => (Icon::Cut, "Cortar", "Ctrl+X"),
        ContextMenuAction::Copy => (Icon::Copy, "Copiar", "Ctrl+C"),
        ContextMenuAction::Paste => (Icon::Paste, "Pegar", "Ctrl+V"),
        ContextMenuAction::SelectAll => (Icon::SelectAll, "Seleccionar todo", "Ctrl+A"),
    }
}

/// Una fila del menú. Devuelve `true` si se pulsó.
fn menu_row(ui: &mut Ui, icon: Icon, label: &str, shortcut: &str, enabled: bool) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(MENU_WIDTH, 30.0), sense);
    let painter = ui.painter();
    if enabled && response.hovered() {
        let fill = if response.is_pointer_button_down_on() {
            PRESSED
        } else {
            HOVER
        };
        painter.rect_filled(rect, CornerRadius::same(6), fill);
    }
    let color = if enabled { TEXT } else { FAINT };
    let center_y = rect.center().y;
    icons::paint(
        painter,
        egui::Rect::from_center_size(pos2(rect.left() + 18.0, center_y), vec2(15.0, 15.0)),
        icon,
        if enabled { MUTED } else { FAINT },
    );
    painter.text(
        pos2(rect.left() + 38.0, center_y),
        Align2::LEFT_CENTER,
        label,
        theme::font(13.5),
        color,
    );
    if !shortcut.is_empty() {
        painter.text(
            pos2(rect.right() - 10.0, center_y),
            Align2::RIGHT_CENTER,
            shortcut,
            theme::font(12.0),
            FAINT,
        );
    }
    enabled && response.clicked()
}

fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 9.0), Sense::hover());
    ui.painter().hline(
        (rect.left() + 8.0)..=(rect.right() - 8.0),
        rect.center().y,
        Stroke::new(1.0, BORDER),
    );
}
