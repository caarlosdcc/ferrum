/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Listas de bloqueo de dominios.
//!
//! La búsqueda recorre los sufijos del host (`a.b.ejemplo.com` → `b.ejemplo.com`
//! → `ejemplo.com` → `com`) contra un `HashMap`, así que cuesta O(etiquetas)
//! por petición sin importar cuántos dominios haya en la lista: decenas de
//! miles de entradas no ralentizan la navegación.
//!
//! Formatos de lista aceptados por [`Blocklist::load_list`]:
//! - Un dominio por línea: `ejemplo.com`
//! - Archivo hosts: `0.0.0.0 ejemplo.com` o `127.0.0.1 ejemplo.com`
//! - Reglas de red de Adblock Plus: `||ejemplo.com^` (las demás se ignoran)
//! - Dominio con ruta: `ejemplo.com/pixel`
//! - Comentarios con `#` o `!`

use std::collections::HashMap;

use url::Url;

/// Lista integrada (ver `lists/trackers.txt`).
const BUILTIN_TRACKERS: &str = include_str!("../lists/trackers.txt");

/// Tipo de amenaza de una entrada.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    /// Rastreo, analítica invasiva o publicidad. Se bloquea solo como tercero.
    Tracker,
    /// Malware, phishing o estafas. Se bloquea siempre, incluso al navegar.
    Malware,
}

#[derive(Clone, Debug)]
struct PathRule {
    domain: String,
    path_prefix: String,
    category: Category,
}

/// Conjunto de dominios y rutas bloqueadas.
#[derive(Clone, Debug, Default)]
pub struct Blocklist {
    domains: HashMap<String, Category>,
    path_rules: Vec<PathRule>,
}

impl Blocklist {
    /// Lista vacía.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lista con los rastreadores integrados en Ferrum.
    pub fn with_builtin() -> Self {
        let mut list = Self::new();
        list.load_list(BUILTIN_TRACKERS, Category::Tracker);
        list
    }

    /// Número de reglas cargadas.
    pub fn len(&self) -> usize {
        self.domains.len() + self.path_rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Añade un dominio (y todos sus subdominios). Si ya existía, la categoría
    /// `Malware` tiene prioridad sobre `Tracker`.
    pub fn add_domain(&mut self, domain: &str, category: Category) {
        let Some(domain) = normalize_domain(domain) else {
            return;
        };
        self.domains
            .entry(domain)
            .and_modify(|existing| {
                if category == Category::Malware {
                    *existing = Category::Malware;
                }
            })
            .or_insert(category);
    }

    /// Añade una regla `dominio + prefijo de ruta`.
    pub fn add_path_rule(&mut self, domain: &str, path_prefix: &str, category: Category) {
        let Some(domain) = normalize_domain(domain) else {
            return;
        };
        let path_prefix = if path_prefix.starts_with('/') {
            path_prefix.to_owned()
        } else {
            format!("/{path_prefix}")
        };
        self.path_rules.push(PathRule {
            domain,
            path_prefix,
            category,
        });
    }

    /// Carga una lista de texto y devuelve cuántas reglas añadió.
    pub fn load_list(&mut self, text: &str, category: Category) -> usize {
        let before = self.len();
        for line in text.lines() {
            match parse_line(line) {
                Some(Rule::Domain(domain)) => self.add_domain(domain, category),
                Some(Rule::Path(domain, path)) => self.add_path_rule(domain, path, category),
                None => {},
            }
        }
        self.len() - before
    }

    /// Busca la URL en la lista. Devuelve el dominio que coincidió y su
    /// categoría. Si coinciden varias reglas gana `Malware`.
    pub fn lookup(&self, url: &Url) -> Option<(String, Category)> {
        let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
        let mut found: Option<(String, Category)> = None;

        let mut consider = |domain: &str, category: Category| {
            let better = match &found {
                None => true,
                Some((_, existing)) => {
                    *existing == Category::Tracker && category == Category::Malware
                },
            };
            if better {
                found = Some((domain.to_owned(), category));
            }
        };

        for suffix in host_suffixes(&host) {
            if let Some(category) = self.domains.get(suffix) {
                consider(suffix, *category);
            }
        }

        if !self.path_rules.is_empty() {
            let path = url.path();
            for rule in &self.path_rules {
                if host_matches(&host, &rule.domain) && path.starts_with(&rule.path_prefix) {
                    consider(&rule.domain, rule.category);
                }
            }
        }

        found
    }
}

enum Rule<'a> {
    Domain(&'a str),
    Path(&'a str, &'a str),
}

fn parse_line(line: &str) -> Option<Rule<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('!') || line.starts_with('[') {
        return None;
    }

