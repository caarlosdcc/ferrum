/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # Ferrum Shield
//!
//! Motor de políticas de privacidad y seguridad de Ferrum.
//!
//! Este crate **no depende del motor web**: recibe URLs y devuelve decisiones
//! (permitir, bloquear, limpiar). Así se puede probar de forma aislada, auditar
//! con facilidad y reutilizar si algún día Ferrum cambia de motor.
//!
//! Piezas principales:
//! - [`blocklist`]: bloqueo de rastreadores, publicidad y dominios maliciosos.
//! - [`url_clean`]: elimina parámetros de rastreo (`utm_*`, `fbclid`, `gclid`…).
//! - [`downloads`]: detecta descargas de ejecutables peligrosos.
//! - [`omnibox`]: convierte lo que escribe el usuario en una URL segura.
//! - [`domain`]: utilidades de dominio (primera parte vs. terceros).

pub mod blocklist;
pub mod domain;
pub mod downloads;
pub mod omnibox;
pub mod url_clean;

use std::cell::Cell;

use url::Url;

pub use crate::blocklist::{Blocklist, Category};

/// Configuración del escudo. `Default` es el modo recomendado: máxima
/// protección sin romper la mayoría de sitios.
#[derive(Clone, Debug)]
pub struct ShieldConfig {
    /// Bloquear peticiones de terceros a rastreadores y publicidad.
    pub block_trackers: bool,
    /// Bloquear la navegación a dominios marcados como maliciosos.
    pub block_malware: bool,
    /// Quitar parámetros de rastreo de las URLs de las páginas.
    pub strip_tracking_params: bool,
    /// Bloquear la descarga directa de ejecutables y scripts.
    pub block_dangerous_downloads: bool,
}

impl Default for ShieldConfig {
    fn default() -> Self {
        Self {
            block_trackers: true,
            block_malware: true,
            strip_tracking_params: true,
            block_dangerous_downloads: true,
        }
    }
}

/// Qué hacer con una petición de red.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Dejarla pasar sin cambios.
    Allow,
    /// Cancelarla (para recursos secundarios: scripts, imágenes, iframes…).
    Block(BlockReason),
    /// Navegar a otra URL en su lugar (por ejemplo, sin parámetros de rastreo).
    Redirect(Url),
}

/// Motivo de un bloqueo, para mostrárselo al usuario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Rastreador o publicidad de terceros.
    Tracker { domain: String },
    /// Dominio en una lista de malware o phishing.
    Malware { domain: String },
    /// Descarga de un tipo de archivo que puede ejecutar código.
    DangerousDownload { extension: String },
}

impl BlockReason {
    /// Código corto y estable, apto para URLs y registros.
    pub fn code(&self) -> &'static str {
        match self {
            BlockReason::Tracker { .. } => "rastreador",
            BlockReason::Malware { .. } => "malware",
            BlockReason::DangerousDownload { .. } => "descarga",
        }
    }

    /// Detalle legible (dominio o extensión).
    pub fn detail(&self) -> &str {
        match self {
            BlockReason::Tracker { domain } | BlockReason::Malware { domain } => domain,
            BlockReason::DangerousDownload { extension } => extension,
        }
    }
}

/// Contadores de lo que el escudo ha bloqueado en esta sesión.
/// Viven solo en memoria: Ferrum no guarda historial.
#[derive(Debug, Default)]
pub struct Stats {
    pub trackers_blocked: Cell<u64>,
    pub malware_blocked: Cell<u64>,
    pub downloads_blocked: Cell<u64>,
    pub urls_cleaned: Cell<u64>,
}

impl Stats {
    fn record(&self, verdict: &Verdict) {
        let counter = match verdict {
            Verdict::Allow => return,
            Verdict::Redirect(_) => &self.urls_cleaned,
            Verdict::Block(BlockReason::Tracker { .. }) => &self.trackers_blocked,
            Verdict::Block(BlockReason::Malware { .. }) => &self.malware_blocked,
            Verdict::Block(BlockReason::DangerousDownload { .. }) => &self.downloads_blocked,
        };
        counter.set(counter.get().saturating_add(1));
    }

    /// Total de amenazas bloqueadas (sin contar URLs limpiadas).
    pub fn total_blocked(&self) -> u64 {
        self.trackers_blocked.get() + self.malware_blocked.get() + self.downloads_blocked.get()
    }
}

/// El escudo: combina listas de bloqueo y políticas en una sola decisión.
#[derive(Debug, Default)]
pub struct Shield {
    config: ShieldConfig,
    blocklist: Blocklist,
    stats: Stats,
}

impl Shield {
    /// Escudo con la lista de bloqueo integrada y la configuración indicada.
    pub fn new(config: ShieldConfig) -> Self {
        Self {
            config,
            blocklist: Blocklist::with_builtin(),
            stats: Stats::default(),
        }
    }

    pub fn config(&self) -> &ShieldConfig {
        &self.config
    }

