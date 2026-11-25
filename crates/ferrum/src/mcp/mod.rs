/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Servidor MCP (Model Context Protocol): deja que un agente de IA, como Claude
//! Code, navegue con Ferrum.
//!
//! Son dos procesos. `ferrum --mcp` ([`relay`]) habla con el agente y abre el
//! navegador cuando hace falta; el navegador (`--mcp-browser`, este módulo)
//! ejecuta las herramientas.
//!
//! - Solo se habla por la entrada y la salida estándar, nunca por la red: lo
//!   controla únicamente el programa que abrió Ferrum.
//! - La barra muestra la pastilla «Agente» mientras un agente tiene el control.
//! - Nada persiste: cada sesión empieza sin cookies ni historial.

mod protocol;
pub mod relay;
mod scripts;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use base64::Engine;
use keyboard_types::{Code, Key, KeyState, Location, Modifiers, NamedKey};
use serde_json::{Value, json};
use servo::{
    DevicePoint, InputEvent, JSValue, JavaScriptEvaluationError, MouseButtonAction,
    MouseButtonEvent, MouseMoveEvent, RgbaImage, WebView, WheelDelta, WheelEvent, WheelMode,
};

use self::protocol::{Call, Message, Output};
use crate::browser::{Browser, DialogKind};
use crate::pages;

/// Cuánto se espera a que cargue una página.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Si no se ve empezar ninguna carga (un ancla en la misma página, por
/// ejemplo), se da por terminada pasado este tiempo.
const LOAD_FALLBACK: Duration = Duration::from_secs(5);
/// Margen tras cargar para que la página se pinte.
const LOAD_SETTLE: Duration = Duration::from_millis(300);
/// Tras un clic o una tecla, cuánto se espera a ver si empieza otra página.
const ACTION_SETTLE: Duration = Duration::from_millis(400);
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// La primera pestaña necesita un momento antes de poder navegar.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TEXT_CHARS: u64 = 6000;

pub struct Server {
    incoming: Receiver<Incoming>,
    /// Llamadas que esperan a que el navegador esté listo.
    queued: VecDeque<Call>,
    tasks: Vec<Task>,
    ready: bool,
    started: Option<Instant>,
    closed: bool,
}

enum Incoming {
    Line(String),
    Closed,
}

struct Task {
    id: Value,
    wait: Wait,
}

type ScriptSlot = Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>>;

/// Lo que espera una herramienta antes de responder.
enum Wait {
    /// Que la pestaña activa termine de cargar.
    Load {
        started: Instant,
        /// Cargas empezadas antes de la acción (ver [`Browser::load_generation`]).
        generation: u64,
        done_at: Option<Instant>,
        prefix: String,
    },
    /// Un momento tras un clic o una tecla; si empezó otra página, se espera.
    Settle {
        until: Instant,
        generation: u64,
        prefix: String,
    },
    /// El resultado de un script.
    Script {
        slot: ScriptSlot,
        started: Instant,
        then: Then,
    },
    /// Un clic diferido, para dar tiempo a que la página se desplace.
    ClickAt {
        at: Instant,
        point: DevicePoint,
        prefix: String,
    },
    /// Un instante concreto.
    Until { at: Instant, reply: String },
    /// La próxima imagen de la ventana.
    Capture { started: Instant, include_ui: bool },
}

/// Qué hacer con el resultado de un script.
enum Then {
    Evaluate,
    ReadPage,
    Find,
    SelectOption,
}

enum Step {
    Done(Output),
    Wait(Wait),
}

