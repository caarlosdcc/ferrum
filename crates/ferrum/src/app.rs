/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Bucle de eventos de la ventana (winit): reparte ratón y teclado entre la
//! interfaz y la página, y compone cada fotograma.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ferrum_shield::omnibox;
use ferrum_shield::{Category, Shield, ShieldConfig};
use servo::{
    DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent, MouseButtonAction,
    MouseButtonEvent, MouseLeftViewportEvent, MouseMoveEvent, RenderingContext, ServoBuilder,
    WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

use crate::browser::Browser;
use crate::config::Config;
use crate::gui::{Action, Gui};
use crate::{hardening, input, pages};

/// Píxeles por "línea" de rueda del ratón.
const LINE_HEIGHT: f32 = 76.0;

/// Espera tras cargar la página antes de hacer la captura de `--screenshot`.
const SCREENSHOT_DELAY: Duration = Duration::from_secs(2);

pub struct App {
    state: State,
    cursor: PhysicalPosition<f64>,
    cursor_in_page: bool,
    /// Se pulsó un botón dentro de la página: la página recibe el ratón hasta
    /// soltarlo, aunque salga de ella (para seleccionar texto arrastrando).
    dragging_in_page: bool,
    modifiers: ModifiersState,
}

enum State {
    Starting { waker: Waker, config: Config },
    Running(Box<Running>),
    Finished,
}

struct Running {
    browser: Rc<Browser>,
    gui: Gui,
    screenshot: Option<Screenshot>,
}

struct Screenshot {
    path: PathBuf,
    /// Clic de prueba (`--click`) que se hace antes de la captura.
    click: Option<DevicePoint>,
    click_at: Option<Instant>,
    due: Option<Instant>,
}

impl App {
    pub fn new(event_loop: &EventLoop<WakeUp>, config: Config) -> Self {
        Self {
            state: State::Starting {
                waker: Waker(event_loop.create_proxy()),
                config,
            },
            cursor: PhysicalPosition::default(),
            cursor_in_page: false,
            dragging_in_page: false,
            modifiers: ModifiersState::empty(),
        }
    }

    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let State::Starting { waker, config } = std::mem::replace(&mut self.state, State::Finished)
        else {
            return Ok(());
        };

        let attributes = Window::default_attributes()
            .with_title("Ferrum")
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0));
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| format!("no se pudo crear la ventana: {e}"))?;

        let display_handle = event_loop
            .display_handle()
            .map_err(|e| format!("sin acceso a la pantalla: {e}"))?;
        let window_handle = window
            .window_handle()
            .map_err(|e| format!("sin acceso a la ventana: {e}"))?;
        let rendering_context = Rc::new(
            WindowRenderingContext::new(display_handle, window_handle, window.inner_size())
                .map_err(|e| format!("no se pudo iniciar OpenGL: {e:?}"))?,
        );
        rendering_context
            .make_current()
            .map_err(|e| format!("no se pudo activar OpenGL: {e:?}"))?;

        let mut gui = Gui::new(event_loop, &window, rendering_context.glow_gl_api())?;

        let servo = ServoBuilder::default()
            .opts(hardening::opts(&config))
            .preferences(hardening::preferences(&config))
            .event_loop_waker(Box::new(waker))
            .build();
        servo.setup_logging();

        let shield = build_shield(&config);
        let browser = Browser::new(
            window,
            servo,
            rendering_context,
            shield,
            config.search.clone(),
        );

        // La URL inicial se pasa al crear la pestaña: cargarla justo después
        // fallaría porque el contexto de navegación aún no existe.
        let initial_url = config
            .initial_input
            .as_deref()
            .and_then(|input| omnibox::resolve(input, &config.search));
        match initial_url {
            Some(url) => browser.open_tab(url),
            None => {
                browser.open_tab(pages::start_url());
                gui.focus_address();
            },
        }

        log::info!(
            "Ferrum {} iniciado (estricto: {}, multiproceso: {}, sandbox: {})",
            env!("CARGO_PKG_VERSION"),
            config.strict,
            config.multiprocess,
            config.multiprocess && hardening::sandbox_supported(),
        );
        self.state = State::Running(Box::new(Running {
            browser,
            gui,
            screenshot: config.screenshot.map(|path| Screenshot {
                path,
                click: config.click.map(|(x, y)| DevicePoint::new(x, y)),
                click_at: None,
                due: None,
            }),
        }));
        Ok(())
    }

    fn finish(&mut self, event_loop: &ActiveEventLoop) {
        if let State::Running(running) = &mut self.state {
            running.gui.destroy();
        }
        self.state = State::Finished;
        event_loop.exit();
    }

    fn page_event(browser: &Browser, event: InputEvent) {
        if let Some(webview) = browser.active_webview() {
            webview.notify_input_event(event);
        }
    }

    /// Reparte un evento de la ventana. Devuelve `true` si hay que cerrar.
    fn handle_window_event(&mut self, event: WindowEvent) -> bool {
        let State::Running(running) = &mut self.state else {
            return false;
        };
        let Running {
            browser,
            gui,
            screenshot,
        } = &mut **running;
        browser.servo().spin_event_loop();
        running_after_spin(browser, screenshot);

        if gui.on_window_event(browser.window(), &event) {
            browser.window().request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::RedrawRequested => {
                if redraw(browser, gui, screenshot) {
                    return true;
                }
            },
            WindowEvent::Resized(size) => browser.resize_window(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                browser.set_scale_factor(scale_factor)
            },
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    browser.note_user_gesture();
                }
                let consumed = handle_shortcut(browser, gui, &event, self.modifiers);
                // Si se está escribiendo en la barra de direcciones, la página no
                // recibe las teclas.
                if !consumed && !gui.wants_keyboard() {
                    Self::page_event(
                        browser,
                        InputEvent::Keyboard(input::keyboard_event(&event, self.modifiers)),
                    );
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let point = if self.dragging_in_page {
                    Some(gui.page_point_unchecked(position))
                } else {
                    gui.page_point(position)
                };
                match point {
                    Some(point) => {
                        if !self.cursor_in_page {
                            self.cursor_in_page = true;
                            browser.apply_page_cursor();
                        }
                        Self::page_event(
                            browser,
                            InputEvent::MouseMove(MouseMoveEvent::new(point.into())),
                        );
                    },
                    None if self.cursor_in_page => {
                        self.cursor_in_page = false;
                        // La interfaz usa su propio cursor; no heredar el de la web.
                        browser.window().set_cursor_visible(true);
                        browser.window().set_cursor(CursorIcon::Default);
                        Self::page_event(
                            browser,
                            InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()),
                        );
                    },
                    None => {},
                }
            },
            WindowEvent::CursorLeft { .. } => {
                if self.cursor_in_page {
                    self.cursor_in_page = false;
                    Self::page_event(
                        browser,
                        InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()),
                    );
                }
            },
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    browser.note_user_gesture();
                    match button {
                        MouseButton::Back => browser.go_back(),
                        MouseButton::Forward => browser.go_forward(),
                        _ => {},
                    }
                }
                let point: Option<DevicePoint> = match state {
                    ElementState::Pressed => gui.page_point(self.cursor),
                    ElementState::Released if self.dragging_in_page => {
                        Some(gui.page_point_unchecked(self.cursor))
                    },
                    ElementState::Released => gui.page_point(self.cursor),
                };
                if let Some(point) = point {
                    self.dragging_in_page = state == ElementState::Pressed;
                    if state == ElementState::Pressed {
                        // Clic en la página: la barra de direcciones suelta el foco.
                        gui.release_focus();
                    }
                    let action = match state {
                        ElementState::Pressed => MouseButtonAction::Down,
                        ElementState::Released => MouseButtonAction::Up,
                    };
                    Self::page_event(
                        browser,
                        InputEvent::MouseButton(MouseButtonEvent::new(
                            action,
                            input::mouse_button(button),
                            point.into(),
                        )),
                    );
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(point) = gui.page_point(self.cursor) {
                    let (x, y) = match delta {
                        MouseScrollDelta::LineDelta(dx, dy) => {
                            (f64::from(dx * LINE_HEIGHT), f64::from(dy * LINE_HEIGHT))
                        },
                        MouseScrollDelta::PixelDelta(delta) => (delta.x, delta.y),
                    };
                    Self::page_event(
                        browser,
                        InputEvent::Wheel(WheelEvent::new(
                            WheelDelta {
                                x,
                                y,
                                z: 0.0,
                                mode: WheelMode::DeltaPixel,
                            },
                            point.into(),
                        )),
                    );
                }
            },
            _ => {},
        }

        browser.exit_requested()
    }
}

