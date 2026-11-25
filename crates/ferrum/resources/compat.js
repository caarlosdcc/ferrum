/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Ferrum: arreglos de compatibilidad para sitios concretos, como las
// "intervenciones" de Firefox. Cada uno dice qué arregla y por qué no se
// aplica a toda la web. Cuando Servo implemente lo que falta, se borran.
(() => {
  "use strict";

  const host = location.hostname;
  const on = (domain) => host === domain || host.endsWith("." + domain);

  // YouTube: su filtro de seguridad (polymer-resin) deja pasar los valores
  // vacíos con `valor || valor === document.all`. Servo aún no tiene
  // `document.all`, así que para `undefined` la comparación es cierta: el
  // filtro cambia el valor por "zClosurez" y la lista de resultados queda
  // oculta (`hidden`). En los navegadores, `document.all` es un objeto que se
  // comporta como `false`, algo que JavaScript no puede imitar; por eso solo se
  // define aquí. En otras webs, un `document.all` verdadero haría creer a código
  // antiguo que el navegador es Internet Explorer.
  if (on("youtube.com") && !("all" in document)) {
    Object.defineProperty(Document.prototype, "all", {
      configurable: true,
      enumerable: true,
      get() {
        return this.getElementsByTagName("*");
      },
    });
  }
})();
