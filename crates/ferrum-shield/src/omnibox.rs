/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Interpretación segura de lo que el usuario escribe en la barra de direcciones.
//!
//! Reglas:
//! - Las direcciones sin esquema se abren con `https://` (nunca `http://`).
//! - `http://` se convierte en `https://`, salvo en `localhost` (desarrollo).
//! - Esquemas peligrosos (`javascript:`, `file:`, `data:`…) no se abren: se
//!   buscan como texto. Así un "pega esto en tu barra" no puede ejecutar código.
//! - Todo lo demás es una búsqueda en un buscador que no rastrea.

use url::Url;

use crate::domain;

/// Buscador usado para las búsquedas desde la barra.
#[derive(Clone, Debug)]
pub struct SearchEngine {
    pub name: &'static str,
    /// Plantilla con `{q}` donde va la consulta codificada.
    pub template: &'static str,
}

impl SearchEngine {
    /// DuckDuckGo en su versión HTML: sin rastreo, sin JavaScript pesado y
    /// muy ligera en memoria.
    pub const DUCKDUCKGO: SearchEngine = SearchEngine {
        name: "DuckDuckGo",
        template: "https://html.duckduckgo.com/html/?q={q}",
    };

    /// Mojeek: buscador independiente con índice propio y sin rastreo.
    pub const MOJEEK: SearchEngine = SearchEngine {
        name: "Mojeek",
        template: "https://www.mojeek.com/search?q={q}",
    };

    /// URL de búsqueda para `query`.
    pub fn search_url(&self, query: &str) -> Option<Url> {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        Url::parse(&self.template.replace("{q}", &encoded)).ok()
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self::DUCKDUCKGO
    }
}

/// Convierte la entrada del usuario en la URL a cargar.
pub fn resolve(input: &str, engine: &SearchEngine) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    if let Some(url) = as_web_url(input) {
        return Some(url);
    }

    engine.search_url(input)
}

fn as_web_url(input: &str) -> Option<Url> {
    if input.contains(char::is_whitespace) {
        return None;
    }

    if let Ok(url) = Url::parse(input) {
        match url.scheme() {
            "https" => return Some(url),
            "http" => return Some(upgrade_to_https(url)),
            // "localhost:8080" se interpreta como esquema "localhost"; lo
            // tratamos abajo como host. Cualquier otro esquema no se abre.
            scheme if scheme != "localhost" && !looks_like_host(scheme) => return None,
            _ => {},
        }
    }

    if !looks_like_host(input.split(['/', '?', '#']).next().unwrap_or_default()) {
        return None;
    }

    let candidate = Url::parse(&format!("https://{input}")).ok()?;
    if domain::is_local(&candidate) {
        // Los servidores de desarrollo locales rara vez tienen TLS.
        let mut local = candidate;
        local.set_scheme("http").ok()?;
        return Some(local);
    }
    Some(candidate)
}

/// `http://sitio` → `https://sitio` (excepto en hosts locales).
pub fn upgrade_to_https(url: Url) -> Url {
    if url.scheme() != "http" || domain::is_local(&url) {
        return url;
    }
    let mut upgraded = url.clone();
    if upgraded.set_scheme("https").is_err() {
        return url;
    }
    if upgraded.port() == Some(80) {
        let _ = upgraded.set_port(None);
    }
    upgraded
}

/// ¿Parece un nombre de host (`ejemplo.com`, `localhost:3000`, `10.0.0.1`)?
fn looks_like_host(text: &str) -> bool {
    let host = text.rsplit_once(':').map_or(text, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
            host
        } else {
            text
        }
    });
    if host == "localhost" {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2 &&
        labels.iter().all(|label| {
            !label.is_empty() &&
                !label.starts_with('-') &&
                label.chars().all(|c| c.is_alphanumeric() || c == '-')
        }) &&
        // El TLD debe tener letras (o ser una IP completa).
        labels
            .last()
            .is_some_and(|tld| tld.chars().any(char::is_alphabetic) || labels.len() == 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(input: &str) -> String {
        resolve(input, &SearchEngine::DUCKDUCKGO)
            .map(|u| u.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn domains_open_with_https() {
        assert_eq!(go("example.com"), "https://example.com/");
        assert_eq!(
            go("es.wikipedia.org/wiki/Rust"),
            "https://es.wikipedia.org/wiki/Rust"
        );
        assert_eq!(go("  servo.org  "), "https://servo.org/");
    }

    #[test]
    fn http_is_upgraded() {
        assert_eq!(go("http://example.com/a"), "https://example.com/a");
        assert_eq!(go("http://example.com:80/"), "https://example.com/");
    }

    #[test]
    fn localhost_keeps_http() {
        assert_eq!(go("localhost:3000"), "http://localhost:3000/");
        assert_eq!(go("http://127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
    }

    #[test]
    fn text_becomes_a_search() {
        assert_eq!(
            go("cómo aprender rust"),
            "https://html.duckduckgo.com/html/?q=c%C3%B3mo+aprender+rust"
        );
        assert!(go("rust").starts_with("https://html.duckduckgo.com/"));
    }

    #[test]
    fn dangerous_schemes_are_searched_not_opened() {
        assert!(go("javascript:alert(1)").starts_with("https://html.duckduckgo.com/"));
        assert!(go("file:///C:/Windows/System32").starts_with("https://html.duckduckgo.com/"));
        assert!(
            go("data:text/html,<script>x</script>").starts_with("https://html.duckduckgo.com/")
        );
    }

    #[test]
    fn empty_input_does_nothing() {
        assert!(resolve("   ", &SearchEngine::DUCKDUCKGO).is_none());
    }
}
