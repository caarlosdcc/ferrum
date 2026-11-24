/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Interfaz del navegador: barra de pestañas, barra de herramientas y los
//! controles que una página pide mostrar (listas desplegables, selector de color).
//!
//! Se dibuja con egui directamente en OpenGL, fuera del motor web. La página se
//! pinta en un búfer aparte y se copia debajo de la barra, así que ningún script
//! puede leer, tapar ni imitar la barra de direcciones.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::text::CCursorRange;
use egui::{
    Align, Area, Button, CentralPanel, Color32, ColorImage, Context, CornerRadius, Frame, Id,
    Image, Key, Label, Layout, Margin, Order, Panel, Rect, RichText, ScrollArea, Sense, Spinner,
    Stroke, TextEdit, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2, pos2, vec2,
};
use egui_glow::egui_winit;
use euclid::default::{Point2D, Rect as GlRect, Size2D};
use servo::{
    DevicePoint, OffscreenRenderingContext, PixelFormat, RgbColor, SelectElement,
    SelectElementOption, SelectElementOptionOrOptgroup, WebViewId,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::browser::{Browser, Location, PageControl};

/// Alto de la barra de pestañas, en puntos lógicos.
pub const TAB_BAR_HEIGHT: f32 = 36.0;
/// Alto de la barra de herramientas, en puntos lógicos.
pub const TOOLBAR_HEIGHT: f32 = 44.0;

const ADDRESS_ID: &str = "ferrum-direccion";
const CONTROL_ID: &str = "ferrum-control";

// Paleta: la misma que la página de inicio.
const BG: Color32 = Color32::from_rgb(0x0b, 0x0d, 0x11);
const TOOLBAR_BG: Color32 = Color32::from_rgb(0x18, 0x1b, 0x22);
const FIELD_BG: Color32 = Color32::from_rgb(0x0f, 0x11, 0x15);
const TAB_HOVER: Color32 = Color32::from_rgb(0x14, 0x17, 0x1d);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xa6);
const ACCENT: Color32 = Color32::from_rgb(0xe0, 0x70, 0x3a);
const SECURE: Color32 = Color32::from_rgb(0x6c, 0xc0, 0x7a);
const WARNING: Color32 = Color32::from_rgb(0xf2, 0xb8, 0x4b);
const MESSAGE_BG: Color32 = Color32::from_rgb(0x2b, 0x24, 0x16);

/// Lo que el usuario pidió en la interfaz. Se aplica después de dibujar, para
/// no llamar a Servo mientras egui tiene prestado el estado.
#[derive(Debug, PartialEq)]
pub enum Action {
    Activate(usize),
    Close(usize),
    NewTab,
    Back,
    Forward,
    Reload,
    Home,
    Navigate(String),
    DismissMessage,
}

pub struct Gui {
    ctx: Context,
    input: egui_winit::State,
    painter: egui_glow::Painter,
    shapes: Vec<egui::epaint::ClippedShape>,
    textures_delta: egui::TexturesDelta,
    pixels_per_point: f32,
    repaint_at: Option<Instant>,
    /// Texto de la barra de direcciones.
    address: String,
    focus_address: bool,
    favicons: HashMap<WebViewId, TextureHandle>,
    /// Zona de la ventana donde se ve la página, en puntos.
    viewport: Rect,
    /// Hay una lista desplegable o un selector abierto encima de la página.
    control_open: bool,
}