    pub fn blocklist_mut(&mut self) -> &mut Blocklist {
        &mut self.blocklist
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Decide qué hacer con la carga de un **documento principal** (la página
    /// que el usuario ve en la pestaña, no un iframe).
    pub fn check_document(&self, url: &Url) -> Verdict {
        let verdict = self.document_verdict(url);
        self.stats.record(&verdict);
        verdict
    }

    /// Decide qué hacer con un **recurso secundario** (script, imagen, iframe,
    /// fetch…) cargado por la página `top_level`.
    pub fn check_subresource(&self, url: &Url, top_level: Option<&Url>) -> Verdict {
        let verdict = self.subresource_verdict(url, top_level);
        self.stats.record(&verdict);
        verdict
    }

    fn document_verdict(&self, url: &Url) -> Verdict {
        let Some(host) = url.host_str() else {
            return Verdict::Allow;
        };

        if self.config.block_malware
            && let Some((domain, Category::Malware)) = self.blocklist.lookup(url)
        {
            return Verdict::Block(BlockReason::Malware { domain });
        }

        if self.config.block_dangerous_downloads
            && let Some(extension) = downloads::dangerous_extension(url)
        {
            return Verdict::Block(BlockReason::DangerousDownload { extension });
        }

        if self.config.strip_tracking_params
            && let Some(clean) = url_clean::strip_tracking_params(url)
        {
            log::debug!("URL limpiada en {host}");
            return Verdict::Redirect(clean);
        }

        Verdict::Allow
    }

    fn subresource_verdict(&self, url: &Url, top_level: Option<&Url>) -> Verdict {
        let Some((domain, category)) = self.blocklist.lookup(url) else {
            return Verdict::Allow;
        };

        match category {
            Category::Malware if self.config.block_malware => {
                Verdict::Block(BlockReason::Malware { domain })
            },
            Category::Tracker if self.config.block_trackers => {
                // Un sitio que se carga a sí mismo no es un tercero: visitar
                // facebook.com debe funcionar aunque sus dominios estén en la lista.
                let first_party = top_level.is_some_and(|page| domain::same_site(page, url));
                if first_party {
                    Verdict::Allow
                } else {
                    Verdict::Block(BlockReason::Tracker { domain })
                }
            },
            _ => Verdict::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("URL de prueba válida")
    }

    #[test]
    fn blocks_third_party_trackers() {
        let shield = Shield::new(ShieldConfig::default());
        let page = url("https://example.com/");
        let verdict = shield.check_subresource(
            &url("https://www.google-analytics.com/g/collect"),
            Some(&page),
        );
        assert!(matches!(
            verdict,
            Verdict::Block(BlockReason::Tracker { .. })
        ));
        assert_eq!(shield.stats().trackers_blocked.get(), 1);
    }

    #[test]
    fn allows_first_party_requests() {
        let shield = Shield::new(ShieldConfig::default());
        let page = url("https://www.facebook.com/");
        let verdict =
            shield.check_subresource(&url("https://connect.facebook.net/sdk.js"), Some(&page));
        // facebook.com y facebook.net son sitios distintos para el navegador,
        // así que esto sí es un tercero y debe bloquearse.
        assert!(matches!(verdict, Verdict::Block(_)));

        let page = url("https://news.example.com/");
        let verdict = shield.check_subresource(&url("https://cdn.example.com/app.js"), Some(&page));
        assert_eq!(verdict, Verdict::Allow);
    }

    #[test]
    fn allows_normal_resources() {
        let shield = Shield::new(ShieldConfig::default());
        let page = url("https://example.com/");
        assert_eq!(
            shield.check_subresource(&url("https://cdn.jsdelivr.net/npm/x.js"), Some(&page)),
            Verdict::Allow
        );
    }

    #[test]
    fn cleans_document_urls() {
        let shield = Shield::new(ShieldConfig::default());
        let verdict = shield.check_document(&url("https://example.com/a?utm_source=x&id=7"));
        assert_eq!(
            verdict,
            Verdict::Redirect(url("https://example.com/a?id=7"))
        );
        assert_eq!(shield.stats().urls_cleaned.get(), 1);
    }

    #[test]
    fn blocks_executable_downloads() {
        let shield = Shield::new(ShieldConfig::default());
        let verdict = shield.check_document(&url("https://example.com/free-game.exe"));
        assert!(matches!(
            verdict,
            Verdict::Block(BlockReason::DangerousDownload { .. })
        ));
    }

    #[test]
    fn blocks_malware_domains() {
        let mut shield = Shield::new(ShieldConfig::default());
        shield
            .blocklist_mut()
            .add_domain("evil.example", Category::Malware);
        let verdict = shield.check_document(&url("https://login.evil.example/"));
        assert!(matches!(
            verdict,
            Verdict::Block(BlockReason::Malware { .. })
        ));
    }

    #[test]
    fn respects_disabled_config() {
        let shield = Shield::new(ShieldConfig {
            block_trackers: false,
            block_malware: false,
            strip_tracking_params: false,
            block_dangerous_downloads: false,
        });
        let page = url("https://example.com/");
        assert_eq!(
            shield.check_subresource(&url("https://doubleclick.net/ad.js"), Some(&page)),
            Verdict::Allow
        );
        assert_eq!(
            shield.check_document(&url("https://example.com/?utm_source=x")),
            Verdict::Allow
        );
    }
}
