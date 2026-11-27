/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Ferrum: anuncios de vídeo de YouTube.
//
// YouTube no pide sus anuncios de vídeo a otra dirección: vienen en la misma
// respuesta que el vídeo (`adPlacements`, `playerAds`…), así que bloquear por
// dirección no sirve. Este script quita esas entradas antes de que el
// reproductor las lea:
// - en `ytInitialPlayerResponse`, la respuesta que trae la propia página;
// - en cada `JSON.parse` y `Response.json()`, las que llegan al navegar.
(() => {
  "use strict";

  const host = location.hostname;
  if (host !== "youtube.com" && !host.endsWith(".youtube.com")) {
    return;
  }

  const AD_KEYS = ["adPlacements", "adSlots", "playerAds", "adBreakHeartbeatParams"];

  const prune = (value) => {
    if (value !== null && typeof value === "object") {
      for (const key of AD_KEYS) {
        if (key in value) {
          delete value[key];
        }
      }
      if (value.playerResponse !== null && typeof value.playerResponse === "object") {
        prune(value.playerResponse);
      }
    }
    return value;
  };

  let initial;
  try {
    Object.defineProperty(window, "ytInitialPlayerResponse", {
      configurable: true,
      get: () => initial,
      set: (value) => {
        initial = prune(value);
      },
    });
  } catch (_) {
    /* ya definida: se deja como está */
  }

  const parse = JSON.parse;
  JSON.parse = function (text, reviver) {
    return prune(parse.call(this, text, reviver));
  };

  const json = Response.prototype.json;
  Response.prototype.json = function () {
    return json.call(this).then(prune);
  };
})();