impl Server {
    /// Empieza a leer mensajes de la entrada estándar. `wake` despierta el
    /// bucle de la ventana cuando llega uno.
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, incoming) = mpsc::channel();
        let reader = move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if sender.send(Incoming::Line(line)).is_err() {
                    return;
                }
                wake();
            }
            let _ = sender.send(Incoming::Closed);
            wake();
        };
        if let Err(error) = std::thread::Builder::new()
            .name("ferrum-mcp".to_owned())
            .spawn(reader)
        {
            log::error!("No se pudo leer la entrada del agente: {error}");
        }
        Server {
            incoming,
            queued: VecDeque::new(),
            tasks: Vec::new(),
            ready: false,
            started: None,
            closed: false,
        }
    }

    /// El agente cerró la conexión: Ferrum debe terminar.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Atiende lo que haya llegado y avanza lo pendiente.
    pub fn poll(&mut self, browser: &Browser) {
        while let Ok(incoming) = self.incoming.try_recv() {
            match incoming {
                Incoming::Closed => self.closed = true,
                Incoming::Line(line) => match protocol::parse(&line) {
                    Message::Reply(reply) => send(&reply),
                    Message::Call(call) => self.queued.push_back(call),
                    Message::Cancel(id) => {
                        self.queued.retain(|call| call.id != id);
                        self.tasks.retain(|task| task.id != id);
                    },
                    Message::Ignore => {},
                },
            }
        }
        let now = Instant::now();
        if !self.ready {
            let started = *self.started.get_or_insert(now);
            self.ready =
                browser.active_page_loaded() || now.duration_since(started) > READY_TIMEOUT;
        }
        if self.ready {
            while let Some(call) = self.queued.pop_front() {
                let step = start_tool(browser, &call.name, &call.arguments);
                self.advance(browser, call.id, step);
            }
        }
        for task in std::mem::take(&mut self.tasks) {
            let step = poll_wait(browser, task.wait, now);
            self.advance(browser, task.id, step);
        }
        browser.set_agent(Some(!self.tasks.is_empty() || !self.queued.is_empty()));
    }

    fn advance(&mut self, browser: &Browser, id: Value, step: Step) {
        match step {
            Step::Done(output) => send(&protocol::success(id, output.into_result())),
            Step::Wait(wait) => {
                if matches!(wait, Wait::Capture { .. }) {
                    browser.window().request_redraw();
                }
                self.tasks.push(Task { id, wait });
            },
        }
    }

    /// Cuándo hay que volver a mirar las tareas aunque no pase nada.
    pub fn deadline(&self) -> Option<Instant> {
        let ready = (!self.ready)
            .then(|| self.started.map(|started| started + READY_TIMEOUT))
            .flatten();
        self.tasks
            .iter()
            .map(|task| match &task.wait {
                Wait::Load {
                    started, done_at, ..
                } => done_at.unwrap_or(*started + LOAD_FALLBACK),
                Wait::Settle { until, .. } => *until,
                Wait::Script { started, .. } => *started + SCRIPT_TIMEOUT,
                Wait::ClickAt { at, .. } | Wait::Until { at, .. } => *at,
                Wait::Capture { started, .. } => *started + CAPTURE_TIMEOUT,
            })
            .chain(ready)
            .min()
    }

    /// Hay una captura pendiente: el próximo fotograma debe leerse.
    pub fn wants_capture(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| matches!(task.wait, Wait::Capture { .. }))
    }

    /// Entrega la imagen de la ventana recién pintada a las capturas pendientes.
    /// `page` es la zona de la página en píxeles: (x, y, ancho, alto).
    pub fn deliver_capture(&mut self, window: &RgbaImage, page: Option<[u32; 4]>) {
        let (captures, rest): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.tasks)
            .into_iter()
            .partition(|task| matches!(task.wait, Wait::Capture { .. }));
        self.tasks = rest;
        for task in captures {
            let Wait::Capture { include_ui, .. } = task.wait else {
                continue;
            };
            let output = match (include_ui, page) {
                (false, Some([x, y, width, height])) => {
                    let cropped = image::imageops::crop_imm(window, x, y, width, height).to_image();
                    encode(
                        &cropped,
                        "Página activa (las coordenadas de la imagen valen para click).",
                    )
                },
                _ => encode(window, "Ventana completa, con la interfaz de Ferrum."),
            };
            send(&protocol::success(task.id, output.into_result()));
        }
    }

    /// Ferrum se cierra: responde a lo pendiente para que el agente no espere.
    pub fn shutdown(&mut self) {
        let pending = self
            .tasks
            .drain(..)
            .map(|task| task.id)
            .chain(self.queued.drain(..).map(|call| call.id));
        for id in pending {
            let output = Output::Error("La ventana de Ferrum se cerró.".to_owned());
            send(&protocol::success(id, output.into_result()));
        }
    }
}

