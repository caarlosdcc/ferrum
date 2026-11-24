/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Interfaz del navegador: pestañas en la barra de título, barra de
//! herramientas y los controles que una página pide mostrar.
//!
//! Se dibuja con egui directamente en OpenGL, fuera del motor web. La página se
//! pinta en un búfer aparte y se copia debajo de la barra, así que ningún script
//! puede leer, tapar ni imitar la barra de direcciones.

mod controls;
mod icons;
mod theme;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::text::{CCursorRange, LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Area, CentralPanel, Color32, ColorImage, Context, CornerRadius, Frame, Galley,
    Id, Key, Label, Layout, Margin, Order, Painter, Panel, Pos2, Rect, Response, RichText, Sense,
    Shape, Stroke, StrokeKind, TextBuffer, TextEdit, TextureHandle, TextureOptions, Ui, UiBuilder,
    pos2, vec2,
};
use egui_glow::egui_winit;
use euclid::default::{Point2D, Rect as GlRect, Size2D};
use servo::{DevicePoint, OffscreenRenderingContext, PixelFormat, RgbColor, WebViewId};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use self::controls::Outcome;
use self::icons::Icon;
use self::theme::*;
use crate::browser::{Browser, Location, NavigationState, PageControl, TabInfo};

/// Alto de la barra de título con las pestañas, en puntos lógicos.
pub const TAB_BAR_HEIGHT: f32 = 40.0;
/// Alto de la barra de herramientas, en puntos lógicos.
pub const TOOLBAR_HEIGHT: f32 = 46.0;

const ADDRESS_ID: &str = "ferrum-direccion";
const CONTROL_ID: &str = "ferrum-control";
/// Ancho de cada botón de ventana (minimizar, maximizar, cerrar).
const WINDOW_BUTTON_WIDTH: f32 = 46.0;
const MAX_TAB_WIDTH: f32 = 240.0;
const MIN_TAB_WIDTH: f32 = 36.0;
/// Radio de las curvas que unen la pestaña activa con la barra.
const TAB_FLARE: f32 = 8.0;

