/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Páginas internas de Ferrum (inicio y avisos de bloqueo).
//!
//! Se sirven interceptando la carga, sin servidor ni red. Usan el dominio
//! reservado `.invalid` (RFC 6761), que nunca resuelve en DNS: si algo fallara,
//! la petición no saldría a ningún sitio real.

use ferrum_shield::BlockReason;
use ferrum_shield::omnibox::SearchEngine;
use url::Url;

pub const INTERNAL_HOST: &str = "ferrum.invalid";

/// Cabecera CSP de las páginas internas: sin scripts externos, sin conexiones.
pub const START_PAGE_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; form-action https:; base-uri 'none'";
pub const NOTICE_PAGE_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; base-uri 'none'";

pub fn start_url() -> Url {
    Url::parse(&format!("https://{INTERNAL_HOST}/")).expect("URL interna válida")
}

pub fn is_internal(url: &Url) -> bool {
    url.host_str() == Some(INTERNAL_HOST)
}

const STYLE: &str = r#"
:root { color-scheme: dark; --bg:#0f1115; --fg:#e8e6e3; --muted:#9aa0a6; --accent:#e0703a; --card:#181b22; }
* { box-sizing: border-box; }
body { margin:0; min-height:100vh; background:var(--bg); color:var(--fg);
       font-family: system-ui, "Segoe UI", sans-serif; display:flex; align-items:center; justify-content:center; }
main { width:min(680px, 92vw); padding:48px 0; }
h1 { font-size:44px; margin:0 0 4px; letter-spacing:-1px; }
h1 span { color:var(--accent); }
p { color:var(--muted); line-height:1.5; }
form { display:flex; gap:8px; margin:28px 0; }
input[type=search] { flex:1; font-size:18px; padding:14px 16px; border-radius:10px;
       border:1px solid #2c313c; background:var(--card); color:var(--fg); }
button, .button { font-size:16px; padding:0 20px; border-radius:10px; border:0; background:var(--accent);
       color:#111; font-weight:600; cursor:pointer; text-decoration:none; display:inline-flex; align-items:center; }
ul { list-style:none; padding:0; display:grid; grid-template-columns:1fr 1fr; gap:10px; }
li { background:var(--card); padding:12px 14px; border-radius:10px; font-size:14px; }
li b { display:block; margin-bottom:2px; }
kbd { background:#262a33; border-radius:4px; padding:1px 6px; font-size:12px; }
.small { font-size:13px; }
code { background:var(--card); padding:2px 6px; border-radius:6px; word-break:break-all; }
"#;

/// Página de inicio con buscador.
pub fn start_page(engine: &SearchEngine) -> String {
    let (action, param) = search_form_target(engine);
    let engine_name = engine.name;
    format!(
        r#"<!DOCTYPE html>
<html lang="es"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<title>Inicio</title><style>{STYLE}</style></head>
<body><main>
<h1>Ferrum<span>.</span></h1>
<p>Navegador ligero, seguro y privado. Nada de lo que hagas aquí se guarda al cerrar.</p>
<form action="{action}" method="get" role="search">
  <input type="search" name="{param}" placeholder="Buscar en {engine_name} o escribir una dirección" autofocus>
  <button type="submit">Buscar</button>
</form>
<ul>
  <li><b>🛡 Rastreadores bloqueados</b>Publicidad, analítica y huella digital de terceros.</li>
  <li><b>🔒 Solo HTTPS</b>Las conexiones sin cifrar se mejoran a cifradas.</li>
  <li><b>🧹 Enlaces limpios</b>Sin <code>utm_*</code>, <code>fbclid</code> ni <code>gclid</code>.</li>
  <li><b>🚫 Descargas peligrosas</b>Ejecutables y scripts bloqueados.</li>
  <li><b>📵 APIs de espionaje apagadas</b>WebRTC, geolocalización, notificaciones, Bluetooth.</li>
  <li><b>🗑 Sin rastro</b>Cookies, caché e historial solo en memoria.</li>
</ul>
<p class="small"><kbd>Ctrl</kbd>+<kbd>L</kbd> dirección · <kbd>Ctrl</kbd>+<kbd>T</kbd> nueva pestaña ·
<kbd>Ctrl</kbd>+<kbd>W</kbd> cerrar · <kbd>Alt</kbd>+<kbd>←</kbd> atrás · <kbd>F5</kbd> recargar</p>
</main></body></html>"#,
        action = html_escape(&action),
        param = html_escape(&param),
        engine_name = html_escape(engine_name),
    )
}

/// Aviso que sustituye a una página bloqueada.
pub fn block_page(url: &Url, reason: &BlockReason) -> String {
    let (title, explanation) = match reason {
        BlockReason::Malware { .. } => (
            "Sitio peligroso bloqueado",
            "Este dominio aparece en una lista de malware o phishing. Puede intentar robar \
             tus contraseñas o instalar software malicioso.",
        ),
        BlockReason::DangerousDownload { .. } => (
            "Descarga peligrosa bloqueada",
            "Este enlace descarga un archivo que puede ejecutar programas en tu equipo \
             (instaladores, scripts, accesos directos…). Es la forma más común de \
             infectar un ordenador. Si confías plenamente en la fuente, descárgalo \
             desde su web oficial con otra herramienta.",
        ),
        BlockReason::Tracker { .. } => (
            "Rastreador bloqueado",
            "Esta dirección pertenece a un servicio de rastreo o publicidad.",
        ),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="es"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<title>{title}</title><style>{STYLE}</style></head>
<body><main>
<h1>⛔ {title}</h1>
<p>{explanation}</p>
<p class="small">Motivo: <b>{code}</b> · <code>{detail}</code></p>
<p class="small">Dirección: <code>{url}</code></p>
<form onsubmit="history.back(); return false;"><button type="submit">← Volver</button></form>
</main></body></html>"#,
        code = reason.code(),
        detail = html_escape(reason.detail()),
        url = html_escape(url.as_str()),
    )
}

/// Página puente que salta a la URL limpia sin dejar la sucia en el historial.
pub fn redirect_page(target: &Url) -> String {
    let js_target = js_string_escape(target.as_str());
    let html_target = html_escape(target.as_str());
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<meta http-equiv="refresh" content="0;url={html_target}">
<script>location.replace("{js_target}");</script></head><body></body></html>"#
    )
}

/// Separa la plantilla del buscador en destino del formulario y nombre del campo.
fn search_form_target(engine: &SearchEngine) -> (String, String) {
    if let Ok(mut url) = Url::parse(engine.template) {
        let param = url
            .query_pairs()
            .find(|(_, value)| value == "{q}")
            .map(|(key, _)| key.into_owned());
        if let Some(param) = param {
            url.set_query(None);
            return (url.to_string(), param);
        }
    }
    ("https://html.duckduckgo.com/html/".into(), "q".into())
}

pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn js_string_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '<' => out.push_str("\\u003C"),
            '>' => out.push_str("\\u003E"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            html_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn escapes_js() {
        let escaped = js_string_escape(r#"a"b\c</script>"#);
        assert_eq!(escaped, "a\\\"b\\\\c\\u003C/script\\u003E");
        assert!(!escaped.contains('<') && !escaped.contains('>'));
    }

    #[test]
    fn search_form_from_template() {
        let (action, param) = search_form_target(&SearchEngine::DUCKDUCKGO);
        assert_eq!(action, "https://html.duckduckgo.com/html/");
        assert_eq!(param, "q");
    }

    #[test]
    fn internal_urls() {
        assert!(is_internal(&start_url()));
        assert!(!is_internal(
            &Url::parse("https://example.com/").expect("url")
        ));
    }

    #[test]
    fn block_page_escapes_url() {
        let url = Url::parse("https://evil.example/a?x=<script>").expect("url");
        let html = block_page(
            &url,
            &BlockReason::Malware {
                domain: "evil.example".into(),
            },
        );
        assert!(!html.contains("<script>"));
    }
}
