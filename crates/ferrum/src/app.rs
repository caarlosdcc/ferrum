/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Bucle de eventos de la ventana (winit) y atajos de teclado.

use std::rc::Rc;

use ferrum_shield::{Category, Shield, ShieldConfig};
use servo::DevicePoint;
use servo::{
    EventLoopWaker, InputEvent, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, RenderingContext, ServoBuilder, WheelDelta, WheelEvent, WheelMode,
    WindowRenderingContext,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowId};

use crate::browser::Browser;
use crate::config::Config;
use crate::{hardening, input, pages};

/// Píxeles por "línea" de rueda del ratón.
const LINE_HEIGHT: f32 = 76.0;

pub struct App {
    state: State,
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
}

enum State {
    Starting { waker: Waker, config: Config },
    Running(Rc<Browser>),
    Finished,
}

impl App {
    pub fn new(event_loop: &EventLoop<WakeUp>, config: Config) -> Self {
        Self {
            state: State::Starting {
                waker: Waker(event_loop.create_proxy()),
                config,
            },
            cursor: PhysicalPosition::default(),
            modifiers: ModifiersState::empty(),
        }
    }

    fn browser(&self) -> Option<Rc<Browser>> {
        match &self.state {
            State::Running(browser) => Some(browser.clone()),
            _ => None,
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
            .with_min_inner_size(LogicalSize::new(400.0, 300.0));
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

        match config.initial_input.as_deref() {
            Some(input) => {
                browser.open_tab(pages::start_url());
                browser.navigate(input);
            },
            None => {
                browser.open_tab(pages::start_url());
                browser.begin_address_edit();
            },
        }

        log::info!(
            "Ferrum {} iniciado (estricto: {}, multiproceso: {}, sandbox: {})",
            env!("CARGO_PKG_VERSION"),
            config.strict,
            config.multiprocess,
            config.multiprocess && hardening::sandbox_supported(),
        );
        self.state = State::Running(browser);
        Ok(())
    }

    /// Atajos del navegador. Devuelve `true` si la tecla fue consumida.
    fn handle_shortcut(&self, browser: &Browser, event: &KeyEvent) -> bool {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();

        if browser.is_editing_address() {
            if event.state == ElementState::Pressed {
                match &event.logical_key {
                    Key::Named(NamedKey::Enter) => browser.commit_address(),
                    Key::Named(NamedKey::Escape) => browser.cancel_address(),
                    Key::Named(NamedKey::Backspace) => browser.address_backspace(),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => {
                        browser.address_paste()
                    },
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => {
                        browser.address_select_all()
                    },
                    _ if !ctrl && !alt => {
                        if let Some(text) = &event.text {
                            browser.address_type(text);
                        }
                    },
                    _ => {},
                }
            }
            // Mientras se escribe la dirección, la página no recibe teclas.
            return true;
        }

        if event.state != ElementState::Pressed {
            return false;
        }

        match &event.logical_key {
            Key::Character(c) if ctrl && !alt => match c.to_lowercase().as_str() {
                "l" => browser.begin_address_edit(),
                "t" => {
                    browser.open_tab(pages::start_url());
                    browser.begin_address_edit();
                },
                "w" => browser.close_active_tab(),
                "r" => browser.reload(),
                "+" | "=" => browser.zoom(Some(1.1)),
                "-" => browser.zoom(Some(1.0 / 1.1)),
                "0" => browser.zoom(None),
                _ => return false,
            },
            Key::Character(c) if alt && !ctrl && c.eq_ignore_ascii_case("d") => {
                browser.begin_address_edit()
            },
            Key::Named(NamedKey::F6) => browser.begin_address_edit(),
            Key::Named(NamedKey::F5) => browser.reload(),
            Key::Named(NamedKey::Tab) if ctrl => browser.cycle_tab(!shift),
            Key::Named(NamedKey::ArrowLeft) if alt => browser.go_back(),
            Key::Named(NamedKey::ArrowRight) if alt => browser.go_forward(),
            Key::Named(NamedKey::BrowserBack) => browser.go_back(),
            Key::Named(NamedKey::BrowserForward) => browser.go_forward(),
            _ => return false,
        }
        true
    }

    fn cursor_point(&self) -> DevicePoint {
        DevicePoint::new(self.cursor.x as f32, self.cursor.y as f32)
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: WakeUp) {
        if let Some(browser) = self.browser() {
            browser.servo().spin_event_loop();
            if browser.exit_requested() {
                self.state = State::Finished;
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(browser) = self.browser() else {
            return;
        };
        browser.servo().spin_event_loop();

        match event {
            WindowEvent::CloseRequested => {
                self.state = State::Finished;
                event_loop.exit();
                return;
            },
            WindowEvent::RedrawRequested => browser.paint(),
            WindowEvent::Resized(size) => browser.resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                browser.set_scale_factor(scale_factor)
            },
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    browser.note_user_gesture();
                }
                if !self.handle_shortcut(&browser, &event)
                    && let Some(webview) = browser.active_webview()
                {
                    webview.notify_input_event(InputEvent::Keyboard(input::keyboard_event(
                        &event,
                        self.modifiers,
                    )));
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if let Some(webview) = browser.active_webview() {
                    webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                        self.cursor_point().into(),
                    )));
                }
            },
            WindowEvent::CursorLeft { .. } => {
                if let Some(webview) = browser.active_webview() {
                    webview.notify_input_event(InputEvent::MouseLeftViewport(
                        MouseLeftViewportEvent::default(),
                    ));
                }
            },
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    browser.note_user_gesture();
                    if browser.is_editing_address() {
                        browser.cancel_address();
                    }
                    match button {
                        MouseButton::Back => return browser.go_back(),
                        MouseButton::Forward => return browser.go_forward(),
                        _ => {},
                    }
                }
                if let Some(webview) = browser.active_webview() {
                    let action = match state {
                        ElementState::Pressed => MouseButtonAction::Down,
                        ElementState::Released => MouseButtonAction::Up,
                    };
                    webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        input::mouse_button(button),
                        self.cursor_point().into(),
                    )));
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(webview) = browser.active_webview() {
                    let (x, y, mode) = match delta {
                        MouseScrollDelta::LineDelta(dx, dy) => (
                            f64::from(dx * LINE_HEIGHT),
                            f64::from(dy * LINE_HEIGHT),
                            WheelMode::DeltaPixel,
                        ),
                        MouseScrollDelta::PixelDelta(delta) => {
                            (delta.x, delta.y, WheelMode::DeltaPixel)
                        },
                    };
                    webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                        WheelDelta { x, y, z: 0.0, mode },
                        self.cursor_point().into(),
                    )));
                }
            },
            _ => {},
        }

        if browser.exit_requested() {
            self.state = State::Finished;
            event_loop.exit();
        }
    }
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