impl Gui {
    pub fn new(
        event_loop: &ActiveEventLoop,
        window: &Window,
        gl: Arc<egui_glow::glow::Context>,
    ) -> Result<Self, String> {
        let painter = egui_glow::Painter::new(gl, "", None, true)
            .map_err(|e| format!("no se pudo iniciar la interfaz: {e}"))?;
        let ctx = Context::default();
        apply_style(&ctx);
        let pixels_per_point = window.scale_factor() as f32;
        let input = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            event_loop,
            Some(pixels_per_point),
            None,
            Some(painter.max_texture_side()),
        );
        Ok(Self {
            ctx,
            input,
            painter,
            shapes: Vec::new(),
            textures_delta: egui::TexturesDelta::default(),
            pixels_per_point,
            repaint_at: None,
            address: String::new(),
            focus_address: false,
            favicons: HashMap::new(),
            viewport: Rect::NOTHING,
            control_open: false,
        })
    }

    /// Pasa un evento de la ventana a egui. Devuelve `true` si hay que redibujar.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.input.on_window_event(window, event).repaint
    }

    /// Algún campo de la interfaz (la barra de direcciones) tiene el teclado.
    pub fn wants_keyboard(&self) -> bool {
        self.ctx.egui_wants_keyboard_input() || self.control_open
    }

    /// Pone el foco en la barra de direcciones con todo el texto seleccionado.
    pub fn focus_address(&mut self) {
        self.focus_address = true;
    }

    /// Momento en que egui quiere volver a dibujar (animaciones, tooltips…).
    pub fn repaint_deadline(&self) -> Option<Instant> {
        self.repaint_at
    }

    pub fn clear_repaint_deadline(&mut self) {
        self.repaint_at = None;
    }

    /// Convierte la posición del ratón en la ventana a coordenadas de la página.
    /// `None` si el ratón está sobre la interfaz o hay un control abierto.
    pub fn page_point(&self, cursor: PhysicalPosition<f64>) -> Option<DevicePoint> {
        if self.control_open {
            return None;
        }
        let ppp = self.pixels_per_point;
        let point = pos2(cursor.x as f32 / ppp, cursor.y as f32 / ppp);
        if !self.viewport.contains(point) {
            return None;
        }
        Some(self.page_point_unchecked(cursor))
    }

    /// Como [`Self::page_point`], aunque el ratón esté fuera de la página
    /// (para arrastrar una selección de texto más allá del borde).
    pub fn page_point_unchecked(&self, cursor: PhysicalPosition<f64>) -> DevicePoint {
        let ppp = self.pixels_per_point;
        DevicePoint::new(
            cursor.x as f32 - self.viewport.min.x * ppp,
            cursor.y as f32 - self.viewport.min.y * ppp,
        )
    }

    /// Quita el foco de la barra de direcciones (clic en la página).
    pub fn release_focus(&mut self) {
        let id = Id::unique(ADDRESS_ID);
        self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        self.focus_address = false;
    }

    /// Tamaño en píxeles físicos de la zona de la página.
    pub fn viewport_size(&self) -> Option<PhysicalSize<u32>> {
        if !self.viewport.is_positive() {
            return None;
        }
        let size = self.viewport.size() * self.pixels_per_point;
        Some(PhysicalSize::new(
            (size.x.round() as u32).max(1),
            (size.y.round() as u32).max(1),
        ))
    }

    /// Dibuja la interfaz de un fotograma y devuelve lo que pidió el usuario.
    pub fn run(&mut self, browser: &Browser) -> Vec<Action> {
        let window = browser.window();
        let raw_input = self.input.take_egui_input(window);
        self.update_favicons(browser);

        let mut actions = Vec::new();
        let ctx = self.ctx.clone();
        let output = ctx.run_ui(raw_input, |ui| self.ui(ui, browser, &mut actions));

        self.input
            .handle_platform_output(window, output.platform_output);
        self.shapes = output.shapes;
        self.pixels_per_point = output.pixels_per_point;
        self.textures_delta.append(output.textures_delta);

        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport| viewport.repaint_delay);
        self.repaint_at = if delay.is_zero() {
            window.request_redraw();
            None
        } else {
            Instant::now().checked_add(delay)
        };
        actions
    }

    /// Pinta lo último que se dibujó con [`Self::run`] en la ventana.
    pub fn paint(&mut self, window: &Window) {
        let mut textures_delta = std::mem::take(&mut self.textures_delta);
        // El orden no importa: cada textura se actualiza por separado.
        #[allow(clippy::iter_over_hash_type)]
        for (id, image_deltas) in textures_delta.set.drain() {
            for image_delta in image_deltas {
                self.painter.set_texture(id, &image_delta);
            }
        }
        let shapes = std::mem::take(&mut self.shapes);
        let primitives = self.ctx.tessellate(shapes, self.pixels_per_point);
        let size: [u32; 2] = window.inner_size().into();
        self.painter.clear(size, BG.to_normalized_gamma_f32());
        self.painter
            .paint_primitives(size, self.pixels_per_point, &primitives);
        #[allow(clippy::iter_over_hash_type)]
        for id in textures_delta.free.drain() {
            self.painter.free_texture(id);
        }
    }

    pub fn destroy(&mut self) {
        self.painter.destroy();
    }

    // ------------------------------------------------------------------
    // Dibujo
    // ------------------------------------------------------------------

    fn ui(&mut self, ui: &mut Ui, browser: &Browser, actions: &mut Vec<Action>) {
        self.tab_bar(ui, browser, actions);
        self.toolbar(ui, browser, actions);
        if let Some(message) = browser.page_message() {
            message_bar(ui, &message, actions);
            if let Some(remaining) = browser.page_message_remaining() {
                ui.ctx().request_repaint_after(remaining);
            }
        }

        let offscreen = browser.offscreen();
        let viewport = CentralPanel::no_frame()
            .show(ui, |ui| {
                let rect = ui.max_rect();
                ui.painter().add(page_callback(rect, offscreen));
                rect
            })
            .inner;
        self.viewport = viewport;

        self.control_open = self.page_control(ui.ctx(), browser);
    }

    fn tab_bar(&mut self, ui: &mut Ui, browser: &Browser, actions: &mut Vec<Action>) {
        let tabs = browser.tab_infos();
        let active = browser.active_index();
        Panel::top("pestañas")
            .exact_size(TAB_BAR_HEIGHT)
            .resizable(false)
            .frame(Frame::NONE.fill(BG).inner_margin(Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 0,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let new_tab_width = 32.0;
                    let available = ui.available_width() - new_tab_width - 8.0;
                    let width = (available / tabs.len().max(1) as f32).clamp(48.0, 230.0);
                    for (index, tab) in tabs.iter().enumerate() {
                        self.tab(ui, index, tab, index == active, width, actions);
                    }
                    let new_tab = ui
                        .add(
                            Button::new(RichText::new("+").size(20.0).color(MUTED))
                                .frame(false)
                                .min_size(vec2(new_tab_width, TAB_BAR_HEIGHT - 8.0)),
                        )
                        .on_hover_text("Nueva pestaña (Ctrl+T)");
                    if new_tab.clicked() {
                        actions.push(Action::NewTab);
                    }
                });
            });
    }

    fn tab(
        &self,
        ui: &mut Ui,
        index: usize,
        tab: &crate::browser::TabInfo,
        active: bool,
        width: f32,
        actions: &mut Vec<Action>,
    ) {
        let height = TAB_BAR_HEIGHT - 6.0;
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        let fill = if active {
            TOOLBAR_BG
        } else if response.hovered() {
            TAB_HOVER
        } else {
            Color32::TRANSPARENT
        };
        let radius = CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        ui.painter().rect_filled(rect, radius, fill);

        let inner = rect.shrink2(vec2(10.0, 0.0));
        let mut content = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center)),
        );
        content.spacing_mut().item_spacing.x = 6.0;
        if tab.loading {
            content.add(Spinner::new().size(14.0).color(ACCENT));
        } else if let Some(texture) = self.favicons.get(&tab.id) {
            content.add(Image::new((texture.id(), vec2(16.0, 16.0))));
        } else {
            content.label(RichText::new("🌐").size(13.0).color(MUTED));
        }
        content.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = ui
                .add(
                    Button::new(RichText::new("×").size(16.0).color(MUTED))
                        .frame(false)
                        .min_size(vec2(18.0, 18.0)),
                )
                .on_hover_text("Cerrar pestaña (Ctrl+W)");
            if close.clicked() {
                actions.push(Action::Close(index));
            }
            let color = if active { TEXT } else { MUTED };
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(Label::new(RichText::new(&tab.title).color(color)).truncate());
            });
        });

        let response = response.on_hover_text(&tab.title);
        if response.clicked() {
            actions.push(Action::Activate(index));
        }
        if response.middle_clicked() {
            actions.push(Action::Close(index));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, browser: &Browser, actions: &mut Vec<Action>) {
        let nav = browser.navigation_state();
        Panel::top("herramientas")
            .exact_size(TOOLBAR_HEIGHT)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(TOOLBAR_BG)
                    .inner_margin(Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if nav_button(ui, "⬅", nav.can_go_back, "Atrás (Alt+←)") {
                        actions.push(Action::Back);
                    }
                    if nav_button(ui, "➡", nav.can_go_forward, "Adelante (Alt+→)") {
                        actions.push(Action::Forward);
                    }
                    if nav_button(ui, "🔄", nav.has_page, "Recargar (F5)") {
                        actions.push(Action::Reload);
                    }
                    if nav_button(ui, "🏠", true, "Página de inicio") {
                        actions.push(Action::Home);
                    }
                    ui.add_space(4.0);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let blocked = browser.blocked_count();
                        ui.add(
                            Label::new(
                                RichText::new(format!("🛡 {blocked}"))
                                    .size(15.0)
                                    .color(ACCENT),
                            )
                            .sense(Sense::hover()),
                        )
                        .on_hover_text(
                            "Rastreadores, anuncios y ventanas emergentes bloqueados en esta sesión",
                        );
                        ui.add_space(6.0);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            self.address_field(ui, browser, &nav.location, actions);
                        });
                    });
                });
            });
    }

    fn address_field(
        &mut self,
        ui: &mut Ui,
        browser: &Browser,
        location: &Location,
        actions: &mut Vec<Action>,
    ) {
        let id = Id::unique(ADDRESS_ID);
        let editing = ui.ctx().memory(|memory| memory.has_focus(id));
        if !editing && !self.focus_address {
            // Mientras no se edita, la barra muestra siempre la dirección real.
            self.address = browser.display_url();
        }

        Frame::NONE
            .fill(FIELD_BG)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(
                1.0,
                if editing {
                    ACCENT
                } else {
                    Color32::from_rgb(0x2c, 0x31, 0x3c)
                },
            ))
            .inner_margin(Margin::symmetric(12, 4))
            .show(ui, |ui| {
                // Todo el alto del campo, con el texto centrado en vertical.
                let size = ui.available_size();
                ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
                    match location {
                        Location::Secure => {
                            ui.label(RichText::new("🔒").color(SECURE))
                                .on_hover_text("Conexión cifrada (HTTPS)");
                        },
                        Location::Insecure => {
                            ui.label(RichText::new("⚠ No seguro").color(WARNING))
                                .on_hover_text(
                                    "Esta conexión no está cifrada: no escribas contraseñas \
                                     ni datos personales",
                                );
                        },
                        Location::Internal | Location::Other => {},
                    }
                    let hint = format!(
                        "Buscar en {} o escribir una dirección",
                        browser.search_engine_name()
                    );
                    let mut output = TextEdit::singleline(&mut self.address)
                        .id(id)
                        .frame(Frame::NONE)
                        .hint_text(RichText::new(hint).color(MUTED))
                        .desired_width(f32::INFINITY)
                        .align(egui::Align2::LEFT_CENTER)
                        .text_color(TEXT)
                        .show(ui);

                    let response = &output.response;
                    if self.focus_address || response.gained_focus() {
                        // Como en cualquier navegador: al entrar, todo seleccionado.
                        response.request_focus();
                        output
                            .state
                            .cursor
                            .set_char_range(Some(CCursorRange::select_all_str(&self.address)));
                        output.state.store(ui.ctx(), id);
                        self.focus_address = false;
                    }
                    if output.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let text = self.address.trim().to_owned();
                        if !text.is_empty() {
                            actions.push(Action::Navigate(text));
                        }
                    }
                });
            });
    }

    /// Dibuja la lista desplegable o el selector de color que pidió la página.
    /// Devuelve `true` si sigue abierto.
    fn page_control(&self, ctx: &Context, browser: &Browser) -> bool {
        let Some(mut control) = browser.take_control() else {
            return false;
        };
        let ppp = self.pixels_per_point;
        let element = control.position();
        let anchor = pos2(
            self.viewport.min.x + element.min.x as f32 / ppp,
            self.viewport.min.y + element.max.y as f32 / ppp,
        );
        let min_width = (element.width() as f32 / ppp).max(140.0);

        let mut outcome = Outcome::Open;
        let area = Area::new(Id::unique(CONTROL_ID))
            .order(Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(self.viewport)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(min_width);
                    outcome = match &mut control {
                        PageControl::Select(select) => select_list(ui, select),
                        PageControl::Color { color, .. } => color_chooser(ui, color),
                    };
                });
            });

        // Clic fuera o Escape: se cierra sin cambiar nada.
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|pos| !area.response.rect.contains(pos))
        });
        if pressed_outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
            outcome = Outcome::Cancel;
        }

        match outcome {
            Outcome::Open => {
                browser.put_back_control(control);
                true
            },
            Outcome::Submit => {
                control.submit();
                browser.window().request_redraw();
                false
            },
            Outcome::Cancel => {
                // Al soltarlo, Servo recibe la selección original.
                drop(control);
                browser.window().request_redraw();
                false
            },
        }
    }

    fn update_favicons(&mut self, browser: &Browser) {
        for (id, image) in browser.take_favicon_changes() {
            match image.as_ref().and_then(favicon_image) {
                Some(image) => {
                    let texture = self.ctx.load_texture(
                        format!("favicon-{id:?}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    self.favicons.insert(id, texture);
                },
                None => {
                    self.favicons.remove(&id);
                },
            }
        }
        let open: Vec<WebViewId> = browser.tab_infos().iter().map(|tab| tab.id).collect();
        self.favicons.retain(|id, _| open.contains(id));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Open,
    Submit,
    Cancel,
}

fn select_list(ui: &mut Ui, select: &mut SelectElement) -> Outcome {
    let multiple = select.allow_select_multiple();
    let mut selected = select.selected_options();
    let mut chosen = None;

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
                            ui.label(RichText::new(label).strong().color(MUTED));
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
        format!("    {}", option.label)
    } else {
        option.label.clone()
    };
    let response = ui.add_enabled(
        !option.is_disabled,
        Button::selectable(is_selected, label).wrap_mode(egui::TextWrapMode::Truncate),
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

fn color_chooser(ui: &mut Ui, color: &mut Color32) -> Outcome {
    egui::color_picker::color_picker_color32(ui, color, egui::color_picker::Alpha::Opaque);
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

fn nav_button(ui: &mut Ui, icon: &str, enabled: bool, tooltip: &str) -> bool {
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(icon).size(17.0))
            .frame(false)
            .min_size(vec2(32.0, 30.0)),
    )
    .on_hover_text(tooltip)
    .clicked()
}

/// Franja con el mensaje de un `alert()`, claramente separada de la interfaz.
fn message_bar(ui: &mut Ui, message: &str, actions: &mut Vec<Action>) {
    Panel::top("aviso-pagina")
        .resizable(false)
        .frame(
            Frame::NONE
                .fill(MESSAGE_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("💬 La página dice:").color(WARNING));
                ui.add(Label::new(RichText::new(format!("«{message}»")).color(TEXT)).truncate());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(Button::new(RichText::new("×").color(MUTED)).frame(false))
                        .clicked()
                    {
                        actions.push(Action::DismissMessage);
                    }
                });
            });
        });
}