/// Escribe un mensaje en la salida estándar, en una sola línea.
fn send(message: &Value) {
    let mut stdout = std::io::stdout().lock();
    let result = writeln!(stdout, "{message}").and_then(|()| stdout.flush());
    if let Err(error) = result {
        log::warn!("No se pudo responder al agente: {error}");
    }
}

fn encode(image: &RgbaImage, caption: &str) -> Output {
    let mut png = Vec::new();
    match image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
        Ok(()) => Output::Image {
            png_base64: base64::engine::general_purpose::STANDARD.encode(png),
            caption: format!("{caption} {}×{} px.", image.width(), image.height()),
        },
        Err(error) => Output::Error(format!("No se pudo codificar la captura: {error}")),
    }
}

// ----------------------------------------------------------------------
// Herramientas
// ----------------------------------------------------------------------

fn start_tool(browser: &Browser, name: &str, args: &Value) -> Step {
    match tool(browser, name, args) {
        Ok(step) => step,
        Err(message) => Step::Done(Output::Error(message)),
    }
}

fn tool(browser: &Browser, name: &str, args: &Value) -> Result<Step, String> {
    let now = Instant::now();
    Ok(match name {
        "navigate" => {
            let input = string(args, "url")?;
            if input.trim().is_empty() {
                return Err("La dirección está vacía.".to_owned());
            }
            let generation = browser.load_generation();
            browser.navigate(input);
            load(now, generation, String::new())
        },
        "history" => {
            let webview = active(browser)?;
            let generation = browser.load_generation();
            match string(args, "action")? {
                "back" if webview.can_go_back() => browser.go_back(),
                "forward" if webview.can_go_forward() => browser.go_forward(),
                "reload" => browser.reload(),
                "back" => return Err("No hay ninguna página anterior.".to_owned()),
                "forward" => return Err("No hay ninguna página siguiente.".to_owned()),
                other => return Err(format!("Acción desconocida: {other}")),
            }
            load(now, generation, String::new())
        },
        "read_page" => {
            let max = args
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_TEXT_CHARS)
                .clamp(200, 50_000);
            run_script(
                browser,
                scripts::call(scripts::READ_PAGE, &[json!(max)]),
                Then::ReadPage,
            )?
        },
        "screenshot" => Step::Wait(Wait::Capture {
            started: now,
            include_ui: args
                .get("include_ui")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        "click" => {
            let x = args.get("x").and_then(Value::as_f64);
            let y = args.get("y").and_then(Value::as_f64);
            let selector = args.get("selector").and_then(Value::as_str);
            let text = args.get("text").and_then(Value::as_str);
            match (x, y, selector, text) {
                (Some(x), Some(y), ..) => {
                    let point = page_point(browser, x, y)?;
                    let generation = browser.load_generation();
                    click(browser, point)?;
                    settle(now, generation, format!("Clic en ({x:.0}, {y:.0})."))
                },
                (_, _, Some(selector), _) => run_script(
                    browser,
                    scripts::call(scripts::FIND, &[json!(selector), Value::Null]),
                    Then::Find,
                )?,
                (_, _, None, Some(text)) => run_script(
                    browser,
                    scripts::call(scripts::FIND, &[Value::Null, json!(text)]),
                    Then::Find,
                )?,
                _ => return Err("Indica x e y, un selector o un texto.".to_owned()),
            }
        },
        "type_text" => {
            let text = string(args, "text")?;
            let webview = active(browser)?;
            let generation = browser.load_generation();
            browser.note_user_gesture();
            for character in text.chars() {
                let key = match character {
                    '\n' => Key::Named(NamedKey::Enter),
                    '\t' => Key::Named(NamedKey::Tab),
                    other => Key::Character(other.to_string()),
                };
                press(&webview, key, Modifiers::empty());
            }
            let count = text.chars().count();
            if args.get("submit").and_then(Value::as_bool).unwrap_or(false) {
                press(&webview, Key::Named(NamedKey::Enter), Modifiers::empty());
                settle(
                    now,
                    generation,
                    format!("Escritos {count} caracteres y pulsado Intro."),
                )
            } else {
                Step::Done(Output::Text(format!("Escritos {count} caracteres.")))
            }
        },
        "press_key" => {
            let name = string(args, "key")?;
            let key = parse_key(name).ok_or_else(|| format!("Tecla desconocida: {name}"))?;
            let modifiers = parse_modifiers(args.get("modifiers"))?;
            let webview = active(browser)?;
            let generation = browser.load_generation();
            browser.note_user_gesture();
            press(&webview, key, modifiers);
            settle(now, generation, format!("Pulsada la tecla {name}."))
        },
        "scroll" => {
            let dy = number(args, "dy")?;
            let dx = args.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
            let size = browser.viewport_size();
            let x = args
                .get("x")
                .and_then(Value::as_f64)
                .unwrap_or(f64::from(size.width) / 2.0);
            let y = args
                .get("y")
                .and_then(Value::as_f64)
                .unwrap_or(f64::from(size.height) / 2.0);
            let point = page_point(browser, x, y)?;
            // La rueda de Servo usa el signo de winit: positivo es hacia arriba.
            let delta = WheelDelta {
                x: -dx,
                y: -dy,
                z: 0.0,
                mode: WheelMode::DeltaPixel,
            };
            let webview = active(browser)?;
            webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point.into())));
            webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(delta, point.into())));
            Step::Wait(Wait::Until {
                at: now + Duration::from_millis(250),
                reply: format!("Desplazado {dy:.0} px en vertical y {dx:.0} px en horizontal."),
            })
        },
        "select_option" => {
            let selector = string(args, "selector")?;
            let option = string(args, "option")?;
            run_script(
                browser,
                scripts::call(scripts::SELECT_OPTION, &[json!(selector), json!(option)]),
                Then::SelectOption,
            )?
        },
        "evaluate" => run_script(browser, string(args, "script")?.to_owned(), Then::Evaluate)?,
        "tabs" => tabs(browser, args, now)?,
        "dialog" => {
            let accept = args
                .get("accept")
                .and_then(Value::as_bool)
                .ok_or("Falta accept.")?;
            let text = args.get("text").and_then(Value::as_str);
            let generation = browser.load_generation();
            if !browser.answer_active_dialog(accept, text) {
                return Err("La pestaña activa no tiene ningún aviso abierto.".to_owned());
            }
            let reply = if accept {
                "Aviso aceptado."
            } else {
                "Aviso cancelado."
            };
            settle(now, generation, reply.to_owned())
        },
        "wait" => {
            let ms = args
                .get("ms")
                .and_then(Value::as_u64)
                .ok_or("Falta ms.")?
                .min(10_000);
            Step::Wait(Wait::Until {
                at: now + Duration::from_millis(ms),
                reply: format!("Esperé {ms} ms."),
            })
        },
        other => return Err(format!("Herramienta desconocida: {other}")),
    })
}