impl ApplicationHandler<WakeUp> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.start(event_loop) {
            log::error!("{error}");
            eprintln!("Ferrum no pudo arrancar: {error}");
            event_loop.exit();
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, State::Running(running)) =
            (cause, &mut self.state)
        {
            running.gui.clear_repaint_deadline();
            running.browser.window().request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: WakeUp) {
        let State::Running(running) = &mut self.state else {
            return;
        };
        running.browser.servo().spin_event_loop();
        running_after_spin(&running.browser, &mut running.screenshot);
        if running.browser.exit_requested() {
            self.finish(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.handle_window_event(event) {
            self.finish(event_loop);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let State::Running(running) = &self.state else {
            return;
        };
        let deadline = [
            running.gui.repaint_deadline(),
            running.screenshot.as_ref().and_then(|shot| shot.due),
            running.screenshot.as_ref().and_then(|shot| shot.click_at),
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(match deadline {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }
}

/// Trabajo pendiente tras dejar que Servo procese sus mensajes.
fn running_after_spin(browser: &Browser, screenshot: &mut Option<Screenshot>) {
    browser.run_file_picker();
    let Some(shot) = screenshot.as_mut() else {
        return;
    };
    let now = Instant::now();
    if shot.due.is_none() && shot.click_at.is_none() && browser.active_page_loaded() {
        // Con --click, se deja un segundo para que la página se pinte antes.
        if shot.click.is_some() {
            shot.click_at = now.checked_add(Duration::from_secs(1));
        } else {
            shot.due = now.checked_add(SCREENSHOT_DELAY);
        }
    }
    if shot.click_at.is_some_and(|at| now >= at) {
        shot.click_at = None;
        if let Some(point) = shot.click.take() {
            App::page_event(
                browser,
                InputEvent::MouseMove(MouseMoveEvent::new(point.into())),
            );
            for action in [MouseButtonAction::Down, MouseButtonAction::Up] {
                App::page_event(
                    browser,
                    InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        servo::MouseButton::Primary,
                        point.into(),
                    )),
                );
            }
        }
        shot.due = now.checked_add(SCREENSHOT_DELAY);
    }
}

/// Compone un fotograma: la página (pintada por Servo) y la interfaz encima.
/// Devuelve `true` si hay que cerrar (tras una captura de `--screenshot`).
fn redraw(browser: &Browser, gui: &mut Gui, screenshot: &mut Option<Screenshot>) -> bool {
    browser.paint_page();
    let actions = gui.run(browser);

    let rendering_context = browser.rendering_context();
    rendering_context.prepare_for_rendering();
    gui.paint(browser.window());

    let capture_now = screenshot
        .as_ref()
        .and_then(|shot| shot.due)
        .is_some_and(|due| Instant::now() >= due);
    if capture_now && let Some(shot) = screenshot.take() {
        save_screenshot(rendering_context, browser.window(), &shot.path);
    }
    rendering_context.present();

    if let Some(size) = gui.viewport_size() {
        browser.resize_viewport(size);
    }
    let had_actions = !actions.is_empty();
    for action in actions {
        apply_action(browser, gui, action);
    }
    if had_actions {
        browser.window().request_redraw();
    }
    capture_now
}

fn apply_action(browser: &Browser, gui: &mut Gui, action: Action) {
    match action {
        Action::Activate(index) => browser.activate(index),
        Action::Close(index) => browser.close_tab(index),
        Action::NewTab => {
            browser.open_tab(pages::start_url());
            gui.focus_address();
        },
        Action::Back => browser.go_back(),
        Action::Forward => browser.go_forward(),
        Action::Reload => browser.reload(),
        Action::Home => browser.home(),
        Action::Navigate(text) => browser.navigate(&text),
        Action::DismissMessage => browser.dismiss_message(),
    }
}

fn save_screenshot(rendering_context: &WindowRenderingContext, window: &Window, path: &PathBuf) {
    let size = window.inner_size();
    let rect = DeviceIntRect::from_size(DeviceIntSize::new(size.width as i32, size.height as i32));
    match rendering_context.read_to_image(rect) {
        Some(image) => match image.save(path) {
            Ok(()) => log::info!("Captura guardada en {}", path.display()),
            Err(error) => eprintln!("No se pudo guardar la captura: {error}"),
        },
        None => eprintln!("No se pudo leer la imagen de la ventana"),
    }
}

/// Atajos del navegador. Devuelve `true` si la tecla fue consumida.
///
/// Las teclas de edición (Ctrl+C/V/X/A/Z) no son atajos: llegan a la barra de
/// direcciones o a la página según quién tenga el foco.
fn handle_shortcut(
    browser: &Browser,
    gui: &mut Gui,
    event: &KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    if event.state != ElementState::Pressed {
        return false;
    }
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();
    let shift = modifiers.shift_key();

    match &event.logical_key {
        Key::Character(c) if ctrl && !alt => match c.to_lowercase().as_str() {
            "l" => gui.focus_address(),
            "t" => {
                browser.open_tab(pages::start_url());
                gui.focus_address();
            },
            "w" => browser.close_active_tab(),
            "r" => browser.reload(),
            "+" | "=" => browser.zoom(Some(1.1)),
            "-" => browser.zoom(Some(1.0 / 1.1)),
            "0" => browser.zoom(None),
            _ => return false,
        },
        Key::Character(c) if alt && !ctrl && c.eq_ignore_ascii_case("d") => gui.focus_address(),
        Key::Named(NamedKey::F6) => gui.focus_address(),
        Key::Named(NamedKey::F5) => browser.reload(),
        Key::Named(NamedKey::Tab) if ctrl => browser.cycle_tab(!shift),
        Key::Named(NamedKey::ArrowLeft) if alt => browser.go_back(),
        Key::Named(NamedKey::ArrowRight) if alt => browser.go_forward(),
        Key::Named(NamedKey::BrowserBack) => browser.go_back(),
        Key::Named(NamedKey::BrowserForward) => browser.go_forward(),
        _ => return false,
    }
    browser.window().request_redraw();
    true
}

fn build_shield(config: &Config) -> Shield {
    let mut shield = Shield::new(ShieldConfig::default());
    let lists = config
        .blocklists
        .iter()
        .map(|path| (path, Category::Tracker))
        .chain(
            config
                .malware_lists
                .iter()
                .map(|path| (path, Category::Malware)),
        );
    for (path, category) in lists {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let added = shield.blocklist_mut().load_list(&text, category);
                log::info!("{added} reglas cargadas de {}", path.display());
            },
            Err(error) => {
                log::warn!("No se pudo leer la lista {}: {error}", path.display());
                eprintln!("Aviso: no se pudo leer {}: {error}", path.display());
            },
        }
    }
    shield
}

/// Evento con el que Servo despierta el bucle de la ventana.
#[derive(Debug)]
pub struct WakeUp;

#[derive(Clone)]
struct Waker(EventLoopProxy<WakeUp>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        // Si el bucle ya terminó, no hay nada que despertar.
        let _ = self.0.send_event(WakeUp);
    }
}