    // Adblock Plus: solo reglas de red de dominio completo, `||dominio^`.
    if let Some(rest) = line.strip_prefix("||") {
        let end = rest.find(['^', '$', '/', '*']).unwrap_or(rest.len());
        let domain = &rest[..end];
        let tail = &rest[end..];
        // Reglas con comodines o rutas no se interpretan aquí.
        if tail.is_empty() || (tail.starts_with('^') && !tail.contains('*')) {
            return Some(Rule::Domain(domain));
        }
        return None;
    }
    // Excepciones y reglas cosméticas de Adblock: no se aplican.
    if line.starts_with("@@") || line.contains("##") || line.contains("#@#") {
        return None;
    }

    // Formato hosts: "0.0.0.0 dominio" (con comentario opcional al final).
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let entry = if matches!(first, "0.0.0.0" | "127.0.0.1" | "::" | "::1") {
        words.next()?
    } else {
        first
    };
    let entry = entry.split('#').next()?.trim();

    match entry.split_once('/') {
        Some((domain, path)) if !path.is_empty() => Some(Rule::Path(domain, path)),
        Some((domain, _)) => Some(Rule::Domain(domain)),
        None => Some(Rule::Domain(entry)),
    }
}

/// Valida y normaliza un dominio. Rechaza IPs, comodines y nombres locales.
fn normalize_domain(domain: &str) -> Option<String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let valid = domain.contains('.')
        && !domain.starts_with('.')
        && !domain.starts_with('-')
        && domain.len() <= 253
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !matches!(domain.as_str(), "localhost.localdomain" | "local")
        && !domain.chars().all(|c| c.is_ascii_digit() || c == '.');
    valid.then_some(domain)
}

/// `a.b.c` → `["a.b.c", "b.c", "c"]`
fn host_suffixes(host: &str) -> impl Iterator<Item = &str> {
    std::iter::once(host).chain(host.match_indices('.').map(move |(i, _)| &host[i + 1..]))
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("url")
    }

    #[test]
    fn builtin_list_loads() {
        let list = Blocklist::with_builtin();
        assert!(
            list.len() > 150,
            "la lista integrada parece incompleta: {}",
            list.len()
        );
        assert!(
            list.lookup(&url("https://www.google-analytics.com/analytics.js"))
                .is_some()
        );
        assert!(
            list.lookup(&url("https://securepubads.g.doubleclick.net/tag/js/gpt.js"))
                .is_some()
        );
        assert!(list.lookup(&url("https://www.wikipedia.org/")).is_none());
    }

    #[test]
    fn matches_subdomains_but_not_lookalikes() {
        let mut list = Blocklist::new();
        list.add_domain("tracker.com", Category::Tracker);
        assert!(list.lookup(&url("https://tracker.com/")).is_some());
        assert!(list.lookup(&url("https://a.b.tracker.com/x")).is_some());
        assert!(list.lookup(&url("https://nottracker.com/")).is_none());
        assert!(
            list.lookup(&url("https://tracker.com.example.org/"))
                .is_none()
        );
    }

    #[test]
    fn path_rules() {
        let mut list = Blocklist::new();
        list.add_path_rule("facebook.com", "/tr", Category::Tracker);
        assert!(
            list.lookup(&url("https://www.facebook.com/tr?id=1"))
                .is_some()
        );
        assert!(
            list.lookup(&url("https://www.facebook.com/profile"))
                .is_none()
        );
    }

    #[test]
    fn parses_common_formats() {
        let mut list = Blocklist::new();
        let added = list.load_list(
            "# comentario\n\
             ! comentario adblock\n\
             0.0.0.0 hosts-format.com\n\
             127.0.0.1 other-hosts.net # nota\n\
             ||adblock-format.org^\n\
             ||with-options.io^$third-party\n\
             plain-domain.co\n\
             @@||exception.com^\n\
             example.com##.banner\n\
             ||wild*card.com^\n\
             0.0.0.0 0.0.0.0\n",
            Category::Tracker,
        );
        assert_eq!(added, 5);
        for host in [
            "hosts-format.com",
            "other-hosts.net",
            "adblock-format.org",
            "with-options.io",
            "plain-domain.co",
        ] {
            assert!(
                list.lookup(&url(&format!("https://{host}/"))).is_some(),
                "{host}"
            );
        }
        assert!(list.lookup(&url("https://exception.com/")).is_none());
    }

    #[test]
    fn malware_wins_over_tracker() {
        let mut list = Blocklist::new();
        list.add_domain("bad.com", Category::Tracker);
        list.add_domain("bad.com", Category::Malware);
        list.add_domain("x.bad.com", Category::Tracker);
        let (_, category) = list.lookup(&url("https://x.bad.com/")).expect("coincide");
        assert_eq!(category, Category::Malware);
    }
}