fn tabs(browser: &Browser, args: &Value, now: Instant) -> Result<Step, String> {
    let index = || {
        let count = browser.tab_infos().len();
        let index = args
            .get("index")
            .and_then(Value::as_u64)
            .ok_or("Falta index (empieza en 1).")?;
        match usize::try_from(index) {
            Ok(index) if (1..=count).contains(&index) => Ok(index - 1),
            _ => Err(format!("No hay pestaña {index}: hay {count}.")),
        }
    };
    Ok(match string(args, "action")? {
        "list" => Step::Done(Output::Text(tab_list(browser))),
        "new" => {
            let generation = browser.load_generation();
            let input = args.get("url").and_then(Value::as_str).unwrap_or_default();
            if !browser.open_tab_for(input) {
                browser.open_tab(pages::start_url());
            }
            load(now, generation, "Pestaña nueva.\n".to_owned())
        },
        "select" => {
            browser.activate(index()?);
            Step::Done(Output::Text(page_summary(browser)))
        },
        "close" => {
            let index = index()?;
            if browser.tab_infos().len() == 1 {
                return Err("Es la última pestaña: cerrarla cerraría Ferrum.".to_owned());
            }
            browser.close_tab(index);
            Step::Done(Output::Text(tab_list(browser)))
        },
        other => return Err(format!("Acción desconocida: {other}")),
    })
}

