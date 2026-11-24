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
:root { color-scheme: dark; --bg:#0e1014; --fg:#ecebe8; --muted:#9299a3; --faint:#5c636d;
        --accent:#e0703a; --card:#161920; --border:#252a33; --green:#5cc489; }
* { box-sizing: border-box; }
html { background: var(--bg); }
body { margin:0; min-height:100vh; color:var(--fg);
       background: radial-gradient(900px 480px at 50% -8%, rgba(224,112,58,.13), transparent 70%), var(--bg);
       font: 15px/1.5 system-ui, "Segoe UI", sans-serif; display:flex; justify-content:center; }
main { width:min(720px, 90vw); padding:12vh 0 48px; }
.brand { display:flex; align-items:center; gap:16px; justify-content:center; }
.brand svg { width:56px; height:56px; }
h1 { font-size:46px; font-weight:700; letter-spacing:-1.5px; margin:0; }
.tagline { text-align:center; color:var(--muted); margin:10px 0 34px; }
form.search { display:flex; align-items:center; gap:10px; background:#0b0d11; border:1px solid var(--border);
       border-radius:999px; padding:6px 6px 6px 20px; }
form.search:focus-within { border-color: var(--accent); box-shadow: 0 0 0 4px rgba(224,112,58,.15); }
form.search svg { width:18px; height:18px; color:var(--muted); flex:none; }
input[type=search] { flex:1; min-width:0; font:inherit; font-size:17px; padding:10px 0; border:0; outline:0;
       background:transparent; color:var(--fg); }
input[type=search]::placeholder { color:var(--faint); }
button.go { flex:none; width:42px; height:42px; border-radius:50%; border:0; background:var(--accent);
       cursor:pointer; display:flex; align-items:center; justify-content:center; padding:0; }
button.go:hover { background:#ea8550; }
button.go svg { color:#14100c; width:18px; height:18px; }
.stat { display:flex; align-items:center; justify-content:center; gap:8px; margin:18px 0 46px;
        color:var(--muted); font-size:14px; }
.stat b { color:var(--accent); font-weight:600; }
.stat svg { width:16px; height:16px; color:var(--accent); flex:none; }
ul.features { list-style:none; padding:0; margin:0; display:grid; grid-template-columns:repeat(3, 1fr); gap:12px; }
ul.features li { background:var(--card); border:1px solid var(--border); border-radius:14px; padding:14px 16px;
       font-size:13px; color:var(--muted); }
ul.features li svg { width:20px; height:20px; color:var(--accent); display:block; margin-bottom:8px; }
ul.features li b { display:block; color:var(--fg); font-size:14px; font-weight:600; margin-bottom:2px; }
code { font: 12px ui-monospace, Consolas, monospace; background:#0b0d11; border:1px solid var(--border);
       padding:1px 5px; border-radius:5px; }
footer { margin-top:40px; text-align:center; color:var(--faint); font-size:12.5px; }
kbd { font: 11.5px ui-monospace, Consolas, monospace; color:var(--muted); background:#1b1f27;
      border:1px solid var(--border); border-bottom-width:2px; border-radius:5px; padding:1px 6px; }
main.notice { text-align:center; padding-top:16vh; }
.badge { width:76px; height:76px; border-radius:50%; display:flex; align-items:center; justify-content:center;
         margin:0 auto 22px; }
.badge svg { width:38px; height:38px; }
.badge.danger { background:rgba(229,83,75,.14); color:#ef6b63; }
.badge.warn { background:rgba(240,180,76,.14); color:#f0b44c; }
.notice h1 { font-size:30px; letter-spacing:-.5px; }
.notice p { color:var(--muted); max-width:560px; margin:12px auto; }
.details { margin:26px auto; max-width:560px; text-align:left; background:var(--card); border:1px solid var(--border);
           border-radius:12px; padding:12px 16px; font-size:13px; color:var(--muted); }
.details div { display:flex; gap:10px; padding:4px 0; }
.details div > span:first-child { flex:none; width:80px; color:var(--faint); }
.details code { word-break:break-all; }
button.back { font:inherit; font-weight:600; color:#14100c; background:var(--accent); border:0;
              border-radius:999px; padding:10px 22px; cursor:pointer; }
button.back:hover { background:#ea8550; }
@media (max-width: 640px) { ul.features { grid-template-columns:1fr 1fr; } }
"#;

/// Logotipo: la misma geometría que `logo.rs` (hexágono óxido con "F").
const LOGO: &str = r##"<svg viewBox="0 0 100 100" aria-hidden="true"><defs><linearGradient id="fe" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f28c50"/><stop offset="1" stop-color="#c4501d"/></linearGradient></defs><polygon points="50,3 90.7,26.5 90.7,73.5 50,97 9.3,73.5 9.3,26.5" fill="url(#fe)"/><g fill="#15171c"><rect x="36" y="27" width="11" height="46"/><rect x="36" y="27" width="30" height="10"/><rect x="36" y="45.5" width="24" height="9"/></g></svg>"##;

/// Icono de trazo para las páginas internas (24×24, `currentColor`).
fn icon(path: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{path}</svg>"#
    )
}

const ICON_SEARCH: &str = r#"<circle cx="11" cy="11" r="6.5"/><path d="M16 16l4.5 4.5"/>"#;
const ICON_ARROW: &str = r#"<path d="M5 12h13M13 6l6 6-6 6"/>"#;
const ICON_SHIELD: &str = r#"<path d="M12 3l7.5 3v5.5c0 4.4-3.1 7.9-7.5 9.5-4.4-1.6-7.5-5.1-7.5-9.5V6z"/><path d="M8.8 12.2l2.2 2.2 4.3-4.6"/>"#;
const ICON_LOCK: &str = r#"<rect x="5" y="10.5" width="14" height="9.5" rx="2.2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>"#;
const ICON_LINK: &str = r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#;
const ICON_BLOCK: &str = r#"<circle cx="12" cy="12" r="8.5"/><path d="M6 6l12 12"/>"#;
const ICON_EYE_OFF: &str = r#"<path d="M3.5 3.5l17 17"/><path d="M10.4 5.6A9 9 0 0 1 12 5.5c5 0 8.5 6.5 8.5 6.5a16 16 0 0 1-2.8 3.5"/><path d="M6.6 7.1C4.6 8.7 3.5 12 3.5 12S7 18.5 12 18.5a8 8 0 0 0 3.7-.9"/>"#;
const ICON_TRASH: &str =
    r#"<path d="M4.5 7h15M10 11v5.5M14 11v5.5M6.5 7l1 12.5h9l1-12.5M9.5 7V4.5h5V7"/>"#;
const ICON_DOWNLOAD: &str = r#"<path d="M12 4v10M7.5 9.5L12 14l4.5-4.5"/><path d="M5 19.5h14"/>"#;

/// Página de inicio con buscador.
pub fn start_page(engine: &SearchEngine, blocked: u64) -> String {
    let (action, param) = search_form_target(engine);
    let engine_name = engine.name;
    let stat = match blocked {
        0 => "Escudo activo: todavía no ha hecho falta bloquear nada".to_owned(),
        1 => "<b>1</b> rastreador o anuncio bloqueado en esta sesión".to_owned(),
        n => format!("<b>{n}</b> rastreadores y anuncios bloqueados en esta sesión"),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="es"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Nueva pestaña</title><style>{STYLE}</style></head>
<body><main>
<div class="brand">{LOGO}<h1>Ferrum</h1></div>
<p class="tagline">Navegador ligero, seguro y privado. Nada de lo que hagas aquí se guarda al cerrar.</p>
<form class="search" action="{action}" method="get" role="search">
  {search_icon}
  <input type="search" name="{param}" placeholder="Buscar en {engine_name} o escribir una dirección" aria-label="Buscar" autofocus>
  <button class="go" type="submit" aria-label="Buscar">{arrow_icon}</button>
</form>
<p class="stat">{shield_icon}<span>{stat}</span></p>
<ul class="features">
  <li>{shield_icon}<b>Sin rastreadores</b>Publicidad, analítica y huella digital de terceros.</li>
  <li>{lock_icon}<b>Solo HTTPS</b>Las conexiones sin cifrar se mejoran a cifradas.</li>
  <li>{link_icon}<b>Enlaces limpios</b>Sin <code>utm_*</code>, <code>fbclid</code> ni <code>gclid</code>.</li>
  <li>{download_icon}<b>Descargas peligrosas</b>Ejecutables y scripts, bloqueados.</li>
  <li>{eye_icon}<b>APIs espía apagadas</b>WebRTC, ubicación, notificaciones.</li>
  <li>{trash_icon}<b>Sin rastro</b>Cookies, caché e historial solo en memoria.</li>
</ul>
<footer><kbd>Ctrl</kbd> <kbd>L</kbd> dirección &nbsp;·&nbsp; <kbd>Ctrl</kbd> <kbd>T</kbd> nueva pestaña &nbsp;·&nbsp;
<kbd>Ctrl</kbd> <kbd>W</kbd> cerrar &nbsp;·&nbsp; <kbd>Alt</kbd> <kbd>←</kbd> atrás</footer>
</main></body></html>"#,
        action = html_escape(&action),
        param = html_escape(&param),
        engine_name = html_escape(engine_name),
        search_icon = icon(ICON_SEARCH),
        arrow_icon = icon(ICON_ARROW),
        shield_icon = icon(ICON_SHIELD),
        lock_icon = icon(ICON_LOCK),
        link_icon = icon(ICON_LINK),
        download_icon = icon(ICON_DOWNLOAD),
        eye_icon = icon(ICON_EYE_OFF),
        trash_icon = icon(ICON_TRASH),
    )
}

/// Aviso que sustituye a una página bloqueada.
pub fn block_page(url: &Url, reason: &BlockReason) -> String {
    let (title, explanation, badge, badge_icon) = match reason {
        BlockReason::Malware { .. } => (
            "Sitio peligroso bloqueado",
            "Este dominio aparece en una lista de malware o phishing. Puede intentar robar \
             tus contraseñas o instalar software malicioso.",
            "danger",
            ICON_SHIELD,
        ),
        BlockReason::DangerousDownload { .. } => (
            "Descarga peligrosa bloqueada",
            "Este enlace descarga un archivo que puede ejecutar programas en tu equipo \
             (instaladores, scripts, accesos directos…). Es la forma más común de \
             infectar un ordenador. Si confías plenamente en la fuente, descárgalo \
             desde su web oficial con otra herramienta.",
            "warn",
            ICON_BLOCK,
        ),
        BlockReason::Tracker { .. } => (
            "Rastreador bloqueado",
            "Esta dirección pertenece a un servicio de rastreo o publicidad.",
            "warn",
            ICON_EYE_OFF,
        ),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="es"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title><style>{STYLE}</style></head>
<body><main class="notice">
<div class="badge {badge}">{icon}</div>
<h1>{title}</h1>
<p>{explanation}</p>
<div class="details">
  <div><span>Motivo</span><span class="value"><b>{code}</b> · <code>{detail}</code></span></div>
  <div><span>Dirección</span><code>{url}</code></div>
</div>
<form onsubmit="history.back(); return false;"><button class="back" type="submit">← Volver a un lugar seguro</button></form>
</main></body></html>"#,
        icon = icon(badge_icon),
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
