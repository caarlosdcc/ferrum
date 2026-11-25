/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! El navegador: pestañas, navegación y la conexión entre Servo y el escudo de
//! privacidad.
//!
//! Toda petición de red pasa por [`Browser::filter_request`] antes de salir del
//! equipo. Ahí se decide si se bloquea, se limpia o se deja pasar.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use euclid::Scale;
use ferrum_shield::omnibox::{self, SearchEngine};
use ferrum_shield::{Shield, Verdict};
use http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use servo::{
    ColorPicker, ConsoleLogLevel, ContextMenu, ContextMenuAction, CreateNewWebViewRequest, Cursor,
    DeviceIndependentPixel, DeviceIntRect, DevicePixel, DevicePoint, EmbedderControl,
    EmbedderControlId, FilePicker, Image, LoadStatus, NavigationRequest, OffscreenRenderingContext,
    PermissionRequest, RenderingContext, SelectElement, Servo, SimpleDialog, UserContentManager,
    UserScript, WebResourceLoad, WebResourceResponse, WebView, WebViewBuilder, WebViewDelegate,
    WebViewId, WindowRenderingContext,
};
use url::Url;
use winit::dpi::PhysicalSize;
use winit::window::{CursorIcon, Window};

use crate::gui::{self, TAB_BAR_HEIGHT, TOOLBAR_HEIGHT};
use crate::{input, pages};

/// Script anti-huella digital inyectado en cada página.
const PRIVACY_SCRIPT: &str = include_str!("../resources/privacy.js");

/// Arreglos de compatibilidad para sitios concretos (lo que a Servo aún le falta).
const COMPAT_SCRIPT: &str = include_str!("../resources/compat.js");

/// Una página solo puede abrir ventanas nuevas justo después de un clic o una
/// tecla del usuario. Así se bloquean los pop-ups y pop-unders automáticos.
const POPUP_GESTURE_WINDOW: Duration = Duration::from_millis(1500);

/// Cuánto tiempo se muestra un aviso de Ferrum en la franja de mensajes.
const PAGE_MESSAGE_DURATION: Duration = Duration::from_secs(8);

/// Largo máximo del texto de un `alert()`, `confirm()` o `prompt()`.
const DIALOG_MAX_CHARS: usize = 1500;

/// Lo que la interfaz necesita saber de cada pestaña.
#[derive(Clone, Debug)]
pub struct TabInfo {
    pub id: WebViewId,
    pub title: String,
    pub url: Option<Url>,
    pub loading: bool,
}

/// Tipo de conexión de la página activa, para el indicador de la barra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Secure,
    Insecure,
    Internal,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct NavigationState {
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub has_page: bool,
    pub loading: bool,
    pub location: Location,
}

/// Control de formulario que la página pidió mostrar y que la interfaz dibuja
/// encima de ella.
pub enum PageControl {
    Select(SelectElement),
    Color {
        picker: ColorPicker,
        color: egui::Color32,
    },
    /// Menú de clic derecho. `at` es el punto de la página donde se hizo clic.
    Menu {
        menu: ContextMenu,
        at: Option<DevicePoint>,
    },
}

impl PageControl {
    fn id(&self) -> EmbedderControlId {
        match self {
            PageControl::Select(select) => select.id(),
            PageControl::Color { picker, .. } => picker.id(),
            PageControl::Menu { menu, .. } => menu.id(),
        }
    }

    /// Zona que ocupa en la página el elemento que abrió el control.
    pub fn position(&self) -> DeviceIntRect {
        match self {
            PageControl::Select(select) => select.position(),
            PageControl::Color { picker, .. } => picker.position(),
            PageControl::Menu { menu, .. } => menu.position(),
        }
    }

    /// Envía a la página lo que eligió el usuario.
    pub fn submit(self) {
        match self {
            PageControl::Select(select) => select.submit(),
            PageControl::Color { mut picker, color } => {
                picker.select(Some(gui::color_to_rgb(color)));
                picker.submit();
            },
            PageControl::Menu { menu, .. } => menu.dismiss(),
        }
    }

    /// Ejecuta una acción del menú de clic derecho (Servo hace el resto).
    pub fn choose(self, action: ContextMenuAction) {
        if let PageControl::Menu { menu, .. } = self {
            menu.select(action);
        }
    }
}

/// Tipo de aviso que pide una página.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
}

/// `alert()`, `confirm()` o `prompt()` de una página, esperando respuesta. La
/// página queda en pausa hasta que se responde, pero solo esa pestaña: el resto
/// del navegador sigue funcionando.
pub struct PageDialog {
    dialog: SimpleDialog,
    /// Quién lo pide: el dominio de la página, para que no pueda hacerse pasar
    /// por Ferrum ni por otra web.
    pub origin: String,
    pub message: String,
    /// Texto del campo de `prompt()`.
    pub text: String,
    /// Avisos que lleva mostrados esta página. Desde el segundo se ofrece
    /// impedir más (contra las webs que encadenan avisos para no dejarte salir).
    pub shown: u32,
    pub mute: bool,
}