fn load(now: Instant, generation: u64, prefix: String) -> Step {
    Step::Wait(Wait::Load {
        started: now,
        generation,
        done_at: None,
        prefix,
    })
}

fn settle(now: Instant, generation: u64, prefix: String) -> Step {
    Step::Wait(Wait::Settle {
        until: now + ACTION_SETTLE,
        generation,
        prefix,
    })
}

fn run_script(browser: &Browser, script: String, then: Then) -> Result<Step, String> {
    // Con un aviso abierto, la página está en pausa: el script no respondería.
    if let Some((_, origin, message)) = browser.active_dialog() {
        return Err(format!(
            "La página espera respuesta a un aviso de {origin}: «{message}». Respóndelo con la \
             herramienta dialog."
        ));
    }
    let webview = active(browser)?;
    let slot: ScriptSlot = Rc::default();
    let filled = slot.clone();
    webview.evaluate_javascript(script, move |result| {
        *filled.borrow_mut() = Some(result);
    });
    Ok(Step::Wait(Wait::Script {
        slot,
        started: Instant::now(),
        then,
    }))
}

fn poll_wait(browser: &Browser, wait: Wait, now: Instant) -> Step {
    match wait {
        Wait::Load {
            started,
            generation,
            mut done_at,
            prefix,
        } => {
            let loaded = browser.active_page_loaded();
            let fresh = browser.load_generation() != generation;
            let elapsed = now.duration_since(started);
            if loaded && (fresh || elapsed >= LOAD_FALLBACK) {
                let at = *done_at.get_or_insert(now + LOAD_SETTLE);
                if now >= at {
                    return Step::Done(Output::Text(format!("{prefix}{}", page_summary(browser))));
                }
            } else {
                // Si vuelve a cargar (una redirección), el margen empieza de nuevo.
                done_at = None;
            }
            if elapsed >= LOAD_TIMEOUT {
                return Step::Done(Output::Text(format!(
                    "{prefix}La página sigue cargando tras {} s.\n{}",
                    LOAD_TIMEOUT.as_secs(),
                    page_summary(browser)
                )));
            }
            Step::Wait(Wait::Load {
                started,
                generation,
                done_at,
                prefix,
            })
        },
        Wait::Settle {
            until,
            generation,
            prefix,
        } if now >= until => {
            let navigating = browser.load_generation() != generation;
            if navigating && !browser.active_page_loaded() {
                load(now, generation, format!("{prefix}\n"))
            } else {
                Step::Done(Output::Text(format!("{prefix}\n{}", page_summary(browser))))
            }
        },
        Wait::Script {
            slot,
            started,
            then,
        } => {
            let result = slot.borrow_mut().take();
            match result {
                Some(result) => script_done(browser, result, then, now),
                None if now.duration_since(started) >= SCRIPT_TIMEOUT => Step::Done(Output::Error(
                    format!("El script no respondió en {} s.", SCRIPT_TIMEOUT.as_secs()),
                )),
                None => Step::Wait(Wait::Script {
                    slot,
                    started,
                    then,
                }),
            }
        },
        Wait::ClickAt { at, point, prefix } if now >= at => {
            let generation = browser.load_generation();
            match click(browser, point) {
                Ok(()) => settle(now, generation, prefix),
                Err(message) => Step::Done(Output::Error(message)),
            }
        },
        Wait::Until { at, reply } if now >= at => Step::Done(Output::Text(reply)),
        Wait::Capture { started, .. } if now.duration_since(started) >= CAPTURE_TIMEOUT => {
            Step::Done(Output::Error(
                "No llegó ninguna imagen de la ventana. ¿Está minimizada?".to_owned(),
            ))
        },
        other => Step::Wait(other),
    }
}

