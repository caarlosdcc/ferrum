/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Controles de formulario que una página pide mostrar: listas desplegables
//! y selector de color.

use egui::{Align, Button, Color32, Layout, RichText, ScrollArea, Ui};
use servo::{SelectElement, SelectElementOption, SelectElementOptionOrOptgroup};

use super::theme::{MUTED, TEXT};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Open,
    Submit,
    Cancel,
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
