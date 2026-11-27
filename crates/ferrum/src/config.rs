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
    --no-mse              Sin Media Source Extensions: las webs de vídeo usan su
                          formato básico (YouTube en 360p). Por defecto están
                          activas (vídeo en HD; solo MP4 con H.264/AAC).
    --no-filter-lists     No descargar EasyList ni EasyPrivacy (sin bloqueo de
                          anuncios; queda la lista integrada de rastreadores).
    --malware-list <ARCHIVO>
                          Carga una lista de dominios maliciosos o de phishing.
                          Se bloquean incluso al navegar directamente. Repetible.
    --search <ddg|mojeek> Buscador de la barra de direcciones (por defecto: ddg).
    --proxy <URL>         Envía todo el tráfico por un proxy HTTP, p. ej.
                          http://127.0.0.1:8080 (SOCKS aún no está soportado).
    --screenshot <ARCHIVO.png>
                          Guarda una captura de la ventana cuando la página termina
                          de cargar y se cierra. Pensado para pruebas automáticas.
    --click <X,Y>         Con --screenshot: antes de la captura, hace clic en ese
                          punto de la página (en píxeles), p. ej. para abrir una
                          lista desplegable.
    --mcp                 Servidor MCP: deja que un agente de IA (p. ej. Claude
                          Code) use Ferrum por la entrada y salida estándar. No
                          abre ningún puerto de red, y la ventana no aparece hasta
                          que el agente la usa.
    -h, --help            Muestra esta ayuda.
    -V, --version         Muestra la versión.

ATAJOS:
    Ctrl+L / F6     Ir a la barra de direcciones
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
    /// Descargar y aplicar EasyList y EasyPrivacy.
    pub filter_lists: bool,
    /// Media Source Extensions (`--no-mse` las quita). Sin el reproductor de
    /// vídeo (compilado sin `media`) no sirven de nada.
    pub mse: bool,
    pub search: SearchEngine,
    pub proxy: Option<String>,
    pub screenshot: Option<PathBuf>,
    pub click: Option<(f32, f32)>,
    /// `--mcp`: proceso que habla con el agente (sin ventana).
    pub mcp_relay: bool,
    /// `--mcp-browser` (uso interno): navegador controlado por ese proceso.
    pub mcp: bool,
    pub initial_input: Option<String>,
}

pub enum ParseOutcome {
    Run(Config),
    Exit(String),
}

impl Config {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
        let mut config = Config {
            filter_lists: true,
            mse: cfg!(feature = "media"),
            ..Config::default()
        };
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
                "--no-filter-lists" => config.filter_lists = false,
                "--no-mse" => config.mse = false,
                // Antes eran opcionales; se acepta para no romper accesos directos.
                "--mse" => config.mse = cfg!(feature = "media"),
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
                "--screenshot" => config.screenshot = Some(value("--screenshot")?.into()),
                "--click" => config.click = Some(parse_point(&value("--click")?)?),
                "--mcp" => config.mcp_relay = true,
                "--mcp-browser" => config.mcp = true,
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

/// `"120,45"` → `(120.0, 45.0)`.
fn parse_point(text: &str) -> Result<(f32, f32), String> {
    let invalid = || format!("punto no válido: {text} (formato: X,Y)");
    let (x, y) = text.split_once(',').ok_or_else(invalid)?;
    let x: f32 = x.trim().parse().map_err(|_| invalid())?;
    let y: f32 = y.trim().parse().map_err(|_| invalid())?;
    if !(x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0) {
        return Err(invalid());
    }
    Ok((x, y))
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
        assert!(config.screenshot.is_none());
        assert!(!config.mcp && !config.mcp_relay);
        assert!(config.filter_lists);
        assert!(!run(&["--no-filter-lists"]).filter_lists);
        assert_eq!(config.mse, cfg!(feature = "media"));
        assert!(!run(&["--no-mse"]).mse);
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
    fn parses_test_click() {
        let config = run(&["--screenshot", "a.png", "--click", "200, 130"]);
        assert_eq!(config.click, Some((200.0, 130.0)));
        assert!(parse(&["--click", "200"]).is_err());
        assert!(parse(&["--click", "-1,5"]).is_err());
    }

    #[test]
    fn mcp_is_opt_in() {
        let relay = run(&["--mcp"]);
        assert!(relay.mcp_relay && !relay.mcp);
        assert!(run(&["--mcp-browser"]).mcp);
    }

    #[test]
    fn help_exits() {
        assert!(matches!(parse(&["--help"]), Ok(ParseOutcome::Exit(_))));
    }
}
