/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Scripts que Ferrum ejecuta en la página para el agente. Devuelven JSON en
//! texto y nunca modifican el DOM, salvo `select_option`, que hace lo mismo que
//! haría el usuario al elegir.
//!
//! Las coordenadas salen en píxeles del dispositivo, relativas a la página: las
//! mismas que la captura de pantalla y que los clics.

use serde_json::Value;

/// Llama a una función de JavaScript con argumentos convertidos a JSON (que es
/// JavaScript válido), sin interpolar texto del agente en el código.
pub fn call(function: &str, arguments: &[Value]) -> String {
    let arguments: Vec<String> = arguments.iter().map(Value::to_string).collect();
    format!("({function})({})", arguments.join(", "))
}

/// Título, dirección, texto y elementos con los que se puede interactuar.
/// Los campos de contraseña nunca devuelven su valor.
pub const READ_PAGE: &str = r#"function (maxChars) {
  const dpr = window.devicePixelRatio || 1;
  const vw = window.innerWidth, vh = window.innerHeight;
  const clean = (s) => String(s || '').replace(/\s+/g, ' ').trim();
  const typeOf = (el) => (el.getAttribute('type') || 'text').toLowerCase();
  const labelOf = (el) => {
    const aria = el.getAttribute('aria-label');
    if (aria) return clean(aria);
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
      const value = typeOf(el) === 'password' ? '' : el.value;
      return clean(value || el.placeholder || el.name || el.title);
    }
    if (el.tagName === 'SELECT') {
      const option = el.options[el.selectedIndex];
      return clean(option ? option.text : '');
    }
    const img = el.querySelector('img[alt]');
    return clean(el.innerText || el.title || (img ? img.alt : ''));
  };
  const kindOf = (el) => {
    const tag = el.tagName.toLowerCase();
    const role = el.getAttribute('role');
    if (tag === 'a' || role === 'link') return 'enlace';
    if (tag === 'button' || role === 'button') return 'botón';
    if (tag === 'select') return 'lista';
    if (tag === 'textarea') return 'campo de texto';
    if (tag === 'input') {
      const type = typeOf(el);
      if (['submit', 'button', 'reset', 'image'].includes(type)) return 'botón';
      if (type === 'checkbox') return el.checked ? 'casilla marcada' : 'casilla';
      if (type === 'radio') return el.checked ? 'opción marcada' : 'opción';
      return 'campo ' + type;
    }
    return role || tag;
  };
  const query = 'a[href], button, input:not([type=hidden]), select, textarea, summary, ' +
    '[role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], ' +
    '[contenteditable=""], [contenteditable=true]';
  const elements = [];
  let offscreen = 0;
  for (const el of document.querySelectorAll(query)) {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const style = getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none' || style.opacity === '0') continue;
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    if (cx < 0 || cy < 0 || cx > vw || cy > vh || elements.length >= 150) {
      offscreen++;
      continue;
    }
    elements.push([kindOf(el), labelOf(el).slice(0, 80), Math.round(cx * dpr), Math.round(cy * dpr)]);
  }
  const text = document.body ? document.body.innerText : '';
  const root = document.documentElement;
  return JSON.stringify({
    title: document.title,
    url: location.href,
    text: text.length > maxChars ? text.slice(0, maxChars) : text,
    truncated: text.length > maxChars,
    elements: elements,
    offscreen: offscreen,
    scroll: Math.round(window.scrollY * dpr),
    scroll_max: Math.round(Math.max(0, root.scrollHeight - vh) * dpr),
  });
}"#;

/// Busca un elemento por selector CSS o por su texto visible, lo trae a la
/// vista si hace falta y devuelve el centro donde hacer clic.
pub const FIND: &str = r#"function (selector, wanted) {
  const clean = (s) => String(s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const visible = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  let el = null;
  if (selector) {
    el = document.querySelector(selector);
  } else {
    const target = clean(wanted);
    const textOf = (e) => clean(e.innerText || e.value || e.getAttribute('aria-label') || e.title);
    const candidates = Array.from(document.querySelectorAll(
      'a, button, input[type=submit], input[type=button], summary, label, [role=button], ' +
      '[role=link], [role=tab], [role=menuitem], [role=option]')).filter(visible);
    el = candidates.find((e) => textOf(e) === target) ||
      candidates.find((e) => textOf(e).includes(target));
    if (!el) {
      // Cualquier elemento sin hijos que contenga ese texto.
      el = Array.from(document.body ? document.body.querySelectorAll('*') : [])
        .find((e) => e.children.length === 0 && visible(e) && clean(e.textContent).includes(target));
    }
  }
  if (!el) return JSON.stringify({ found: false });
  let r = el.getBoundingClientRect();
  const scrolled = r.top < 0 || r.left < 0 || r.bottom > innerHeight || r.right > innerWidth;
  if (scrolled) {
    el.scrollIntoView({ block: 'center', inline: 'center' });
    r = el.getBoundingClientRect();
  }
  const dpr = window.devicePixelRatio || 1;
  return JSON.stringify({
    found: true,
    tag: el.tagName.toLowerCase(),
    label: String(el.innerText || el.value || el.getAttribute('aria-label') || '')
      .replace(/\s+/g, ' ').trim().slice(0, 80),
    select: el.tagName === 'SELECT',
    scrolled: scrolled,
    x: Math.round((r.left + r.width / 2) * dpr),
    y: Math.round((r.top + r.height / 2) * dpr),
  });
}"#;

/// Elige una opción de un `<select>` por valor o texto y avisa a la página con
/// los mismos eventos que un usuario.
pub const SELECT_OPTION: &str = r#"function (selector, wanted) {
  const el = document.querySelector(selector);
  if (!el) return JSON.stringify({ error: 'No encontré ningún elemento con ' + selector });
  if (el.tagName !== 'SELECT') return JSON.stringify({ error: 'El elemento no es una lista (<select>)' });
  const clean = (s) => String(s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const target = clean(wanted);
  const options = Array.from(el.options);
  const option = options.find((o) => o.value === wanted) ||
    options.find((o) => clean(o.text) === target) ||
    options.find((o) => clean(o.text).includes(target));
  if (!option) {
    return JSON.stringify({
      error: 'No hay ninguna opción así',
      options: options.slice(0, 50).map((o) => o.text.trim()),
    });
  }
  el.selectedIndex = option.index;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return JSON.stringify({ chosen: option.text.trim() });
}"#;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn arguments_are_json_not_code() {
        let script = call(FIND, &[json!("a\"); alert(1); (\""), Value::Null]);
        assert!(script.ends_with(r#"("a\"); alert(1); (\"", null)"#));
    }
}
