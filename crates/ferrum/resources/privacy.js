/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Ferrum: protecciones contra la huella digital (fingerprinting).
//
// Idea: que todos los usuarios de Ferrum se vean iguales. Solo se tocan APIs
// que casi ninguna web necesita de verdad, para no romper sitios.
(() => {
  "use strict";

  const fixed = (proto, prop, getter) => {
    if (!proto || !(prop in proto)) return;
    try {
      Object.defineProperty(proto, prop, { get: getter, configurable: true, enumerable: true });
    } catch (_) {
      /* propiedad no configurable: se deja como está */
    }
  };

  // Hardware: número de núcleos y memoria iguales para todos.
  if (typeof Navigator !== "undefined") {
    fixed(Navigator.prototype, "hardwareConcurrency", () => 4);
    fixed(Navigator.prototype, "deviceMemory", () => 8);
    // API de batería: identificaba usuarios por su nivel de carga.
    if ("getBattery" in Navigator.prototype) {
      try { delete Navigator.prototype.getBattery; } catch (_) {}
    }
    // Global Privacy Control: señal legal de "no vendas mis datos".
    try {
      Object.defineProperty(Navigator.prototype, "globalPrivacyControl", {
        get: () => true, configurable: true, enumerable: true,
      });
    } catch (_) {}
  }

  // Pantalla: se informa el tamaño de la ventana redondeado, no el del monitor.
  if (typeof Screen !== "undefined") {
    const round = (value, step) => Math.max(step, Math.round(value / step) * step);
    const w = () => round(window.innerWidth, 200);
    const h = () => round(window.innerHeight, 100);
    fixed(Screen.prototype, "width", w);
    fixed(Screen.prototype, "height", h);
    fixed(Screen.prototype, "availWidth", w);
    fixed(Screen.prototype, "availHeight", h);
    fixed(Screen.prototype, "colorDepth", () => 24);
    fixed(Screen.prototype, "pixelDepth", () => 24);
  }
})();