fn script_done(
    browser: &Browser,
    result: Result<JSValue, JavaScriptEvaluationError>,
    then: Then,
    now: Instant,
) -> Step {
    let value = match result {
        Ok(value) => value,
        Err(error) => return Step::Done(Output::Error(script_error(&error))),
    };
    let data: Value = match (&then, value) {
        (Then::Evaluate, value) => return Step::Done(Output::Text(show_js(value))),
        (_, JSValue::String(text)) => serde_json::from_str(&text).unwrap_or(Value::Null),
        _ => {
            return Step::Done(Output::Error(
                "El script de Ferrum no devolvió texto.".to_owned(),
            ));
        },
    };
    match then {
        Then::Evaluate => Step::Done(Output::Text(data.to_string())),
        Then::ReadPage => Step::Done(Output::Text(format_page(&data))),
        Then::SelectOption => Step::Done(match data.get("chosen").and_then(Value::as_str) {
            Some(chosen) => Output::Text(format!("Elegida la opción «{chosen}».")),
            None => {
                let error = data["error"].as_str().unwrap_or("No se pudo elegir.");
                let options = data["options"]
                    .as_array()
                    .map(|options| {
                        let names: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
                        format!(" Opciones: {}.", names.join(" · "))
                    })
                    .unwrap_or_default();
                Output::Error(format!("{error}.{options}"))
            },
        }),
        Then::Find => {
            if data["found"] != true {
                return Step::Done(Output::Error(
                    "No encontré ningún elemento visible así. Usa read_page para ver los que hay."
                        .to_owned(),
                ));
            }
            let tag = data["tag"].as_str().unwrap_or("elemento");
            if data["select"] == true {
                return Step::Done(Output::Error(
                    "Es una lista desplegable: usa select_option con un selector.".to_owned(),
                ));
            }
            let label = data["label"].as_str().unwrap_or_default();
            let x = data["x"].as_f64().unwrap_or(-1.0);
            let y = data["y"].as_f64().unwrap_or(-1.0);
            let point = match page_point(browser, x, y) {
                Ok(point) => point,
                Err(message) => return Step::Done(Output::Error(message)),
            };
            let prefix = if label.is_empty() {
                format!("Clic en <{tag}> en ({x:.0}, {y:.0}).")
            } else {
                format!("Clic en <{tag}> «{label}» en ({x:.0}, {y:.0}).")
            };
            // Si hubo que desplazar la página, se espera a que se pinte.
            let delay = if data["scrolled"] == true { 150 } else { 30 };
            Step::Wait(Wait::ClickAt {
                at: now + Duration::from_millis(delay),
                point,
                prefix,
            })
        },
    }
}

// ----------------------------------------------------------------------
// Entrada simulada
// ----------------------------------------------------------------------

fn active(browser: &Browser) -> Result<WebView, String> {
    browser
        .active_webview()
        .ok_or_else(|| "No hay ninguna pestaña abierta.".to_owned())
}

/// Comprueba que un punto cae dentro de la página.
fn page_point(browser: &Browser, x: f64, y: f64) -> Result<DevicePoint, String> {
    let size = browser.viewport_size();
    let inside = x.is_finite()
        && y.is_finite()
        && x >= 0.0
        && y >= 0.0
        && x < f64::from(size.width)
        && y < f64::from(size.height);
    if !inside {
        return Err(format!(
            "El punto ({x:.0}, {y:.0}) está fuera de la página, que mide {}×{} px. Desplázala \
             con scroll si el elemento no se ve.",
            size.width, size.height
        ));
    }
    Ok(DevicePoint::new(x as f32, y as f32))
}

/// Un clic como el de un usuario: cuenta como gesto (para abrir ventanas).
fn click(browser: &Browser, point: DevicePoint) -> Result<(), String> {
    let webview = active(browser)?;
    browser.note_user_gesture();
    webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point.into())));
    for action in [MouseButtonAction::Down, MouseButtonAction::Up] {
        webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
            action,
            servo::MouseButton::Primary,
            point.into(),
        )));
    }
    Ok(())
}

fn press(webview: &WebView, key: Key, modifiers: Modifiers) {
    for state in [KeyState::Down, KeyState::Up] {
        let event = keyboard_types::KeyboardEvent {
            state,
            key: key.clone(),
            code: Code::Unidentified,
            location: Location::Standard,
            modifiers,
            repeat: false,
            is_composing: false,
        };
        webview.notify_input_event(InputEvent::Keyboard(servo::KeyboardEvent::new(event)));
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(character), None) = (chars.next(), chars.next()) {
        return Some(Key::Character(character.to_string()));
    }
    let canonical = match name.to_ascii_lowercase().as_str() {
        "space" | "espacio" => return Some(Key::Character(" ".to_owned())),
        "enter" | "return" | "intro" => "Enter",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" | "del" | "supr" => "Delete",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        _ => name,
    };
    match canonical.parse::<NamedKey>() {
        Ok(NamedKey::Unidentified) | Err(_) => None,
        Ok(named) => Some(Key::Named(named)),
    }
}

