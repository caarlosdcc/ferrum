/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Configuración endurecida del motor Servo.
//!
//! Principio: **todo lo que no hace falta para leer la web, apagado**. Cada API
//! desactivada es código que un atacante ya no puede usar y una señal menos
//! para identificarte (fingerprinting).

use servo::{Opts, Preferences, UserAgentPlatform};

use crate::config::Config;

/// User-Agent: el estándar de Servo, igual en cada sistema para todos los
/// navegadores basados en Servo (`… rv:153.0) Servo/0.8.0 Firefox/153.0`).
///
/// Ferrum se presentaba antes como Firefox, sin el token `Servo/`, para no
/// destacar. Pero el motor se distingue igual por cómo se comporta su
/// JavaScript, y el inicio de sesión de Google rechaza a quien dice ser Firefox
/// y no lo parece («No se ha podido iniciar sesión: es posible que el navegador
/// no sea seguro»). Con la identidad honesta, Google pide como mucho un CAPTCHA.
fn user_agent() -> String {
    UserAgentPlatform::Desktop.to_user_agent_string()
}

// Preferences tiene más de 160 campos y varios dependen de la configuración:
// partir de los valores por defecto y ajustar es más legible.
#[allow(clippy::field_reassign_with_default)]
pub fn preferences(config: &Config) -> Preferences {
    let mut prefs = Preferences::default();

    // --- Red ---
    // Modo solo-HTTPS: cualquier petición http:// se mejora a https:// dentro
    // del propio motor (también imágenes, scripts e iframes).
    prefs.network_enforce_tls_enabled = true;
    prefs.network_enforce_tls_localhost = false; // desarrollo local sin TLS
    prefs.network_enforce_tls_onion = false; // .onion ya va cifrado por Tor
    // Nada se escribe en disco: sin caché persistente que revele qué visitaste.
    prefs.network_http_disk_cache = String::new();
    // Caché en memoria moderada: buen rendimiento sin disparar la RAM.
    prefs.network_http_cache_size = 2_000;
    if let Some(proxy) = &config.proxy {
        prefs.network_http_proxy_uri = proxy.clone();
        prefs.network_https_proxy_uri = proxy.clone();
    }

    // --- Identidad ---
    prefs.user_agent = user_agent();

    // --- APIs con riesgo de privacidad o de seguridad ---
    prefs.dom_webrtc_enabled = false; // filtra tu IP real incluso tras una VPN
    prefs.dom_webrtc_transceiver_enabled = false;
    prefs.dom_geolocation_enabled = false;
    prefs.dom_notification_enabled = false; // vía habitual de spam y estafas
    prefs.dom_bluetooth_enabled = false;
    prefs.dom_gamepad_enabled = false; // se usa para fingerprinting
    prefs.dom_webgpu_enabled = false; // acceso directo a la GPU
    prefs.dom_webgl2_enabled = false;
    prefs.dom_webxr_enabled = false;
    prefs.dom_serviceworker_enabled = false; // código que sigue vivo al cerrar la pestaña
    prefs.dom_async_clipboard_enabled = false; // que una web no lea tu portapapeles
    prefs.dom_storage_manager_api_enabled = false;
    prefs.dom_cookiestore_enabled = false;
    prefs.devtools_server_enabled = false; // nunca abrir un puerto de depuración

    // --- Compatibilidad ---
    // APIs estándar que Servo aún trae apagadas por ser experimentales, pero sin
    // las que muchas webs no funcionan. Ninguna da acceso nuevo al equipo ni
    // ayuda a identificarte.
    // IntersectionObserver: carga diferida de imágenes y listas (YouTube no
    // muestra resultados sin ella).
    prefs.dom_intersection_observer_enabled = true;

    // --- Modo estricto ---
    if config.strict {
        // Los compiladores JIT de JavaScript concentran la mayoría de exploits
        // de navegador. Sin JIT, JavaScript va más lento pero es mucho más difícil
        // de explotar (misma idea que el "modo súper seguro" de otros navegadores).
        prefs.js_disable_jit = true;
        prefs.js_baseline_jit_enabled = false;
        prefs.js_wasm_enabled = false;
        prefs.js_wasm_baseline_enabled = false;
        prefs.js_wasm_ion_enabled = false;
        prefs.dom_indexeddb_enabled = false;
        prefs.network_http_cache_size = 500;
    }

    // --- Memoria ---
    // Historial de "atrás/adelante" corto en memoria: menos páginas vivas.
    prefs.session_history_max_length = if config.strict { 10 } else { 20 };

    prefs
}

pub fn opts(config: &Config) -> Opts {
    Opts {
        // Nada persiste al cerrar: cookies, localStorage e IndexedDB viven en
        // almacenamiento temporal que se borra al salir.
        temporary_storage: true,
        config_dir: None,
        multiprocess: config.multiprocess,
        // El sandbox de procesos de Servo existe en Linux y macOS. En Windows el
        // aislamiento es por proceso, sin sandbox de privilegios (ver SECURITY.md).
        sandbox: config.multiprocess && sandbox_supported(),
        ..Opts::default()
    }
}

/// Plataformas donde Servo puede encerrar el contenido en un sandbox.
pub fn sandbox_supported() -> bool {
    cfg!(any(
        all(target_os = "linux", target_arch = "x86_64"),
        target_os = "macos"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_is_servo_standard() {
        let agent = user_agent();
        assert!(agent.starts_with("Mozilla/5.0 ("));
        assert!(agent.contains(" Servo/") && agent.contains(" Firefox/"));
    }

    #[test]
    fn common_web_apis_are_on() {
        let prefs = preferences(&Config::default());
        assert!(prefs.dom_intersection_observer_enabled);
        assert!(!prefs.dom_webrtc_enabled && !prefs.dom_geolocation_enabled);
    }
}