/// Copia la imagen de la página (pintada por Servo en su búfer) en `rect`.
fn page_callback(rect: Rect, offscreen: &OffscreenRenderingContext) -> egui::PaintCallback {
    let blit = offscreen.render_to_parent_callback();
    egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            let Some(blit) = &blit else {
                return;
            };
            let viewport = info.viewport_in_pixels();
            let target = GlRect::new(
                Point2D::new(viewport.left_px, viewport.from_bottom_px),
                Size2D::new(viewport.width_px, viewport.height_px),
            );
            blit(painter.gl(), target);
        })),
    }
}

/// Convierte el icono de una web en una textura. Los iconos enormes se
/// descartan: no hace falta gastar memoria en ellos.
fn favicon_image(image: &servo::Image) -> Option<ColorImage> {
    const MAX_SIDE: u32 = 512;
    if image.width == 0 || image.height == 0 || image.width > MAX_SIDE || image.height > MAX_SIDE {
        return None;
    }
    let size = [image.width as usize, image.height as usize];
    let pixels = size[0] * size[1];
    let data = image.data();
    let rgba: Vec<u8> = match image.format {
        PixelFormat::RGBA8 => data.get(..pixels * 4)?.to_vec(),
        PixelFormat::BGRA8 => data
            .get(..pixels * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, a]| [r, g, b, a])
            .collect(),
        PixelFormat::RGB8 => data
            .get(..pixels * 3)?
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        PixelFormat::K8 => data
            .get(..pixels)?
            .iter()
            .flat_map(|&k| [k, k, k, 255])
            .collect(),
        PixelFormat::KA8 => data
            .get(..pixels * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[k, a]| [k, k, k, a])
            .collect(),
    };
    Some(ColorImage::from_rgba_premultiplied(size, &rgba))
}

