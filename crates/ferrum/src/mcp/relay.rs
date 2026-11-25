/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ferrum --mcp`: el proceso que habla con el agente.
//!
//! No abre ventana ni carga el motor web. Responde él mismo a la presentación
//! y a la lista de herramientas, y cuando el agente usa una por primera vez
//! abre el navegador como proceso hijo (`--mcp-browser`) y le pasa las llamadas.
//!
//! - Al agente solo le llegan mensajes JSON-RPC válidos, aunque el motor o sus
//!   bibliotecas escriban otras cosas en la salida estándar.
//! - Si el usuario cierra la ventana, la siguiente herramienta abre otra.
//! - Esperar sin ventana apenas gasta memoria: ni Servo ni OpenGL se cargan.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::protocol::{self, Message, Output};

/// Cuánto se espera a que el navegador se cierre solo antes de forzarlo.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

enum Event {
    Agent(String),
    AgentClosed,
    Browser { generation: u64, line: String },
    BrowserExited(u64),
}

struct BrowserProcess {
    child: Child,
    stdin: ChildStdin,
}

/// Atiende al agente hasta que cierra la conexión. `browser_args` son las
/// opciones con las que se abre cada navegador (sin `--mcp`).
pub fn run(browser_args: Vec<String>) -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("No se encuentra el ejecutable de Ferrum: {error}");
            return ExitCode::FAILURE;
        },
    };
    let (events, receiver) = mpsc::channel();
    let agent = events.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if agent.send(Event::Agent(line)).is_err() {
                return;
            }
        }
        let _ = agent.send(Event::AgentClosed);
    });

    let mut browser: Option<BrowserProcess> = None;
    let mut generation = 0;
    // Llamadas que esperan respuesta del navegador, por su id en JSON.
    let mut pending: HashMap<String, Value> = HashMap::new();

    while let Ok(event) = receiver.recv() {
        match event {
            Event::Agent(line) => match protocol::parse(&line) {
                Message::Reply(reply) => send(&reply.to_string()),
                Message::Ignore => {},
                Message::Cancel(id) => {
                    pending.remove(&id.to_string());
                    if let Some(process) = &mut browser {
                        let _ = forward(process, &line);
                    }
                },
                Message::Call(call) => {
                    if browser.is_none() {
                        generation += 1;
                        match spawn(&exe, &browser_args, generation, events.clone()) {
                            Ok(process) => browser = Some(process),
                            Err(error) => {
                                reply_error(call.id, &format!("No se pudo abrir Ferrum: {error}"));
                                continue;
                            },
                        }
                    }
                    let delivered = browser
                        .as_mut()
                        .is_some_and(|process| forward(process, &line).is_ok());
                    if delivered {
                        pending.insert(call.id.to_string(), call.id);
                    } else {
                        reply_error(call.id, "Ferrum no responde; vuelve a intentarlo.");
                    }
                },
            },
            Event::Browser {
                generation: from,
                line,
            } if from == generation => relay(&line, &mut pending),
            Event::BrowserExited(from) if from == generation => {
                if let Some(mut process) = browser.take() {
                    let _ = process.child.wait();
                }
                for (_, id) in pending.drain() {
                    reply_error(
                        id,
                        "La ventana de Ferrum se cerró. La próxima herramienta abrirá otra.",
                    );
                }
            },
            Event::AgentClosed => break,
            // Restos de un navegador anterior.
            Event::Browser { .. } | Event::BrowserExited(_) => {},
        }
    }

    if let Some(process) = browser {
        close(process);
    }
    ExitCode::SUCCESS
}

fn spawn(
    exe: &Path,
    args: &[String],
    generation: u64,
    events: Sender<Event>,
) -> std::io::Result<BrowserProcess> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .arg("--mcp-browser")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sin consola negra en las compilaciones de depuración.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let missing = || std::io::Error::other("sin canal con el navegador");
    let stdin = child.stdin.take().ok_or_else(missing)?;
    let stdout = child.stdout.take().ok_or_else(missing)?;
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if events.send(Event::Browser { generation, line }).is_err() {
                return;
            }
        }
        let _ = events.send(Event::BrowserExited(generation));
    });
    Ok(BrowserProcess { child, stdin })
}

fn forward(process: &mut BrowserProcess, line: &str) -> std::io::Result<()> {
    writeln!(process.stdin, "{line}")?;
    process.stdin.flush()
}

/// Pasa al agente las respuestas del navegador. Lo que no es JSON-RPC (avisos
/// del motor o de sus bibliotecas) va a la salida de errores.
fn relay(line: &str, pending: &mut HashMap<String, Value>) {
    match serde_json::from_str::<Value>(line) {
        Ok(message) if message.get("jsonrpc").is_some() => {
            if let Some(id) = message.get("id") {
                pending.remove(&id.to_string());
            }
            send(line);
        },
        _ => eprintln!("{line}"),
    }
}

/// Cierra el navegador: sin entrada, termina solo; si no, se fuerza.
fn close(process: BrowserProcess) {
    let BrowserProcess { mut child, stdin } = process;
    drop(stdin);
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn reply_error(id: Value, message: &str) {
    let result = Output::Error(message.to_owned()).into_result();
    send(&protocol::success(id, result).to_string());
}

fn send(line: &str) {
    let mut stdout = std::io::stdout().lock();
    // Si falla, el agente se fue: el bucle termina al cerrarse la entrada.
    let _ = writeln!(stdout, "{line}").and_then(|()| stdout.flush());
}

/// Opciones para cada navegador: las mismas que recibió Ferrum, sin `--mcp`.
pub fn browser_args(args: &[String]) -> Vec<String> {
    args.iter().filter(|arg| *arg != "--mcp").cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_the_mcp_flag() {
        let args: Vec<String> = ["--strict", "--mcp", "--search", "mojeek"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(browser_args(&args), vec!["--strict", "--search", "mojeek"]);
    }

    #[test]
    fn relay_drops_noise_and_clears_pending() {
        let mut pending = HashMap::new();
        pending.insert("7".to_owned(), Value::from(7));
        relay("surfman: aviso cualquiera", &mut pending);
        assert_eq!(pending.len(), 1);
        relay(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, &mut pending);
        assert!(pending.is_empty());
    }
}