fn parse_modifiers(value: Option<&Value>) -> Result<Modifiers, String> {
    let mut modifiers = Modifiers::empty();
    let Some(list) = value.and_then(Value::as_array) else {
        return Ok(modifiers);
    };
    for name in list {
        modifiers |= match name.as_str().unwrap_or_default() {
            "Control" | "Ctrl" => Modifiers::CONTROL,
            "Shift" => Modifiers::SHIFT,
            "Alt" => Modifiers::ALT,
            "Meta" => Modifiers::META,
            other => return Err(format!("Modificador desconocido: {other}")),
        };
    }
    Ok(modifiers)
}

// ----------------------------------------------------------------------
// Texto de las respuestas
// ----------------------------------------------------------------------

fn string<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Falta {name}."))
}

fn number(args: &Value, name: &str) -> Result<f64, String> {
    args.get(name)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("Falta {name}."))
}

fn page_summary(browser: &Browser) -> String {
    let tabs = browser.tab_infos();
    let active = browser.active_index();
    let Some(tab) = tabs.get(active) else {
        return "No hay ninguna pestaña abierta.".to_owned();
    };
    let url = tab
        .url
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut summary = format!(
        "Título: {}\nDirección: {url}\nPestaña {} de {} · {} rastreadores bloqueados en la sesión",
        tab.title,
        active + 1,
        tabs.len(),
        browser.blocked_count()
    );
    if let Some((kind, origin, message)) = browser.active_dialog() {
        let kind = match kind {
            DialogKind::Alert => "Aviso",
            DialogKind::Confirm => "Confirmación",
            DialogKind::Prompt => "Pregunta con respuesta escrita",
        };
        summary.push_str(&format!(
            "\n{kind} de {origin}: «{message}». La página está en pausa hasta que lo respondas \
             con la herramienta dialog."
        ));
    }
    summary
}

fn tab_list(browser: &Browser) -> String {
    let active = browser.active_index();
    browser
        .tab_infos()
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let marker = if index == active { "▶" } else { " " };
            let url = tab
                .url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("{marker} {}. {} — {url}", index + 1, tab.title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convierte la respuesta de `READ_PAGE` en texto para el agente.
fn format_page(data: &Value) -> String {
    let mut out = format!(
        "Título: {}\nDirección: {}\n",
        data["title"].as_str().unwrap_or_default(),
        data["url"].as_str().unwrap_or_default()
    );
    let scroll = data["scroll"].as_u64().unwrap_or(0);
    let scroll_max = data["scroll_max"].as_u64().unwrap_or(0);
    if scroll_max > 0 {
        out.push_str(&format!("Desplazamiento: {scroll} de {scroll_max} px\n"));
    }
    out.push_str("\n## Texto\n");
    out.push_str(data["text"].as_str().unwrap_or_default().trim());
    if data["truncated"] == true {
        out.push_str("\n[… texto recortado; sube max_chars para leer más]");
    }
    out.push_str("\n\n## Elementos visibles (tipo «texto» → x, y para click)\n");
    let elements = data["elements"].as_array().cloned().unwrap_or_default();
    if elements.is_empty() {
        out.push_str("(ninguno)\n");
    }
    for element in &elements {
        let field = |index: usize| element.get(index).cloned().unwrap_or(Value::Null);
        let kind = field(0);
        let label = field(1);
        out.push_str(&format!(
            "- {} «{}» → {}, {}\n",
            kind.as_str().unwrap_or("?"),
            label.as_str().unwrap_or_default(),
            field(2),
            field(3)
        ));
    }
    let offscreen = data["offscreen"].as_u64().unwrap_or(0);
    if offscreen > 0 {
        out.push_str(&format!(
            "({offscreen} elementos más fuera de la vista: usa scroll para verlos)\n"
        ));
    }
    out
}

fn show_js(value: JSValue) -> String {
    match value {
        JSValue::Undefined => "undefined".to_owned(),
        JSValue::String(text) => text,
        other => serde_json::to_string_pretty(&js_to_json(other)).unwrap_or_default(),
    }
}

fn js_to_json(value: JSValue) -> Value {
    match value {
        JSValue::Undefined | JSValue::Null => Value::Null,
        JSValue::Boolean(value) => Value::Bool(value),
        JSValue::Number(number) => json!(number),
        JSValue::String(text) => Value::String(text),
        JSValue::Element(_) => json!("[elemento]"),
        JSValue::ShadowRoot(_) => json!("[shadow root]"),
        JSValue::Frame(_) | JSValue::Window(_) => json!("[ventana]"),
        JSValue::Array(items) => Value::Array(items.into_iter().map(js_to_json).collect()),
        JSValue::Object(map) => {
            let mut entries: Vec<(String, JSValue)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, js_to_json(value)))
                    .collect(),
            )
        },
    }
}