pub fn rgb_to_color(color: Option<RgbColor>) -> Color32 {
    color.map_or(Color32::BLACK, |c| {
        Color32::from_rgb(c.red, c.green, c.blue)
    })
}

pub fn color_to_rgb(color: Color32) -> RgbColor {
    RgbColor {
        red: color.r(),
        green: color.g(),
        blue: color.b(),
    }
}

fn apply_style(ctx: &Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.all_styles_mut(|style| {
        // Los textos de la interfaz no se seleccionan al hacer clic.
        style.interaction.selectable_labels = false;
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = TOOLBAR_BG;
        style.visuals.extreme_bg_color = FIELD_BG;
        style.visuals.selection.bg_fill = ACCENT.linear_multiply(0.6);
        style.visuals.selection.stroke = Stroke::new(1.0, TEXT);
        style.visuals.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        style.visuals.text_cursor.blink = false;
        style.visuals.override_text_color = Some(TEXT);
        style.spacing.button_padding = vec2(8.0, 4.0);
        style.spacing.interact_size = Vec2::new(28.0, 24.0);
        for font in style.text_styles.values_mut() {
            font.size = font.size.max(14.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_round_trip() {
        let rgb = RgbColor {
            red: 1,
            green: 2,
            blue: 3,
        };
        let back = color_to_rgb(rgb_to_color(Some(rgb)));
        assert_eq!((back.red, back.green, back.blue), (1, 2, 3));
    }
}
