/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Ferrum: navegador web ligero, seguro y privado, escrito en Rust sobre Servo.

// En Windows, la versión de distribución no abre una consola negra al lado.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod browser;
mod config;
mod gui;
mod hardening;
mod input;
mod pages;

use std::process::ExitCode;

use winit::event_loop::EventLoop;

use crate::app::App;
use crate::config::{Config, ParseOutcome};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();

    // En modo multiproceso, Servo relanza este mismo ejecutable para cada
    // proceso de contenido web.
    if args.peek().map(String::as_str) == Some("--content-process") {
        args.next();
        let Some(token) = args.next() else {
            eprintln!("--content-process requiere un token");
            return ExitCode::FAILURE;
        };
        servo::run_content_process(token);
        return ExitCode::SUCCESS;
    }

    let config = match Config::from_args(args) {
        Ok(ParseOutcome::Run(config)) => config,
        Ok(ParseOutcome::Exit(message)) => {
            println!("{message}");
            return ExitCode::SUCCESS;
        },
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        },
    };

    // Criptografía TLS: aws-lc-rs (auditado, con verificación formal de partes
    // críticas). Debe instalarse antes de cualquier conexión.
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        eprintln!("No se pudo inicializar el proveedor criptográfico");
        return ExitCode::FAILURE;
    }

    let event_loop = match EventLoop::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("No se pudo crear el bucle de eventos: {error}");
            return ExitCode::FAILURE;
        },
    };

    let mut app = App::new(&event_loop, config);
    match event_loop.run_app(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error en el bucle de eventos: {error}");
            ExitCode::FAILURE
        },
    }
}
