/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Reglas de bloqueo con la sintaxis de Adblock Plus y uBlock Origin
//! (EasyList, EasyPrivacy…), sobre el motor de Brave (`adblock`, MPL-2.0).
//!
//! Hace dos cosas:
//! - decide si una petición de red es un anuncio o un rastreador
//!   (`||ejemplo.com/ads/^$script,third-party`, excepciones `@@`…);
//! - da los selectores CSS de los huecos de anuncios que hay que ocultar en
//!   cada página (`ejemplo.com##.banner`, y los genéricos `##.ad-box`).

use std::collections::HashSet;
use std::fmt;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use url::Url;

/// Tipo de recurso que se pide, para reglas como `$script` o `$image`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Subdocument,
    Script,
    Stylesheet,
    Image,
    Font,
    Media,
    Object,
    Xhr,
    Ping,
    WebSocket,
    Other,
}

impl ResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            ResourceKind::Subdocument => "subdocument",
            ResourceKind::Script => "script",
            ResourceKind::Stylesheet => "stylesheet",
            ResourceKind::Image => "image",
            ResourceKind::Font => "font",
            ResourceKind::Media => "media",
            ResourceKind::Object => "object",
            ResourceKind::Xhr => "xmlhttprequest",
            ResourceKind::Ping => "ping",
            ResourceKind::WebSocket => "websocket",
            ResourceKind::Other => "other",
        }
    }
}

/// Qué ocultar en una página.
#[derive(Debug, Default)]
pub struct PageCosmetics {
    /// Selectores propios de este sitio (`sitio.com##selector`).
    pub selectors: Vec<String>,
    /// Si se pueden aplicar además las reglas genéricas (no hay `$generichide`).
    pub generic: bool,
    /// Clases e ids que el sitio exime de las reglas genéricas (`#@#`).
    pub exceptions: HashSet<String>,
}

/// Motor de reglas, ya compilado. Se puede construir en otro hilo y pasarlo
/// después al navegador.
pub struct AdRules {
    engine: Engine,
    rules: usize,
}

impl fmt::Debug for AdRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdRules")
            .field("rules", &self.rules)
            .finish_non_exhaustive()
    }
}

impl AdRules {
    /// Compila una o varias listas de filtros. Las líneas que no entiende se
    /// ignoran, como hacen los bloqueadores.
    pub fn parse<'a>(lists: impl IntoIterator<Item = &'a str>) -> Self {
        let mut set = FilterSet::new(false);
        let mut rules = 0;
        for list in lists {
            rules += list
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('!') && !line.starts_with('['))
                .count();
            set.add_filter_list(list.to_owned(), ParseOptions::default());
        }
        AdRules {
            engine: Engine::new_with_filter_set(set),
            rules,
        }
    }

    /// Reglas leídas (aproximado: líneas que no son comentarios).
    pub fn rule_count(&self) -> usize {
        self.rules
    }

    /// `true` si hay que bloquear `url`, pedida por la página `source`.
    pub fn should_block(&self, url: &Url, source: Option<&Url>, kind: ResourceKind) -> bool {
        let source = source.unwrap_or(url);
        match Request::new(url.as_str(), source.as_str(), kind.as_str(), "GET") {
            Ok(request) => self.engine.check_network_request(&request).should_block(),
            Err(_) => false,
        }
    }

    /// Selectores que ocultar en la página `url` (los propios de su sitio).
    pub fn page_cosmetics(&self, url: &Url) -> PageCosmetics {
        let resources = self.engine.url_cosmetic_resources(url.as_str());
        let mut selectors: Vec<String> = resources.hide_selectors.into_iter().collect();
        selectors.sort();
        PageCosmetics {
            selectors,
            generic: !resources.generichide,
            exceptions: resources.exceptions,
        }
    }

    /// Selectores genéricos (`##.ad-box`) que afectan a las clases e ids que
    /// tiene la página.
    pub fn generic_selectors(
        &self,
        classes: &[String],
        ids: &[String],
        exceptions: &HashSet<String>,
    ) -> Vec<String> {
        let mut selectors = self
            .engine
            .hidden_class_id_selectors(classes, ids, exceptions);
        selectors.sort();
        selectors.dedup();
        selectors
    }
}

/// Hoja de estilo que oculta los selectores, una regla por selector: si el
/// motor no entiende uno, solo se pierde esa regla y no todas.
pub fn hiding_css(selectors: &[String]) -> String {
    let mut css = String::new();
    for selector in selectors {
        // Nada que pueda cerrar la regla y añadir otra.
        if selector.contains(['{', '}']) || selector.contains("</") {
            continue;
        }
        css.push_str(selector);
        css.push_str("{display:none!important}\n");
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("URL de prueba válida")
    }

    const LIST: &str = "\
! Lista de prueba
||ads.example^$third-party
/banner/*$image
@@||ads.example/allowed.js
example.com##.promo
##.ad-box
example.org#@#.ad-box
";

    #[test]
    fn blocks_network_requests_by_rule() {
        let rules = AdRules::parse([LIST]);
        let page = url("https://news.test/");
        assert!(rules.should_block(
            &url("https://ads.example/x.js"),
            Some(&page),
            ResourceKind::Script
        ));
        // Excepción `@@`.
        assert!(!rules.should_block(
            &url("https://ads.example/allowed.js"),
            Some(&page),
            ResourceKind::Script
        ));
        // `$image`: un script en esa ruta no se bloquea, una imagen sí.
        let banner = url("https://cdn.test/banner/1.png");
        assert!(rules.should_block(&banner, Some(&page), ResourceKind::Image));
        assert!(!rules.should_block(&banner, Some(&page), ResourceKind::Script));
        assert_eq!(rules.rule_count(), 6);
    }

    #[test]
    fn gives_site_and_generic_selectors() {
        let rules = AdRules::parse([LIST]);
        let page = rules.page_cosmetics(&url("https://www.example.com/a"));
        assert_eq!(page.selectors, vec![".promo"]);
        assert!(page.generic);
        let generic = rules.generic_selectors(
            &["ad-box".to_owned(), "otra".to_owned()],
            &[],
            &page.exceptions,
        );
        assert_eq!(generic, vec![".ad-box"]);

        // example.org exime .ad-box.
        let exempt = rules.page_cosmetics(&url("https://example.org/"));
        let generic = rules.generic_selectors(&["ad-box".to_owned()], &[], &exempt.exceptions);
        assert!(generic.is_empty());
    }

    #[test]
    fn css_skips_selectors_that_could_escape() {
        let css = hiding_css(&[".a".to_owned(), ".b{color:red}".to_owned(), "#c".to_owned()]);
        assert_eq!(
            css,
            ".a{display:none!important}\n#c{display:none!important}\n"
        );
    }
}
