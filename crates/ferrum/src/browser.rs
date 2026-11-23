/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! El navegador: pestañas, barra de direcciones y la conexión entre Servo y el
//! escudo de privacidad.
//!
//! Toda petición de red pasa por [`Browser::filter_request`] antes de salir del
//! equipo. Ahí se decide si se bloquea, se limpia o se deja pasar.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use euclid::Scale;
use ferrum_shield::omnibox::{self, SearchEngine};
use ferrum_shield::{Shield, Verdict};
use http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use servo::{
    ConsoleLogLevel, CreateNewWebViewRequest, Cursor, DeviceIndependentPixel, DevicePixel,
    EmbedderControl, LoadStatus, NavigationRequest, PermissionRequest, RenderingContext, Servo,
    SimpleDialog, UserContentManager, UserScript, WebResourceLoad, WebResourceResponse, WebView,
    WebViewBuilder, WebViewDelegate, WindowRenderingContext,
};
use url::Url;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::input;
use crate::pages;

/// Script anti-huella digital inyectado en cada página.
const PRIVACY_SCRIPT: &str = include_str!("../resources/privacy.js");

/// Una página solo puede abrir ventanas nuevas justo después de un clic o una
/// tecla del usuario. Así se bloquean los pop-ups y pop-unders automáticos.
const POPUP_GESTURE_WINDOW: Duration = Duration::from_millis(1500);

/// Cuánto tiempo se muestra en el título un mensaje de `alert()`.
const PAGE_MESSAGE_DURATION: Duration = Duration::from_secs(8);

/// Texto que el usuario está escribiendo en la barra de direcciones.
#[derive(Debug, Default)]
pub struct AddressBar {
    pub text: String,
    /// La primera tecla reemplaza el texto (como si estuviera seleccionado).
    pub replace_on_type: bool,
}

pub struct Browser {
    me: Weak<Browser>,
    window: Window,
    servo: Servo,
    rendering_context: Rc<WindowRenderingContext>,
    user_content: Rc<UserContentManager>,
    shield: Shield,
    search: SearchEngine,
    tabs: RefCell<Vec<WebView>>,
    active: Cell<usize>,
    address_bar: RefCell<Option<AddressBar>>,
    page_message: RefCell<Option<(String, Instant)>>,
    last_gesture: Cell<Option<Instant>>,
    /// Última URL limpiada, para no entrar en bucle si el servidor insiste en
    /// redirigir a la versión con parámetros de rastreo.
    last_cleaned: RefCell<Option<(Url, Instant)>>,
    popups_blocked: Cell<u64>,
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