fn script_error(error: &JavaScriptEvaluationError) -> String {
    match error {
        JavaScriptEvaluationError::EvaluationFailure(Some(info)) => format!(
            "Error de JavaScript: {} (línea {}, columna {})",
            info.message, info.line_number, info.column
        ),
        JavaScriptEvaluationError::EvaluationFailure(None) => {
            "El script lanzó un error.".to_owned()
        },
        JavaScriptEvaluationError::CompilationFailure => {
            "El script tiene un error de sintaxis.".to_owned()
        },
        JavaScriptEvaluationError::DocumentNotFound
        | JavaScriptEvaluationError::WebViewNotReady => {
            "La página aún no está lista; prueba otra vez en un momento.".to_owned()
        },
        JavaScriptEvaluationError::SerializationError(_) => {
            "El script funcionó, pero su resultado no se puede convertir (¿una promesa o una \
             función?)."
                .to_owned()
        },
        JavaScriptEvaluationError::InternalError => "Error interno de Servo.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_aliases() {
        assert_eq!(parse_key("a"), Some(Key::Character("a".to_owned())));
        assert_eq!(parse_key("Enter"), Some(Key::Named(NamedKey::Enter)));
        assert_eq!(parse_key("intro"), Some(Key::Named(NamedKey::Enter)));
        assert_eq!(parse_key("PageDown"), Some(Key::Named(NamedKey::PageDown)));
        assert_eq!(parse_key("Space"), Some(Key::Character(" ".to_owned())));
        assert_eq!(parse_key("NoExiste"), None);
    }

    #[test]
    fn modifiers_are_validated() {
        let both = parse_modifiers(Some(&json!(["Control", "Shift"]))).expect("válidos");
        assert_eq!(both, Modifiers::CONTROL | Modifiers::SHIFT);
        assert!(parse_modifiers(Some(&json!(["Hyper"]))).is_err());
        assert_eq!(parse_modifiers(None), Ok(Modifiers::empty()));
    }

    #[test]
    fn js_values_become_json() {
        let mut map = std::collections::HashMap::new();
        map.insert("b".to_owned(), JSValue::Number(2.0));
        map.insert("a".to_owned(), JSValue::Array(vec![JSValue::Boolean(true)]));
        assert_eq!(
            js_to_json(JSValue::Object(map)),
            json!({ "a": [true], "b": 2.0 })
        );
        assert_eq!(show_js(JSValue::String("hola".to_owned())), "hola");
        assert_eq!(show_js(JSValue::Undefined), "undefined");
    }

    #[test]
    fn page_text_lists_elements() {
        let data = json!({
            "title": "Ejemplo",
            "url": "https://example.com/",
            "text": "Hola",
            "truncated": false,
            "elements": [["enlace", "Más información", 640, 412]],
            "offscreen": 3,
            "scroll": 0,
            "scroll_max": 0,
        });
        let text = format_page(&data);
        assert!(text.contains("Título: Ejemplo"));
        assert!(text.contains("- enlace «Más información» → 640, 412"));
        assert!(text.contains("3 elementos más"));
    }
}