impl PageDialog {
    pub fn kind(&self) -> DialogKind {
        match self.dialog {
            SimpleDialog::Alert(_) => DialogKind::Alert,
            SimpleDialog::Confirm(_) => DialogKind::Confirm,
            SimpleDialog::Prompt(_) => DialogKind::Prompt,
        }
    }

    /// Responde a la página: aceptar o cancelar (un `alert()` solo se cierra).
    fn answer(self, accept: bool) {
        match self.dialog {
            SimpleDialog::Alert(alert) => alert.confirm(),
            SimpleDialog::Confirm(confirm) if accept => confirm.confirm(),
            SimpleDialog::Confirm(confirm) => confirm.dismiss(),
            SimpleDialog::Prompt(mut prompt) if accept => {
                prompt.set_current_value(&self.text);
                prompt.confirm();
            },
            SimpleDialog::Prompt(prompt) => prompt.dismiss(),
        }
    }
}

/// Avisos de una pestaña: cuántos lleva su página y si se silenciaron.
#[derive(Default)]
struct DialogPolicy {
    origin: String,
    shown: u32,
    muted: bool,
}

pub struct Browser {
    me: Weak<Browser>,
    window: Window,
    servo: Servo,
    /// Contexto de la ventana, donde se dibujan la interfaz y la página.
    rendering_context: Rc<WindowRenderingContext>,
    /// Búfer donde Servo pinta las páginas, que luego se copia bajo la barra.
    offscreen: Rc<OffscreenRenderingContext>,
    viewport_size: Cell<PhysicalSize<u32>>,
    user_content: Rc<UserContentManager>,
    shield: Shield,
    search: SearchEngine,
    tabs: RefCell<Vec<WebView>>,
    active: Cell<usize>,
    control: RefCell<Option<(WebViewId, PageControl)>>,
    /// Punto de la página del último clic derecho, para abrir ahí el menú.
    context_point: Cell<Option<DevicePoint>>,
    dialogs: RefCell<HashMap<WebViewId, PageDialog>>,
    dialog_policies: RefCell<HashMap<WebViewId, DialogPolicy>>,
    file_picker: RefCell<Option<FilePicker>>,
    favicon_changes: RefCell<Vec<WebViewId>>,
    page_cursor: Cell<Option<CursorIcon>>,
    page_message: RefCell<Option<(String, Instant)>>,
    last_gesture: Cell<Option<Instant>>,
    /// Última URL limpiada, para no entrar en bucle si el servidor insiste en
    /// redirigir a la versión con parámetros de rastreo.
    last_cleaned: RefCell<Option<(Url, Instant)>>,
    popups_blocked: Cell<u64>,
    /// Cambios de dirección o de estado de carga, en cualquier pestaña. Sirve
    /// para saber si una acción del agente llevó a otra página.
    load_generation: Cell<u64>,
    /// `Some` si un agente controla Ferrum por MCP; `true` mientras trabaja.
    agent: Cell<Option<bool>>,
    exit_requested: Cell<bool>,
}

impl Browser {
    pub fn new(
        window: Window,
        servo: Servo,
        rendering_context: Rc<WindowRenderingContext>,
        shield: Shield,
        search: SearchEngine,
    ) -> Rc<Self> {
        let user_content = Rc::new(UserContentManager::new(&servo));
        user_content.add_script(Rc::new(UserScript::new(PRIVACY_SCRIPT.to_owned(), None)));
        user_content.add_script(Rc::new(UserScript::new(COMPAT_SCRIPT.to_owned(), None)));

        let viewport_size = initial_viewport_size(&window);
        let offscreen = Rc::new(rendering_context.offscreen_context(viewport_size));

        Rc::new_cyclic(|me| Browser {
            me: me.clone(),
            window,
            servo,
            rendering_context,
            offscreen,
            viewport_size: Cell::new(viewport_size),
            user_content,
            shield,
            search,
            tabs: RefCell::default(),
            active: Cell::new(0),
            control: RefCell::default(),
            context_point: Cell::new(None),
            dialogs: RefCell::default(),
            dialog_policies: RefCell::default(),
            file_picker: RefCell::default(),
            favicon_changes: RefCell::default(),
            page_cursor: Cell::new(Some(CursorIcon::Default)),
            page_message: RefCell::default(),
            last_gesture: Cell::new(None),
            last_cleaned: RefCell::default(),
            popups_blocked: Cell::new(0),
            load_generation: Cell::new(0),
            agent: Cell::new(None),
            exit_requested: Cell::new(false),
        })
    }

