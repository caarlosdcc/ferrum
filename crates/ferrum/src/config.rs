/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Opciones de línea de comandos. Sin dependencias externas a propósito:
//! menos código de terceros en el binario.

use std::path::PathBuf;

use ferrum_shield::omnibox::SearchEngine;

pub const HELP: &str = "\
Ferrum - navegador ligero, seguro y privado

USO:
    ferrum [OPCIONES] [URL o búsqueda]

OPCIONES:
    --strict              Modo estricto: desactiva el JIT de JavaScript y WebAssembly
                          (cierra la vía de ataque más usada contra navegadores),
                          aísla cada página en su propio proceso y, en Linux/macOS,
                          la encierra en un sandbox. Más seguro, algo más lento.
    --multiprocess        Aísla el contenido web en procesos separados.
    --blocklist <ARCHIVO> Carga una lista extra de rastreadores (hosts, dominios
                          o reglas ||dominio^ de Adblock). Repetible.
    --malware-list <ARCHIVO>
                          Carga una lista de dominios maliciosos o de phishing.
                          Se bloquean incluso al navegar directamente. Repetible.
    --search <ddg|mojeek> Buscador de la barra de direcciones (por defecto: ddg).
    --proxy <URL>         Envía todo el tráfico por un proxy HTTP, p. ej.
                          http://127.0.0.1:8080 (SOCKS aún no está soportado).
    -h, --help            Muestra esta ayuda.
    -V, --version         Muestra la versión.

ATAJOS:
    Ctrl+L / F6     Escribir dirección o búsqueda (se muestra en la barra de título)
    Ctrl+T          Nueva pestaña          Ctrl+W      Cerrar pestaña
    Ctrl+Tab        Pestaña siguiente      Ctrl+Mayús+Tab  Anterior
    Alt+← / Alt+→   Atrás / Adelante       F5 / Ctrl+R Recargar
    Ctrl++ / Ctrl+- / Ctrl+0               Zoom
";

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub strict: bool,
    pub multiprocess: bool,
    pub blocklists: Vec<PathBuf>,
    pub malware_lists: Vec<PathBuf>,
    pub search: SearchEngine,
    pub proxy: Option<String>,
    pub initial_input: Option<String>,
}

pub enum ParseOutcome {
    Run(Config),
    Exit(String),
}

impl Config {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
        let mut config = Config::default();
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .ok_or_else(|| format!("falta el valor de {name}"))
            };
            match arg.as_str() {
                "-h" | "--help" => return Ok(ParseOutcome::Exit(HELP.to_owned())),
                "-V" | "--version" => {
                    return Ok(ParseOutcome::Exit(format!(
                        "Ferrum {}",
                        env!("CARGO_PKG_VERSION")
                    )));
                },
                "--strict" => {
                    config.strict = true;
                    config.multiprocess = true;
                },
                "--multiprocess" => config.multiprocess = true,
                "--blocklist" => config.blocklists.push(value("--blocklist")?.into()),
                "--malware-list" => config.malware_lists.push(value("--malware-list")?.into()),
                "--search" => {
                    config.search = match value("--search")?.as_str() {
                        "ddg" | "duckduckgo" => SearchEngine::DUCKDUCKGO,
                        "mojeek" => SearchEngine::MOJEEK,
                        other => return Err(format!("buscador desconocido: {other}")),
                    }
                },
                "--proxy" => config.proxy = Some(value("--proxy")?),
                flag if flag.starts_with('-') => {
                    return Err(format!("opción desconocida: {flag}\n\n{HELP}"));
                },
                input => {
                    config.initial_input = Some(match config.initial_input.take() {
                        Some(previous) => format!("{previous} {input}"),
                        None => input.to_owned(),
                    });
                },
            }
        }

        Ok(ParseOutcome::Run(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ParseOutcome, String> {
        Config::from_args(args.iter().map(|s| (*s).to_owned()))
    }

    fn run(args: &[&str]) -> Config {
        match parse(args) {
            Ok(ParseOutcome::Run(config)) => config,
            _ => panic!("se esperaba una configuración para {args:?}"),
        }
    }

    #[test]
    fn defaults_are_safe() {
        let config = run(&[]);
        assert!(!config.strict);
        assert!(!config.multiprocess);
        assert!(config.proxy.is_none());
        assert_eq!(config.search.name, "DuckDuckGo");
    }

    #[test]
    fn strict_implies_multiprocess() {
        let config = run(&["--strict"]);
        assert!(config.strict && config.multiprocess);
    }

    #[test]
    fn collects_lists_and_search_words() {
        let config = run(&[
            "--blocklist",
            "a.txt",
            "--malware-list",
            "b.txt",
            "--search",
            "mojeek",
            "hola",
            "mundo",
        ]);
        assert_eq!(config.blocklists.len(), 1);
        assert_eq!(config.malware_lists.len(), 1);
        assert_eq!(config.search.name, "Mojeek");
        assert_eq!(config.initial_input.as_deref(), Some("hola mundo"));
    }

    #[test]
    fn rejects_unknown_flags_and_missing_values() {
        assert!(parse(&["--inventado"]).is_err());
        assert!(parse(&["--blocklist"]).is_err());
        assert!(parse(&["--search", "google"]).is_err());
    }

    #[test]
    fn help_exits() {
        assert!(matches!(parse(&["--help"]), Ok(ParseOutcome::Exit(_))));
    }
}