/// Ferrum dibuja su propia barra de título (con las pestañas dentro) salvo en
/// macOS, donde se respeta la del sistema y sus botones de semáforo.
pub fn custom_frame() -> bool {
    !cfg!(target_os = "macos")
}

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
    StartWindowDrag,
    ToggleMaximize,
    Minimize,
    Quit,
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
        theme::apply(&ctx);
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
        Panel::top("barra-titulo")
            .exact_size(TAB_BAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| self.title_bar(ui, browser, actions));
        Panel::top("herramientas")
            .exact_size(TOOLBAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(SURFACE))
            .show(ui, |ui| self.toolbar(ui, browser, actions));
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

    // --- Barra de título y pestañas -------------------------------------

    fn title_bar(&mut self, ui: &mut Ui, browser: &Browser, actions: &mut Vec<Action>) {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let custom = custom_frame();

        // Arrastrar la ventana desde cualquier hueco libre. Se registra antes
        // que las pestañas y botones, que quedan encima y tienen prioridad.
        if custom {
            let drag = ui.interact(rect, Id::unique("ferrum-arrastre"), Sense::click_and_drag());
            if drag.double_clicked() {
                actions.push(Action::ToggleMaximize);
            } else if drag.drag_started() {
                actions.push(Action::StartWindowDrag);
            }
        }

        let controls_width = if custom {
            3.0 * WINDOW_BUTTON_WIDTH
        } else {
            0.0
        };
        let tabs = browser.tab_infos();
        let active = browser.active_index();
        let left = rect.left() + 8.0;
        let top = rect.top() + 7.0;
        let bottom = rect.bottom();
        let room = rect.right() - controls_width - 52.0 - left;
        let tab_width = (room / tabs.len().max(1) as f32).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH);
        let tab_rect = |index: usize| {
            Rect::from_min_size(
                pos2(left + index as f32 * tab_width, top),
                vec2(tab_width, bottom - top),
            )
        };

        // Separadores finos entre pestañas inactivas.
        for index in 1..tabs.len() {
            if index != active && index != active + 1 {
                let x = tab_rect(index).left();
                painter.line_segment(
                    [pos2(x, top + 9.0), pos2(x, bottom - 9.0)],
                    Stroke::new(1.0, BORDER),
                );
            }
        }
        if let Some(rect) = (active < tabs.len()).then(|| tab_rect(active)) {
            active_tab_shape(&painter, rect);
        }
        for (index, tab) in tabs.iter().enumerate() {
            self.tab(
                ui,
                &painter,
                tab_rect(index),
                index,
                tab,
                index == active,
                actions,
            );
        }

        // Botón de nueva pestaña.
        let plus_center = pos2(left + tabs.len() as f32 * tab_width + 22.0, top + 16.0);
        let plus = Rect::from_center_size(plus_center, vec2(28.0, 28.0));
        let response = ui
            .interact(plus, Id::unique("ferrum-nueva"), Sense::click())
            .on_hover_text("Nueva pestaña (Ctrl+T)");
        if response.hovered() {
            painter.circle_filled(plus_center, 14.0, HOVER);
        }
        icons::paint(
            &painter,
            Rect::from_center_size(plus_center, vec2(13.0, 13.0)),
            Icon::Plus,
            if response.hovered() { TEXT } else { MUTED },
        );
        if response.clicked() {
            actions.push(Action::NewTab);
        }

        if custom {
            self.window_buttons(ui, &painter, rect, browser.window().is_maximized(), actions);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab(
        &self,
        ui: &mut Ui,
        painter: &Painter,
        rect: Rect,
        index: usize,
        tab: &TabInfo,
        active: bool,
        actions: &mut Vec<Action>,
    ) {
        let response = ui.interact(rect, Id::unique(("ferrum-pestaña", index)), Sense::click());
        let hovered = response.hovered();
        if hovered && !active {
            painter.rect_filled(
                Rect::from_min_max(rect.min + vec2(2.0, 0.0), rect.max - vec2(2.0, 5.0)),
                CornerRadius::same(8),
                TAB_HOVER,
            );
        }

        let center_y = rect.top() + (rect.height() - 2.0) / 2.0;
        let narrow = rect.width() < 72.0;
        let show_close = (active || hovered) && rect.width() >= 52.0;

        // Icono: indicador de carga, favicon de la web o un globo.
        let icon_rect = if narrow && !show_close {
            Rect::from_center_size(pos2(rect.center().x, center_y), vec2(16.0, 16.0))
        } else {
            Rect::from_center_size(pos2(rect.left() + 20.0, center_y), vec2(16.0, 16.0))
        };
        if tab.loading {
            icons::spinner(painter, icon_rect, ui.input(|i| i.time), ACCENT);
            ui.ctx().request_repaint();
        } else if let Some(texture) = self.favicons.get(&tab.id) {
            painter.image(
                texture.id(),
                icon_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            icons::paint(painter, icon_rect.shrink(1.0), Icon::Globe, MUTED);
        }

        // Botón de cerrar: solo en la pestaña activa o bajo el ratón.
        let close_rect =
            Rect::from_center_size(pos2(rect.right() - 20.0, center_y), vec2(20.0, 20.0));
        if show_close {
            let close = ui
                .interact(
                    close_rect,
                    Id::unique(("ferrum-cerrar", index)),
                    Sense::click(),
                )
                .on_hover_text("Cerrar pestaña (Ctrl+W)");
            if close.hovered() {
                painter.circle_filled(close_rect.center(), 10.0, PRESSED);
            }
            icons::paint(
                painter,
                close_rect.shrink(6.0),
                Icon::Close,
                if close.hovered() { TEXT } else { MUTED },
            );
            if close.clicked() {
                actions.push(Action::Close(index));
            }
        }

        // Título, recortado con "…" si no cabe.
        if !narrow {
            let text_left = rect.left() + 36.0;
            let text_right = if show_close {
                close_rect.left() - 4.0
            } else {
                rect.right() - 12.0
            };
            let color = if active { TEXT } else { MUTED };
            let galley = one_line(ui, &tab.title, TAB_FONT, color, text_right - text_left);
            painter.galley(
                pos2(text_left, center_y - galley.size().y / 2.0),
                galley,
                color,
            );
        }

        let response = response.on_hover_text(&tab.title);
        if response.clicked() {
            actions.push(Action::Activate(index));
        }
        if response.middle_clicked() {
            actions.push(Action::Close(index));
        }
    }

    fn window_buttons(
        &self,
        ui: &mut Ui,
        painter: &Painter,
        bar: Rect,
        maximized: bool,
        actions: &mut Vec<Action>,
    ) {
        let buttons = [
            (Icon::Minimize, "Minimizar", Action::Minimize),
            (
                if maximized {
                    Icon::Restore
                } else {
                    Icon::Maximize
                },
                if maximized { "Restaurar" } else { "Maximizar" },
                Action::ToggleMaximize,
            ),
            (Icon::Close, "Cerrar", Action::Quit),
        ];
        let count = buttons.len() as f32;
        for (index, (icon, tooltip, action)) in buttons.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    bar.right() - (count - index as f32) * WINDOW_BUTTON_WIDTH,
                    bar.top(),
                ),
                vec2(WINDOW_BUTTON_WIDTH, 32.0),
            );
            let response = ui
                .interact(rect, Id::unique(("ferrum-ventana", index)), Sense::click())
                .on_hover_text(tooltip);
            let is_close = action == Action::Quit;
            if response.hovered() {
                painter.rect_filled(rect, 0.0, if is_close { CLOSE_HOVER } else { HOVER });
            }
            let color = if response.hovered() && is_close {
                Color32::WHITE
            } else {
                TEXT
            };
            let size = 10.0;
            icons::paint(
                painter,
                Rect::from_center_size(rect.center(), vec2(size, size)),
                icon,
                color,
            );
            if response.clicked() {
                actions.push(action);
            }
        }
    }

    // --- Barra de herramientas ------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui, browser: &Browser, actions: &mut Vec<Action>) {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let nav = browser.navigation_state();
        let center_y = rect.center().y - 1.0;

        let buttons = [
            (Icon::Back, nav.can_go_back, "Atrás (Alt+←)", Action::Back),
            (
                Icon::Forward,
                nav.can_go_forward,
                "Adelante (Alt+→)",
                Action::Forward,
            ),
            (Icon::Reload, nav.has_page, "Recargar (F5)", Action::Reload),
            (Icon::Home, true, "Página de inicio", Action::Home),
        ];
        let mut x = rect.left() + 8.0;
        for (index, (icon, enabled, tooltip, action)) in buttons.into_iter().enumerate() {
            let button = Rect::from_min_size(pos2(x, center_y - 16.0), vec2(32.0, 32.0));
            let id = Id::unique(("ferrum-herramienta", index));
            if icon_button(ui, &painter, button, id, icon, enabled, tooltip) {
                actions.push(action);
            }
            x += 34.0;
        }

        // Pastilla del escudo, a la derecha.
        let count = browser.blocked_count().to_string();
        let count_galley = one_line(ui, &count, 13.0, ACCENT, f32::INFINITY);
        let pill_width = 16.0 + 14.0 + 6.0 + count_galley.size().x + 12.0;
        let pill = Rect::from_min_size(
            pos2(rect.right() - 10.0 - pill_width, center_y - 15.0),
            vec2(pill_width, 30.0),
        );
        let response = ui
            .interact(pill, Id::unique("ferrum-escudo"), Sense::hover())
            .on_hover_text(format!(
                "Escudo de Ferrum: {count} rastreadores, anuncios y ventanas emergentes \
                 bloqueados en esta sesión"
            ));
        painter.rect_filled(
            pill,
            CornerRadius::same(15),
            if response.hovered() {
                ACCENT_SOFT.gamma_multiply(1.6)
            } else {
                ACCENT_SOFT
            },
        );
        icons::paint(
            &painter,
            Rect::from_center_size(pos2(pill.left() + 16.0 + 4.0, center_y), vec2(15.0, 15.0)),
            Icon::Shield,
            ACCENT,
        );
        painter.galley(
            pos2(
                pill.left() + 16.0 + 14.0 + 6.0,
                center_y - count_galley.size().y / 2.0,
            ),
            count_galley,
            ACCENT,
        );

        let address = Rect::from_min_max(
            pos2(x + 8.0, center_y - 17.0),
            pos2(pill.left() - 10.0, center_y + 17.0),
        );
        self.address_field(ui, &painter, address, browser, &nav, actions);

        // Barra de progreso mientras carga la página.
        if nav.loading {
            let time = ui.input(|i| i.time);
            let width = rect.width();
            let segment = width * 0.3;
            let phase = ((time % 1.4) / 1.4) as f32;
            let start = rect.left() - segment + (width + segment) * phase;
            let bar = Rect::from_min_max(
                pos2(start.max(rect.left()), rect.bottom() - 2.0),
                pos2((start + segment).min(rect.right()), rect.bottom()),
            );
            painter.rect_filled(bar, CornerRadius::same(1), ACCENT);
            ui.ctx().request_repaint();
        }
        painter.line_segment(
            [
                pos2(rect.left(), rect.bottom() - 0.5),
                pos2(rect.right(), rect.bottom() - 0.5),
            ],
            Stroke::new(1.0, BG),
        );
    }

    fn address_field(
        &mut self,
        ui: &mut Ui,
        painter: &Painter,
        rect: Rect,
        browser: &Browser,
        nav: &NavigationState,
        actions: &mut Vec<Action>,
    ) {
        let id = Id::unique(ADDRESS_ID);
        let editing = ui.ctx().memory(|memory| memory.has_focus(id));
        let full_url = browser.display_url();
        if self.focus_address {
            self.address = full_url.clone();
        } else if !editing {
            // Mientras no se edita, la barra muestra siempre la dirección real,
            // sin el "https://" (lo indica el candado).
            self.address = pretty_url(&full_url);
        }

        let hovered = ui.rect_contains_pointer(rect);
        let radius = CornerRadius::same((rect.height() / 2.0) as u8);
        painter.rect_filled(
            rect,
            radius,
            if hovered && !editing {
                FIELD_HOVER
            } else {
                FIELD
            },
        );
        let (width, border) = if editing {
            (1.5, ACCENT)
        } else if hovered {
            (1.0, BORDER_HOVER)
        } else {
            (1.0, BORDER)
        };
        painter.rect_stroke(rect, radius, Stroke::new(width, border), StrokeKind::Inside);

        let center_y = rect.center().y;
        let text_left = match (editing, nav.location) {
            (false, Location::Secure) => {
                let chip =
                    Rect::from_center_size(pos2(rect.left() + 18.0, center_y), vec2(24.0, 24.0));
                painter.circle_filled(chip.center(), 12.0, SECURE_SOFT);
                icons::paint(painter, chip.shrink(6.0), Icon::Lock, SECURE);
                ui.interact(chip, Id::unique("ferrum-candado"), Sense::hover())
                    .on_hover_text("Conexión cifrada (HTTPS)");
                rect.left() + 36.0
            },
            (false, Location::Insecure) => {
                let label = one_line(ui, "No seguro", 12.5, WARNING, f32::INFINITY);
                let chip = Rect::from_min_size(
                    pos2(rect.left() + 5.0, center_y - 12.0),
                    vec2(8.0 + 13.0 + 5.0 + label.size().x + 10.0, 24.0),
                );
                painter.rect_filled(chip, CornerRadius::same(12), WARNING_SOFT);
                icons::paint(
                    painter,
                    Rect::from_center_size(pos2(chip.left() + 14.5, center_y), vec2(13.0, 13.0)),
                    Icon::Warning,
                    WARNING,
                );
                painter.galley(
                    pos2(chip.left() + 26.0, center_y - label.size().y / 2.0),
                    label,
                    WARNING,
                );
                ui.interact(chip, Id::unique("ferrum-no-seguro"), Sense::hover()).on_hover_text(
                    "Esta conexión no está cifrada: no escribas contraseñas ni datos personales",
                );
                chip.right() + 8.0
            },
            _ => {
                icons::paint(
                    painter,
                    Rect::from_center_size(pos2(rect.left() + 18.0, center_y), vec2(14.0, 14.0)),
                    Icon::Search,
                    MUTED,
                );
                rect.left() + 36.0
            },
        };

        let text_rect = Rect::from_min_max(
            pos2(text_left, rect.top() + 1.0),
            pos2(rect.right() - 14.0, rect.bottom() - 1.0),
        );
        let hint = format!(
            "Buscar en {} o escribir una dirección",
            browser.search_engine_name()
        );
        // Sin editar, el dominio resalta y el resto queda atenuado: así es más
        // fácil notar una dirección que imita a otra.
        let mut highlight_domain = |ui: &Ui, text: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> {
            let text = text.as_str();
            let end = text.find(['/', '?', '#']).unwrap_or(text.len());
            let mut job = LayoutJob::default();
            let format = |color| TextFormat {
                font_id: theme::font(ADDRESS_FONT),
                color,
                ..Default::default()
            };
            job.append(&text[..end], 0.0, format(TEXT));
            job.append(&text[end..], 0.0, format(MUTED));
            ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
        };
        let mut output = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    let mut edit = TextEdit::singleline(&mut self.address)
                        .id(id)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .font(theme::font(ADDRESS_FONT))
                        .text_color(TEXT)
                        .hint_text(RichText::new(hint).color(MUTED))
                        .desired_width(f32::INFINITY)
                        .align(Align2::LEFT_CENTER);
                    if !editing {
                        edit = edit.layouter(&mut highlight_domain);
                    }
                    edit.show(ui)
                },
            )
            .inner;

        let response = &output.response;
        if self.focus_address || response.gained_focus() {
            // Como en cualquier navegador: al entrar, dirección completa y
            // todo seleccionado.
            if self.address != full_url && !response.changed() {
                self.address = full_url;
            }
            response.request_focus();
            output
                .state
                .cursor
                .set_char_range(Some(CCursorRange::select_all_str(&self.address)));
            output.state.store(ui.ctx(), id);
            self.focus_address = false;
            ui.ctx().request_repaint();
        }
        if output.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let text = self.address.trim().to_owned();
            if !text.is_empty() {
                actions.push(Action::Navigate(text));
            }
        }
    }

    // --- Controles de la página -----------------------------------------

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
            self.viewport.min.y + element.max.y as f32 / ppp + 4.0,
        );
        let min_width = (element.width() as f32 / ppp).max(160.0);

        let mut outcome = Outcome::Open;
        let area = Area::new(Id::unique(CONTROL_ID))
            .order(Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(self.viewport)
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(min_width);
                        outcome = match &mut control {
                            PageControl::Select(select) => controls::select_list(ui, select),
                            PageControl::Color { color, .. } => controls::color_chooser(ui, color),
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

/// Botón redondo con icono. Devuelve `true` si se pulsó.
fn icon_button(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    id: Id,
    icon: Icon,
    enabled: bool,
    tooltip: &str,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response: Response = ui.interact(rect, id, sense).on_hover_text(tooltip);
    if enabled && response.hovered() {
        let fill = if response.is_pointer_button_down_on() {
            PRESSED
        } else {
            HOVER
        };
        painter.circle_filled(rect.center(), rect.width() / 2.0, fill);
    }
    icons::paint(
        painter,
        Rect::from_center_size(rect.center(), vec2(16.0, 16.0)),
        icon,
        if enabled { TEXT } else { FAINT },
    );
    enabled && response.clicked()
}

/// Forma de la pestaña activa: esquinas superiores redondeadas y dos curvas
/// en la base que la funden con la barra de herramientas.
fn active_tab_shape(painter: &Painter, rect: Rect) {
    painter.rect_filled(
        rect,
        CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        },
        SURFACE,
    );
    let flare = TAB_FLARE;
    let bottom = rect.bottom();
    flare_corner(
        painter,
        pos2(rect.left(), bottom),
        pos2(rect.left() - flare, bottom - flare),
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
    flare_corner(
        painter,
        pos2(rect.right(), bottom),
        pos2(rect.right() + flare, bottom - flare),
        std::f32::consts::PI,
        std::f32::consts::FRAC_PI_2,
    );
}

/// Esquina cóncava rellena: abanico de triángulos desde `corner` hasta el arco.
fn flare_corner(painter: &Painter, corner: Pos2, arc_center: Pos2, from: f32, to: f32) {
    let steps = 10;
    let mut mesh = Mesh::default();
    let vertex = |pos: Pos2| Vertex {
        pos,
        uv: WHITE_UV,
        color: SURFACE,
    };
    mesh.vertices.push(vertex(corner));
    for i in 0..=steps {
        let angle = from + (to - from) * i as f32 / steps as f32;
        mesh.vertices.push(vertex(
            arc_center + vec2(angle.cos(), angle.sin()) * TAB_FLARE,
        ));
    }
    for i in 1..=steps as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// Texto en una línea, recortado con "…" si pasa de `max_width`.
fn one_line(ui: &Ui, text: &str, size: f32, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: theme::font(size),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// `https://www.ejemplo.com/` → `www.ejemplo.com`; el resto de la ruta se
/// conserva. Nunca oculta el dominio.
fn pretty_url(full: &str) -> String {
    let rest = full
        .strip_prefix("https://")
        .or_else(|| full.strip_prefix("http://"))
        .unwrap_or(full);
    rest.strip_suffix('/')
        .filter(|trimmed| !trimmed.contains('/'))
        .unwrap_or(rest)
        .to_owned()
}

/// Franja con el mensaje de un `alert()`, claramente separada de la interfaz.
fn message_bar(ui: &mut Ui, message: &str, actions: &mut Vec<Action>) {
    Panel::top("aviso-pagina")
        .resizable(false)
        .show_separator_line(false)
        .frame(
            Frame::NONE
                .fill(MESSAGE_BG)
                .inner_margin(Margin::symmetric(14, 7)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("La página dice:").color(WARNING).strong());
                ui.add(Label::new(RichText::new(format!("«{message}»")).color(TEXT)).truncate());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    if response.hovered() {
                        ui.painter().circle_filled(rect.center(), 11.0, HOVER);
                    }
                    icons::paint(ui.painter(), rect.shrink(6.0), Icon::Close, MUTED);
                    if response.clicked() {
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

    #[test]
    fn pretty_url_keeps_domain_and_path() {
        assert_eq!(pretty_url("https://example.com/"), "example.com");
        assert_eq!(pretty_url("http://example.com/a/b/"), "example.com/a/b/");
        assert_eq!(pretty_url("https://xn--pple-43d.com/"), "xn--pple-43d.com");
        assert_eq!(pretty_url("about:blank"), "about:blank");
        assert_eq!(pretty_url(""), "");
    }
}