        Rc::new_cyclic(|me| Browser {
            me: me.clone(),
            window,
            servo,
            rendering_context,
            user_content,
            shield,
            search,
            tabs: RefCell::default(),
            active: Cell::new(0),
            address_bar: RefCell::default(),
            page_message: RefCell::default(),
            last_gesture: Cell::new(None),
            last_cleaned: RefCell::default(),
            popups_blocked: Cell::new(0),
            exit_requested: Cell::new(false),
        })
    }

    pub fn servo(&self) -> &Servo {
        &self.servo
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested.get()
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
        let webview = WebViewBuilder::new(&self.servo, self.rendering_context.clone())
            .url(url)
            .hidpi_scale_factor(self.scale())
            .delegate(self.delegate())
            .user_content_manager(self.user_content.clone())
            .build();
        self.add_tab(webview);
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

    fn is_active(&self, webview: &WebView) -> bool {
        self.active_webview()
            .is_some_and(|active| active == *webview)
    }

    fn activate(&self, index: usize) {
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
        target.resize(self.window.inner_size());
        self.update_title();
        self.window.request_redraw();
    }

    pub fn close_active_tab(&self) {
        self.close_tab(self.active.get());
    }

    fn close_tab(&self, index: usize) {
        let (removed, remaining) = {
            let mut tabs = self.tabs.borrow_mut();
            if index >= tabs.len() {
                return;
            }
            (tabs.remove(index), tabs.len())
        };
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

    // ------------------------------------------------------------------
    // Ventana
    // ------------------------------------------------------------------

    pub fn paint(&self) {
        if let Some(webview) = self.active_webview() {
            webview.paint();
            self.rendering_context.present();
        }
    }

    pub fn resize(&self, size: PhysicalSize<u32>) {
        // Todas las pestañas comparten el contexto de dibujo: redimensionar
        // una las redimensiona todas.
        if let Some(webview) = self.active_webview() {
            webview.resize(size);
        }
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

    // ------------------------------------------------------------------
    // Barra de direcciones (se muestra en el título de la ventana)
    // ------------------------------------------------------------------

    pub fn is_editing_address(&self) -> bool {
        self.address_bar.borrow().is_some()
    }

    pub fn begin_address_edit(&self) {
        let current = self
            .active_webview()
            .and_then(|webview| webview.url())
            .filter(|url| !pages::is_internal(url))
            .map(String::from)
            .unwrap_or_default();
        *self.address_bar.borrow_mut() = Some(AddressBar {
            replace_on_type: !current.is_empty(),
            text: current,
        });
        self.update_title();
    }

    pub fn address_type(&self, text: &str) {
        if let Some(bar) = self.address_bar.borrow_mut().as_mut() {
            if bar.replace_on_type {
                bar.text.clear();
                bar.replace_on_type = false;
            }
            bar.text.extend(text.chars().filter(|c| !c.is_control()));
        }
        self.update_title();
    }

    pub fn address_backspace(&self) {
        if let Some(bar) = self.address_bar.borrow_mut().as_mut() {
            if bar.replace_on_type {
                bar.text.clear();
                bar.replace_on_type = false;
            } else {
                bar.text.pop();
            }
        }
        self.update_title();
    }

    pub fn address_select_all(&self) {
        if let Some(bar) = self.address_bar.borrow_mut().as_mut() {
            bar.replace_on_type = !bar.text.is_empty();
        }
        self.update_title();
    }

    pub fn address_paste(&self) {
        let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
        match text {
            Ok(text) => self.address_type(&text.replace(['\r', '\n'], " ")),
            Err(error) => log::warn!("No se pudo leer el portapapeles: {error}"),
        }
    }

    pub fn commit_address(&self) {
        let bar = self.address_bar.borrow_mut().take();
        if let Some(bar) = bar {
            self.navigate(&bar.text);
        }
        self.update_title();
    }

    pub fn cancel_address(&self) {
        self.address_bar.borrow_mut().take();
        self.update_title();
    }

    // ------------------------------------------------------------------
    // Título
    // ------------------------------------------------------------------

    fn show_page_message(&self, message: &str) {
        *self.page_message.borrow_mut() = Some((sanitize(message, 80), Instant::now()));
        self.update_title();
    }

    pub fn update_title(&self) {
        let title = match self.address_bar.borrow().as_ref() {
            Some(bar) => {
                let text = if bar.replace_on_type {
                    format!("[{}]", bar.text)
                } else {
                    format!("{}▏", bar.text)
                };
                format!("🔎 {text}    (Enter: ir · Esc: cancelar)")
            },
            None => self.status_title(),
        };
        self.window.set_title(&title);
    }

    fn status_title(&self) -> String {
        let mut title = String::new();

        if let Some(webview) = self.active_webview() {
            // La ubicación va primero y fuera del control de la página: un
            // título malicioso no puede empujarla ni suplantarla.
            let location = webview
                .url()
                .map_or_else(String::new, |url| describe_location(&url));
            title.push_str(&location);
            if matches!(
                webview.load_status(),
                LoadStatus::Started | LoadStatus::HeadParsed
            ) {
                title.push_str(" ⏳");
            }
            if let Some(page_title) = webview
                .page_title()
                .map(|t| sanitize(&t, 60))
                .filter(|t| !t.is_empty())
            {
                title.push_str(" — ");
                title.push_str(&page_title);
            }
        }

        let message = self.page_message.borrow().clone();
        if let Some((message, shown_at)) = message {
            if shown_at.elapsed() < PAGE_MESSAGE_DURATION {
                title.push_str(&format!(" · 💬 La página dice: «{message}»"));
            } else {
                self.page_message.borrow_mut().take();
            }
        }

        let stats = self.shield.stats();
        title.push_str(&format!(
            " · 🛡 {}",
            stats.total_blocked() + self.popups_blocked.get()
        ));

        let tabs = self.tabs.borrow().len();
        if tabs > 1 {
            title.push_str(&format!(" · pestaña {}/{tabs}", self.active.get() + 1));
        }
        title.push_str(" — Ferrum");
        title
    }

    fn refresh_if_active(&self, webview: &WebView) {
        if self.is_active(webview) && !self.is_editing_address() {
            self.update_title();
        }
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
            self.refresh_if_active(webview);
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
            self.refresh_if_active(webview);
        }
    }

    fn filter_document(&self, load: WebResourceLoad, url: Url, is_get: bool) {
        if pages::is_internal(&url) {
            let html = pages::start_page(&self.search);
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
        self.refresh_if_active(&webview);
    }

    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        self.refresh_if_active(&webview);
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
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
        self.refresh_if_active(&webview);
    }

    fn notify_cursor_changed(&self, webview: WebView, cursor: Cursor) {
        if !self.is_active(&webview) {
            return;
        }
        match input::cursor_icon(cursor) {
            Some(icon) => {
                self.window.set_cursor_visible(true);
                self.window.set_cursor(icon);
            },
            None => self.window.set_cursor_visible(false),
        }
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
            self.update_title();
            return;
        }
        let webview = request
            .builder(self.rendering_context.clone())
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

    fn show_embedder_control(&self, _webview: WebView, control: EmbedderControl) {
        match control {
            // alert/confirm/prompt: se muestran en el título, claramente marcados
            // como texto de la página, y se cierran solos. Así una web no puede
            // bloquearte con diálogos en bucle ("¡Tu PC está infectado!").
            EmbedderControl::SimpleDialog(dialog) => match dialog {
                SimpleDialog::Alert(alert) => {
                    self.show_page_message(alert.message());
                    alert.confirm();
                },
                SimpleDialog::Confirm(confirm) => {
                    self.show_page_message(confirm.message());
                    confirm.dismiss();
                },
                SimpleDialog::Prompt(prompt) => {
                    self.show_page_message(prompt.message());
                    prompt.dismiss();
                },
            },
            EmbedderControl::ContextMenu(menu) => menu.dismiss(),
            // Subir archivos queda deshabilitado en esta versión.
            EmbedderControl::FilePicker(picker) => picker.dismiss(),
            _ => {},
        }
    }

    fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
        self.filter_request(&webview, load);
    }

    fn show_console_message(&self, _webview: WebView, _level: ConsoleLogLevel, message: String) {
        log::debug!("[consola] {message}");
    }
}

/// Describe la ubicación para el título: candado + dominio.
/// Los dominios internacionales se muestran en punycode (`xn--…`) para que un
/// dominio que imita a otro con letras de otro alfabeto no pase desapercibido.
fn describe_location(url: &Url) -> String {
    if pages::is_internal(url) {
        return "ferrum://inicio".to_owned();
    }
    let host = url.host_str().unwrap_or_default();
    match url.scheme() {
        "https" => format!("🔒 {host}"),
        "http" => format!("⚠ {host} (sin cifrar)"),
        "about" | "data" | "blob" => url.scheme().to_owned() + ":",
        other => format!("{other}://{host}"),
    }
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
    fn location_shows_lock_and_punycode() {
        let url = Url::parse("https://аpple.com/").expect("url");
        assert!(describe_location(&url).starts_with("🔒 xn--"));
        let url = Url::parse("http://example.com/").expect("url");
        assert_eq!(describe_location(&url), "⚠ example.com (sin cifrar)");
    }
}
