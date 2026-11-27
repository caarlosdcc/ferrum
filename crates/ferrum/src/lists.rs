/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Listas de filtros contra anuncios y rastreadores (EasyList y EasyPrivacy).
//!
//! No van dentro de Ferrum: cambian a diario y tienen su propia licencia
//! (GPLv3 o CC BY-SA). Se descargan por HTTPS, se guardan en la caché del
//! sistema y se renuevan como mucho una vez al día. Todo ocurre en otro hilo:
//! Ferrum arranca sin esperar, y mientras tanto usa la copia de la última vez.
//!
//! Las listas no son datos de navegación: no dicen nada de lo que visitas, así
//! que guardarlas no rompe la regla de "nada en disco".

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use ferrum_shield::AdRules;

/// Cada cuánto se renueva una lista.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Ninguna lista legítima pesa tanto: protege la memoria si algo va mal.
const MAX_LIST_BYTES: u64 = 20 * 1024 * 1024;

struct FilterList {
    name: &'static str,
    url: &'static str,
    file: &'static str,
}

const LISTS: [FilterList; 2] = [
    FilterList {
        name: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
        file: "easylist.txt",
    },
    FilterList {
        name: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        file: "easyprivacy.txt",
    },
];

/// Empieza a preparar las reglas en otro hilo. Llegan por el canal devuelto
/// (primero las de la caché, luego las renovadas), y `wake` despierta el bucle
/// de la ventana cada vez.
pub fn start(proxy: Option<String>, wake: impl Fn() + Send + 'static) -> Receiver<AdRules> {
    let (sender, receiver) = mpsc::channel();
    let worker = move || {
        let Some(dir) = cache_dir() else {
            log::warn!("Sin carpeta de caché: no hay listas de filtros");
            return;
        };
        let send = |rules: AdRules| {
            log::info!("Listas de filtros: {} reglas", rules.rule_count());
            if sender.send(rules).is_ok() {
                wake();
            }
        };

        let cached = read_lists(&dir);
        if !cached.is_empty() {
            send(AdRules::parse(cached.iter().map(String::as_str)));
        }

        let mut renewed = false;
        for list in &LISTS {
            let path = dir.join(list.file);
            if is_fresh(&path) {
                continue;
            }
            match download(list.url, proxy.as_deref()) {
                Ok(text) => match save(&dir, &path, &text) {
                    Ok(()) => {
                        log::info!("{} actualizada", list.name);
                        renewed = true;
                    },
                    Err(error) => log::warn!("No se pudo guardar {}: {error}", list.name),
                },
                Err(error) => log::warn!("No se pudo descargar {}: {error}", list.name),
            }
        }
        if renewed {
            let lists = read_lists(&dir);
            send(AdRules::parse(lists.iter().map(String::as_str)));
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("ferrum-listas".to_owned())
        .spawn(worker)
    {
        log::error!("No se pudo preparar las listas de filtros: {error}");
    }
    receiver
}

/// Carpeta de caché de Ferrum para las listas, según el sistema.
fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Ferrum")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/Ferrum")
    } else {
        match std::env::var_os("XDG_CACHE_HOME") {
            Some(cache) => PathBuf::from(cache).join("ferrum"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".cache/ferrum"),
        }
    };
    Some(base.join("listas"))
}

fn read_lists(dir: &Path) -> Vec<String> {
    LISTS
        .iter()
        .filter_map(|list| std::fs::read_to_string(dir.join(list.file)).ok())
        .filter(|text| looks_like_filter_list(text))
        .collect()
}

fn is_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < MAX_AGE)
}

fn download(url: &str, proxy: Option<&str>) -> Result<String, String> {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        // Una petición anónima: nada que identifique a quien la hace.
        .user_agent("Ferrum")
        .https_only(true);
    if let Some(proxy) = proxy {
        let proxy = ureq::Proxy::new(proxy).map_err(|error| error.to_string())?;
        config = config.proxy(Some(proxy));
    }
    let agent: ureq::Agent = config.build().into();
    let text = agent
        .get(url)
        .call()
        .map_err(|error| error.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_LIST_BYTES)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    if looks_like_filter_list(&text) {
        Ok(text)
    } else {
        Err("la respuesta no es una lista de filtros".to_owned())
    }
}

/// Las listas de Adblock empiezan con una cabecera `[Adblock Plus …]`.
fn looks_like_filter_list(text: &str) -> bool {
    text.trim_start().starts_with("[Adblock")
}

/// Guarda la lista sin dejarla a medias: primero en un archivo temporal.
fn save(dir: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension("descargando");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_filter_lists() {
        assert!(looks_like_filter_list("[Adblock Plus 2.0]\n||ads.example^"));
        assert!(!looks_like_filter_list("<html>Error</html>"));
    }

    #[test]
    fn saves_atomically_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("ferrum-listas-{}", std::process::id()));
        let path = dir.join(LISTS[0].file);
        save(&dir, &path, "[Adblock Plus 2.0]\n||ads.example^\n").expect("guardar");
        assert!(is_fresh(&path));
        assert_eq!(read_lists(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