    pub fn servo(&self) -> &Servo {
        &self.servo
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn rendering_context(&self) -> &WindowRenderingContext {
        &self.rendering_context
    }

    pub fn offscreen(&self) -> &OffscreenRenderingContext {
        &self.offscreen
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested.get()
    }

    pub fn request_exit(&self) {
        self.exit_requested.set(true);
    }

    pub fn load_generation(&self) -> u64 {
        self.load_generation.get()
    }

    fn bump_load_generation(&self) {
        self.load_generation.set(self.load_generation.get() + 1);
    }

    pub fn agent(&self) -> Option<bool> {
        self.agent.get()
    }

    pub fn set_agent(&self, state: Option<bool>) {
        if self.agent.replace(state) != state {
            self.window.request_redraw();
        }
    }

    pub fn search_engine_name(&self) -> &str {
        self.search.name
    }

    pub fn blocked_count(&self) -> u64 {
        self.shield.stats().total_blocked() + self.popups_blocked.get()
    }

    fn delegate(&self) -> Rc<dyn WebViewDelegate> {
        self.me
            .upgrade()
            .expect("el navegador sigue vivo mientras haya pestañas")
    }

    fn scale(&self) -> Scale<f32, DeviceIndependentPixel, DevicePixel> {
        Scale::new(self.window.scale_factor() as f32)
    }

    // ------------------------------------------------------------------
    // Pestañas
    // ------------------------------------------------------------------

    pub fn open_tab(&self, url: Url) {
        let webview = WebViewBuilder::new(&self.servo, self.offscreen.clone())
            .url(url)
            .hidpi_scale_factor(self.scale())
            .delegate(self.delegate())
            .user_content_manager(self.user_content.clone())
            .build();
        self.add_tab(webview);
    }

    /// Abre en una pestaña nueva lo que se escribió (dirección o búsqueda).
    /// Devuelve `false` si no hay nada que abrir.
    pub fn open_tab_for(&self, input: &str) -> bool {
        match omnibox::resolve(input, &self.search) {
            Some(url) => {
                self.open_tab(url);
                true
            },
            None => false,
        }
    }

    fn add_tab(&self, webview: WebView) {
        let index = {
            let mut tabs = self.tabs.borrow_mut();
            tabs.push(webview);
            tabs.len() - 1
        };
        self.activate(index);
    }

    pub fn active_webview(&self) -> Option<WebView> {
        self.tabs.borrow().get(self.active.get()).cloned()
    }

    pub fn active_index(&self) -> usize {
        self.active.get()
    }

    pub fn active_page_loaded(&self) -> bool {
        self.active_webview()
            .is_some_and(|webview| webview.load_status() == LoadStatus::Complete)
    }

    fn is_active(&self, webview: &WebView) -> bool {
        self.active_webview()
            .is_some_and(|active| active == *webview)
    }

    pub fn activate(&self, index: usize) {
        // Copia de los manejadores (son `Rc`): no mantenemos el préstamo de
        // `tabs` mientras llamamos a Servo.
        let tabs: Vec<WebView> = self.tabs.borrow().clone();
        let Some(target) = tabs.get(index) else {
            return;
        };
        self.active.set(index);
        for (i, webview) in tabs.iter().enumerate() {
            if i != index {
                webview.set_focused(false);
                webview.hide();
            }
        }
        target.show();
        target.set_focused(true);
        target.resize(self.viewport_size.get());
        self.update_title();
        self.window.request_redraw();
    }

    pub fn close_active_tab(&self) {
        self.close_tab(self.active.get());
    }

    pub fn close_tab(&self, index: usize) {
        let (removed, remaining) = {
            let mut tabs = self.tabs.borrow_mut();
            if index >= tabs.len() {
                return;
            }
            (tabs.remove(index), tabs.len())
        };
        // Si tenía un aviso abierto, al soltarlo Servo recibe "cancelar".
        self.dialogs.borrow_mut().remove(&removed.id());
        self.dialog_policies.borrow_mut().remove(&removed.id());
        drop(removed);

        if remaining == 0 {
            self.exit_requested.set(true);
            return;
        }
        let active = self.active.get();
        let next = if index < active {
            active - 1
        } else {
            active.min(remaining - 1)
        };
        self.activate(next);
    }

    fn remove_webview(&self, webview: &WebView) {
        let position = self.tabs.borrow().iter().position(|tab| tab == webview);
        if let Some(index) = position {
            self.close_tab(index);
        }
    }

    pub fn cycle_tab(&self, forward: bool) {
        let count = self.tabs.borrow().len();
        if count < 2 {
            return;
        }
        let active = self.active.get();
        let next = if forward {
            (active + 1) % count
        } else {
            (active + count - 1) % count
        };
        self.activate(next);
    }

    pub fn tab_infos(&self) -> Vec<TabInfo> {
        self.tabs
            .borrow()
            .iter()
            .map(|webview| TabInfo {
                id: webview.id(),
                title: tab_title(webview),
                url: webview.url(),
                loading: is_loading(webview),
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Navegación
    // ------------------------------------------------------------------

    /// Abre lo que el usuario escribió (dirección o búsqueda) en la pestaña activa.
    pub fn navigate(&self, input: &str) {
        let Some(url) = omnibox::resolve(input, &self.search) else {
            return;
        };
        match self.active_webview() {
            Some(webview) => webview.load(url),
            None => self.open_tab(url),
        }
    }

    pub fn home(&self) {
        match self.active_webview() {
            Some(webview) => webview.load(pages::start_url()),
            None => self.open_tab(pages::start_url()),
        }
    }

    pub fn go_back(&self) {
        if let Some(webview) = self.active_webview().filter(WebView::can_go_back) {
            webview.go_back(1);
        }
    }

    pub fn go_forward(&self) {
        if let Some(webview) = self.active_webview().filter(WebView::can_go_forward) {
            webview.go_forward(1);
        }
    }

    pub fn reload(&self) {
        if let Some(webview) = self.active_webview() {
            webview.reload();
        }
    }

    pub fn zoom(&self, factor: Option<f32>) {
        if let Some(webview) = self.active_webview() {
            let zoom = factor.map_or(1.0, |factor| webview.page_zoom() * factor);
            webview.set_page_zoom(zoom);
        }
    }

    pub fn navigation_state(&self) -> NavigationState {
        let webview = self.active_webview();
        let url = webview.as_ref().and_then(WebView::url);
        NavigationState {
            can_go_back: webview.as_ref().is_some_and(WebView::can_go_back),
            can_go_forward: webview.as_ref().is_some_and(WebView::can_go_forward),
            has_page: webview.is_some(),
            loading: webview.as_ref().is_some_and(is_loading),
            location: url.as_ref().map_or(Location::Internal, location_of),
        }
    }

    /// Texto que muestra la barra de direcciones cuando no se está editando.
    pub fn display_url(&self) -> String {
        self.active_webview()
            .and_then(|webview| webview.url())
            .map(|url| display_url_of(&url))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Ventana y dibujo
    // ------------------------------------------------------------------

    /// Servo pinta la pestaña activa en su búfer. La interfaz la copia después.
    pub fn paint_page(&self) {
        if let Some(webview) = self.active_webview() {
            webview.paint();
        }
    }

    pub fn resize_window(&self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.rendering_context.resize(size);
        }
        self.window.request_redraw();
    }

    /// Tamaño de la zona de la página, en píxeles físicos.
    pub fn viewport_size(&self) -> PhysicalSize<u32> {
        self.viewport_size.get()
    }

    /// Ajusta las páginas al hueco que deja la interfaz.
    pub fn resize_viewport(&self, size: PhysicalSize<u32>) {
        if size == self.viewport_size.get() {
            return;
        }
        self.viewport_size.set(size);
        // Todas las pestañas comparten el búfer: redimensionar una basta.
        if let Some(webview) = self.active_webview() {
            webview.resize(size);
        }
        self.window.request_redraw();
    }

    pub fn set_scale_factor(&self, scale_factor: f64) {
        let scale = Scale::new(scale_factor as f32);
        for webview in self.tabs.borrow().clone() {
            webview.set_hidpi_scale_factor(scale);
        }
    }

    pub fn note_user_gesture(&self) {
        self.last_gesture.set(Some(Instant::now()));
    }

    /// Vuelve a poner el cursor que pidió la página (al entrar el ratón en ella).
    pub fn apply_page_cursor(&self) {
        match self.page_cursor.get() {
            Some(icon) => {
                self.window.set_cursor_visible(true);
                self.window.set_cursor(icon);
            },
            None => self.window.set_cursor_visible(false),
        }
    }

    // ------------------------------------------------------------------
    // Controles que pide la página
    // ------------------------------------------------------------------

    /// Saca el control abierto de la pestaña activa para dibujarlo. Si era de
    /// otra pestaña, se cancela.
    pub fn take_control(&self) -> Option<PageControl> {
        let (webview_id, control) = self.control.borrow_mut().take()?;
        let active = self.active_webview().map(|webview| webview.id());
        (active == Some(webview_id)).then_some(control)
    }

    pub fn put_back_control(&self, control: PageControl) {
        if let Some(webview) = self.active_webview() {
            *self.control.borrow_mut() = Some((webview.id(), control));
        }
    }

    /// Recuerda dónde se hizo clic derecho en la página.
    pub fn note_context_click(&self, point: DevicePoint) {
        self.context_point.set(Some(point));
    }

    /// Saca el aviso pendiente de la pestaña activa para dibujarlo.
    pub fn take_dialog(&self) -> Option<PageDialog> {
        let id = self.active_webview()?.id();
        self.dialogs.borrow_mut().remove(&id)
    }

    pub fn put_back_dialog(&self, dialog: PageDialog) {
        if let Some(webview) = self.active_webview() {
            self.dialogs.borrow_mut().insert(webview.id(), dialog);
        }
    }

    /// Responde al aviso. Con `mute`, esta página ya no podrá mostrar más.
    pub fn resolve_dialog(&self, dialog: PageDialog, accept: bool) {
        if dialog.mute
            && let Some(webview) = self.active_webview()
            && let Some(policy) = self.dialog_policies.borrow_mut().get_mut(&webview.id())
        {
            policy.muted = true;
        }
        dialog.answer(accept);
        self.window.request_redraw();
    }

    /// El aviso pendiente de la pestaña activa: tipo, dominio y texto.
    pub fn active_dialog(&self) -> Option<(DialogKind, String, String)> {
        let id = self.active_webview()?.id();
        let dialogs = self.dialogs.borrow();
        let dialog = dialogs.get(&id)?;
        Some((dialog.kind(), dialog.origin.clone(), dialog.message.clone()))
    }

    /// Responde al aviso de la pestaña activa (para el agente, por MCP).
    pub fn answer_active_dialog(&self, accept: bool, text: Option<&str>) -> bool {
        let Some(mut dialog) = self.take_dialog() else {
            return false;
        };
        if let Some(text) = text {
            dialog.text = text.to_owned();
        }
        self.resolve_dialog(dialog, accept);
        true
    }

    fn show_dialog(&self, webview: &WebView, dialog: SimpleDialog) {
        let origin = dialog_origin(webview);
        let shown = {
            let mut policies = self.dialog_policies.borrow_mut();
            let policy = policies.entry(webview.id()).or_default();
            if policy.origin != origin {
                *policy = DialogPolicy {
                    origin: origin.clone(),
                    ..DialogPolicy::default()
                };
            }
            if policy.muted {
                log::info!("Aviso de página silenciado");
                PageDialog {
                    dialog,
                    origin,
                    message: String::new(),
                    text: String::new(),
                    shown: 0,
                    mute: true,
                }
                .answer(false);
                return;
            }
            policy.shown += 1;
            policy.shown
        };
        let (message, text) = match &dialog {
            SimpleDialog::Alert(alert) => (alert.message().to_owned(), String::new()),
            SimpleDialog::Confirm(confirm) => (confirm.message().to_owned(), String::new()),
            SimpleDialog::Prompt(prompt) => (
                prompt.message().to_owned(),
                prompt.current_value().to_owned(),
            ),
        };
        let dialog = PageDialog {
            dialog,
            origin,
            message: sanitize_multiline(&message, DIALOG_MAX_CHARS),
            text: sanitize(&text, 500),
            shown,
            mute: false,
        };
        // Si quedaba otro sin responder, al soltarlo Servo recibe "cancelar".
        self.dialogs.borrow_mut().insert(webview.id(), dialog);
        self.window.request_redraw();
    }

    /// Abre el diálogo del sistema para subir archivos, si una página lo pidió.
    /// Se llama fuera de los avisos de Servo para no bloquearlo a medias.
    pub fn run_file_picker(&self) {
        let Some(mut picker) = self.file_picker.borrow_mut().take() else {
            return;
        };
        let extensions: Vec<String> = picker
            .filter_patterns()
            .iter()
            .map(|pattern| pattern.0.trim().trim_start_matches('.').to_owned())
            .filter(|ext| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .collect();

        let mut dialog = rfd::FileDialog::new()
            .set_title("Elegir archivo para subir")
            .set_parent(&self.window);
        if !extensions.is_empty() {
            dialog = dialog.add_filter("Archivos que acepta la página", &extensions);
        }
        let paths = if picker.allow_select_multiple() {
            dialog.pick_files()
        } else {
            dialog.pick_file().map(|path| vec![path])
        };
        match paths {
            Some(paths) => {
                // Solo cuántos, nunca qué archivos: los nombres son privados.
                log::info!("{} archivo(s) elegidos para subir", paths.len());
                picker.select(&paths);
                picker.submit();
            },
            None => picker.dismiss(),
        }
        self.window.request_redraw();
    }

    /// Iconos de pestaña que cambiaron desde la última vez.
    pub fn take_favicon_changes(&self) -> Vec<(WebViewId, Option<Image>)> {
        let changed = std::mem::take(&mut *self.favicon_changes.borrow_mut());
        let tabs = self.tabs.borrow();
        changed
            .into_iter()
            .filter_map(|id| {
                let webview = tabs.iter().find(|webview| webview.id() == id)?;
                Some((id, webview.favicon().map(|image| image.clone())))
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Mensajes y título
    // ------------------------------------------------------------------

    fn show_page_message(&self, message: &str) {
        *self.page_message.borrow_mut() = Some((sanitize(message, 120), Instant::now()));
        self.window.request_redraw();
    }

    pub fn page_message(&self) -> Option<String> {
        let mut slot = self.page_message.borrow_mut();
        match slot.as_ref() {
            Some((message, shown_at)) if shown_at.elapsed() < PAGE_MESSAGE_DURATION => {
                Some(message.clone())
            },
            Some(_) => {
                slot.take();
                None
            },
            None => None,
        }
    }

    pub fn page_message_remaining(&self) -> Option<Duration> {
        let slot = self.page_message.borrow();
        let (_, shown_at) = slot.as_ref()?;
        PAGE_MESSAGE_DURATION.checked_sub(shown_at.elapsed())
    }

    pub fn dismiss_message(&self) {
        self.page_message.borrow_mut().take();
        self.window.request_redraw();
    }

    /// El título de la ventana (barra de tareas, Alt+Tab): solo el de la página.
    pub fn update_title(&self) {
        let page_title = self
            .active_webview()
            .and_then(|webview| webview.page_title())
            .map(|title| sanitize(&title, 80))
            .filter(|title| !title.is_empty());
        let title = match page_title {
            Some(page_title) => format!("{page_title} — Ferrum"),
            None => "Ferrum".to_owned(),
        };
        self.window.set_title(&title);
    }

    /// Algo cambió en una pestaña: redibujar la interfaz.
    fn refresh(&self, webview: &WebView) {
        if self.is_active(webview) {
            self.update_title();
        }
        self.window.request_redraw();
    }

    // ------------------------------------------------------------------
    // Filtro de red
    // ------------------------------------------------------------------

    fn filter_request(&self, webview: &WebView, load: WebResourceLoad) {
        let request = load.request();
        let url = request.url.as_url().clone();

        if request.is_for_main_frame {
            let is_get = request.method == Method::GET;
            self.filter_document(load, url, is_get);
            self.refresh(webview);
            return;
        }

        // Ningún recurso de una página puede cargar las páginas internas.
        if pages::is_internal(&url) {
            cancel(load, url);
            return;
        }

        let top_level = webview.url();
        if let Verdict::Block(reason) = self.shield.check_subresource(&url, top_level.as_ref()) {
            log::debug!("Bloqueado ({}): {}", reason.code(), reason.detail());
            cancel(load, url);
            self.refresh(webview);
        }
    }

    fn filter_document(&self, load: WebResourceLoad, url: Url, is_get: bool) {
        if pages::is_internal(&url) {
            let html = pages::start_page(&self.search, self.blocked_count());
            respond_html(load, &url, StatusCode::OK, html, pages::START_PAGE_CSP);
            return;
        }

        match self.shield.check_document(&url) {
            Verdict::Allow => {},
            Verdict::Block(reason) => {
                log::info!("Página bloqueada ({}): {}", reason.code(), reason.detail());
                let html = pages::block_page(&url, &reason);
                respond_html(
                    load,
                    &url,
                    StatusCode::FORBIDDEN,
                    html,
                    pages::NOTICE_PAGE_CSP,
                );
            },
            Verdict::Redirect(clean) => {
                // Solo GET: limpiar un POST rompería el envío de formularios.
                let looping = self
                    .last_cleaned
                    .borrow()
                    .as_ref()
                    .is_some_and(|(previous, at)| {
                        *previous == url && at.elapsed() < Duration::from_secs(10)
                    });
                if is_get && !looping {
                    *self.last_cleaned.borrow_mut() = Some((url.clone(), Instant::now()));
                    let html = pages::redirect_page(&clean);
                    respond_html(load, &url, StatusCode::OK, html, pages::NOTICE_PAGE_CSP);
                }
            },
        }
    }
}

impl WebViewDelegate for Browser {
    fn notify_new_frame_ready(&self, webview: WebView) {
        if self.is_active(&webview) {
            self.window.request_redraw();
        }
    }

    fn notify_url_changed(&self, webview: WebView, _url: Url) {
        self.bump_load_generation();
        self.refresh(&webview);
    }

    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        self.refresh(&webview);
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        // Una pestaña nueva ya nace en `Started`, sin aviso: por eso cuenta
        // cualquier cambio, y también los de dirección.
        self.bump_load_generation();
        if status == LoadStatus::Complete {
            // Solo el dominio, nunca la URL completa, ni siquiera en depuración.
            let host = webview
                .url()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_default();
            log::debug!(
                "Página cargada: {host} (bloqueados en la sesión: {})",
                self.shield.stats().total_blocked()
            );
        }
        self.refresh(&webview);
    }

    fn notify_favicon_changed(&self, webview: WebView) {
        self.favicon_changes.borrow_mut().push(webview.id());
        self.window.request_redraw();
    }

    fn notify_cursor_changed(&self, webview: WebView, cursor: Cursor) {
        if !self.is_active(&webview) {
            return;
        }
        self.page_cursor.set(input::cursor_icon(cursor));
        self.apply_page_cursor();
    }

    fn notify_closed(&self, webview: WebView) {
        self.remove_webview(&webview);
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        log::error!("Una pestaña falló: {reason}");
        if self.is_active(&webview) {
            self.show_page_message("esta pestaña dejó de funcionar; pulsa F5 para recargarla");
        }
    }

    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        // Las páginas web nunca pueden abrir archivos de tu disco.
        if request.url.scheme() == "file" {
            log::warn!("Navegación a archivo local bloqueada");
            request.deny();
        } else {
            request.allow();
        }
    }

    fn request_create_new(&self, _parent: WebView, request: CreateNewWebViewRequest) {
        let user_initiated = self
            .last_gesture
            .get()
            .is_some_and(|at| at.elapsed() < POPUP_GESTURE_WINDOW);
        if !user_initiated {
            log::info!("Ventana emergente bloqueada");
            self.popups_blocked.set(self.popups_blocked.get() + 1);
            self.window.request_redraw();
            return;
        }
        let webview = request
            .builder(self.offscreen.clone())
            .hidpi_scale_factor(self.scale())
            .delegate(self.delegate())
            .user_content_manager(self.user_content.clone())
            .build();
        self.add_tab(webview);
    }

    fn request_permission(&self, _webview: WebView, request: PermissionRequest) {
        // Cámara, micrófono, ubicación, notificaciones…: denegado sin preguntar.
        log::info!("Permiso denegado: {:?}", request.feature());
        request.deny();
    }

    fn show_embedder_control(&self, webview: WebView, control: EmbedderControl) {
        log::debug!("La página pide un control: {}", control_kind(&control));
        match control {
            // alert/confirm/prompt: un cuadro de Ferrum encima de la página, con
            // el dominio que lo pide. Solo pausa esa pestaña, y a partir del
            // segundo aviso se pueden silenciar (contra "¡Tu PC está infectado!").
            EmbedderControl::SimpleDialog(dialog) => self.show_dialog(&webview, dialog),
            EmbedderControl::SelectElement(select) => {
                *self.control.borrow_mut() = Some((webview.id(), PageControl::Select(select)));
                self.window.request_redraw();
            },
            EmbedderControl::ColorPicker(picker) => {
                let color = gui::rgb_to_color(picker.current_color());
                *self.control.borrow_mut() =
                    Some((webview.id(), PageControl::Color { picker, color }));
                self.window.request_redraw();
            },
            // Solo la pestaña activa puede pedir archivos, y siempre tras un
            // clic del usuario en el formulario.
            EmbedderControl::FilePicker(picker) if self.is_active(&webview) => {
                *self.file_picker.borrow_mut() = Some(picker);
            },
            EmbedderControl::FilePicker(picker) => picker.dismiss(),
            EmbedderControl::ContextMenu(menu) if self.is_active(&webview) => {
                let at = self.context_point.take();
                *self.control.borrow_mut() = Some((webview.id(), PageControl::Menu { menu, at }));
                self.window.request_redraw();
            },
            EmbedderControl::ContextMenu(menu) => menu.dismiss(),
            EmbedderControl::InputMethod(_) => {},
        }
    }

    fn hide_embedder_control(&self, _webview: WebView, control_id: EmbedderControlId) {
        log::debug!("La página cierra su control");
        let mut slot = self.control.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(_, control)| control.id() == control_id)
        {
            slot.take();
            self.window.request_redraw();
        }
    }

    fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
        self.filter_request(&webview, load);
    }

    fn show_console_message(&self, _webview: WebView, _level: ConsoleLogLevel, message: String) {
        log::debug!("[consola] {message}");
    }
}

fn control_kind(control: &EmbedderControl) -> &'static str {
    match control {
        EmbedderControl::SelectElement(_) => "lista desplegable",
        EmbedderControl::ColorPicker(_) => "selector de color",
        EmbedderControl::FilePicker(_) => "subir archivo",
        EmbedderControl::InputMethod(_) => "método de entrada",
        EmbedderControl::SimpleDialog(_) => "diálogo",
        EmbedderControl::ContextMenu(_) => "menú contextual",
    }
}

/// Tamaño de la página antes del primer dibujo: la ventana menos la interfaz.
fn initial_viewport_size(window: &Window) -> PhysicalSize<u32> {
    let size = window.inner_size();
    let chrome = ((TAB_BAR_HEIGHT + TOOLBAR_HEIGHT) * window.scale_factor() as f32).round() as u32;
    PhysicalSize::new(size.width.max(1), size.height.saturating_sub(chrome).max(1))
}

/// Dominio que pide un aviso, tal y como se muestra en el cuadro.
fn dialog_origin(webview: &WebView) -> String {
    webview
        .url()
        .filter(|url| !pages::is_internal(url))
        .and_then(|url| url.host_str().map(str::to_owned))
        .map(|host| sanitize(&host, 80))
        .unwrap_or_else(|| "Esta página".to_owned())
}

/// Como [`sanitize`], pero conserva los saltos de línea (hasta 20 líneas).
fn sanitize_multiline(text: &str, max_chars: usize) -> String {
    let lines: Vec<String> = text
        .lines()
        .take(20)
        .map(|line| sanitize(&line.replace('\t', " "), max_chars))
        .collect();
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.chars().count() > max_chars {
        let cut: String = trimmed.chars().take(max_chars).collect();
        format!("{cut}…")
    } else {
        trimmed.to_owned()
    }
}

fn is_loading(webview: &WebView) -> bool {
    matches!(
        webview.load_status(),
        LoadStatus::Started | LoadStatus::HeadParsed
    )
}

/// Título de la pestaña: el de la página, o el dominio mientras no hay título.
fn tab_title(webview: &WebView) -> String {
    let url = webview.url();
    if url.as_ref().is_some_and(pages::is_internal) {
        return "Nueva pestaña".to_owned();
    }
    if let Some(title) = webview
        .page_title()
        .map(|title| sanitize(&title, 60))
        .filter(|title| !title.is_empty())
    {
        return title;
    }
    url.and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Cargando…".to_owned())
}

fn location_of(url: &Url) -> Location {
    if pages::is_internal(url) {
        return Location::Internal;
    }
    match url.scheme() {
        "https" => Location::Secure,
        "http" => Location::Insecure,
        _ => Location::Other,
    }
}

/// La dirección tal y como se muestra. Los dominios internacionales salen en
/// punycode (`xn--…`) para que uno que imita a otro con letras de otro alfabeto
/// no pase desapercibido.
fn display_url_of(url: &Url) -> String {
    if pages::is_internal(url) {
        return String::new();
    }
    sanitize(url.as_str(), 2048)
}

/// Limpia texto controlado por la web antes de mostrarlo en la interfaz:
/// quita caracteres de control y de dirección bidi (con los que se puede
/// invertir texto para falsificar dominios) y lo recorta.
fn sanitize(text: &str, max_chars: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|c| {
            !c.is_control() &&
                !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > max_chars {
        let truncated: String = clean.chars().take(max_chars).collect();
        format!("{truncated}…")
    } else {
        clean
    }
}

fn cancel(load: WebResourceLoad, url: Url) {
    load.intercept(WebResourceResponse::new(url).status_code(StatusCode::FORBIDDEN))
        .cancel();
}

fn respond_html(load: WebResourceLoad, url: &Url, status: StatusCode, html: String, csp: &str) {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(csp) {
        headers.insert(CONTENT_SECURITY_POLICY, value);
    }
    let reason = status.canonical_reason().unwrap_or_default();
    let response = WebResourceResponse::new(url.clone())
        .headers(headers)
        .status_code(status)
        .status_message(reason.as_bytes().to_vec());
    let mut intercepted = load.intercept(response);
    intercepted.send_body_data(html.into_bytes());
    intercepted.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_bidi_and_controls() {
        assert_eq!(
            sanitize("Hola\u{202E}moc.knab\n mundo", 80),
            "Holamoc.knab mundo"
        );
        assert_eq!(sanitize("abcdef", 3), "abc…");
    }

    #[test]
    fn dialog_text_keeps_lines_but_not_tricks() {
        assert_eq!(
            sanitize_multiline("Hola\u{202E}mundo\nsegunda\tlínea\n", 100),
            "Holamundo\nsegunda línea"
        );
        let long = "a".repeat(50);
        assert_eq!(
            sanitize_multiline(&long, 10),
            format!("{}…", "a".repeat(10))
        );
    }

    #[test]
    fn address_shows_punycode() {
        let url = Url::parse("https://аpple.com/").expect("url");
        assert!(display_url_of(&url).starts_with("https://xn--"));
        assert_eq!(location_of(&url), Location::Secure);
    }

    #[test]
    fn location_kinds() {
        let http = Url::parse("http://example.com/").expect("url");
        assert_eq!(location_of(&http), Location::Insecure);
        assert_eq!(location_of(&pages::start_url()), Location::Internal);
        assert_eq!(display_url_of(&pages::start_url()), "");
    }
}
