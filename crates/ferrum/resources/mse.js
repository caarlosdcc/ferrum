/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Ferrum: Media Source Extensions (MSE) para Servo, que aún no las tiene.
//
// Da `MediaSource` y `SourceBuffer`. Lo que la web añade (vídeo y audio por
// separado, en MP4 fragmentado) se guarda para Ferrum, que lo une en un solo
// MP4 y lo sirve en una dirección interna que se pone en el `<video>`. Solo
// admite MP4 con H.264 y AAC: así las webs eligen formatos que GStreamer y
// esta unión saben tratar. Experimental: solo con `ferrum --mse`.
//
// Ferrum guarda lo que la página añade, como haría el navegador, hasta que
// la página lo quita (`remove()`) o, si ocupa demasiado, lo tira y lo dice al
// recoger (`drain`).
//
// Saltos: el reproductor de Servo no puede saltar dentro de un flujo que no
// termina. Cuando la página cambia `currentTime`, se pide a Ferrum un flujo
// nuevo que empiece ahí (desde el fotograma clave anterior) y se pone en el
// `<video>`. La página no ve esa recarga: ve `seeking` y, cuando hay imagen,
// `seeked`, como en cualquier navegador. Cada flujo empieza en el segundo 0 y
// Ferrum dice a qué segundo del vídeo corresponde (`offset`).
(() => {
  "use strict";

  if (typeof window.MediaSource !== "undefined") {
    return;
  }

  const STREAM = "https://ferrum.invalid/mse/";
  const outbox = [];
  const sources = [];
  const byObjectUrl = new Map();
  const objectUrlOf = new WeakMap();
  // `<video>` → su MediaSource.
  const sourceOf = new WeakMap();
  // Eventos que lanza este script (los demás se callan durante un salto).
  const ours = new WeakSet();
  const media = HTMLMediaElement.prototype;
  const native = (proto, name) => Object.getOwnPropertyDescriptor(proto, name) || {};

  const token = () =>
    Array.from(crypto.getRandomValues(new Uint8Array(12)), (b) => b.toString(16).padStart(2, "0")).join("");

  const later = (task) => setTimeout(task, 0);

  const fire = (target, type) => {
    const event = new Event(type);
    ours.add(event);
    target.dispatchEvent(event);
    // En los elementos, el navegador ya llama a `on<tipo>`; aquí no.
    const handler = target["on" + type];
    if (!(target instanceof Node) && typeof handler === "function") {
      handler.call(target, event);
    }
  };

  const error = (name, message) => new DOMException(message || name, name);

  const toBase64 = (bytes) => {
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) {
      text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(text);
  };

  const toBytes = (data) => {
    if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
    if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    throw new TypeError("appendBuffer necesita un ArrayBuffer o una vista");
  };

  // --- MP4 mínimo: escalas de tiempo y duración de cada fragmento ---------

  const readBoxes = (bytes, start, end) => {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const boxes = [];
    let at = start;
    while (at + 8 <= end) {
      let size = view.getUint32(at);
      let header = 8;
      if (size === 1) {
        if (at + 16 > end) break;
        size = Number(view.getBigUint64(at + 8));
        header = 16;
      } else if (size === 0) {
        size = end - at;
      }
      if (size < header || at + size > end) break;
      const type = String.fromCharCode(bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]);
      boxes.push({ type, start: at, body: at + header, end: at + size });
      at += size;
    }
    return { boxes, used: at };
  };

  const child = (bytes, box, type) => readBoxes(bytes, box.body, box.end).boxes.find((b) => b.type === type);

  const parseInit = (bytes, moov) => {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const trak = child(bytes, moov, "trak");
    const mdhd = trak && child(bytes, child(bytes, trak, "mdia") || trak, "mdhd");
    if (!mdhd) return null;
    const timescale = view.getUint32(mdhd.body + (bytes[mdhd.body] === 1 ? 20 : 12)) || 1;
    const mvex = child(bytes, moov, "mvex");
    const trex = mvex && child(bytes, mvex, "trex");
    return { timescale, defaultDuration: trex ? view.getUint32(trex.body + 12) : 0 };
  };

  const fragmentSpan = (bytes, moof, track) => {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const traf = child(bytes, moof, "traf");
    const tfhd = traf && child(bytes, traf, "tfhd");
    const tfdt = traf && child(bytes, traf, "tfdt");
    if (!tfhd || !tfdt) return null;
    const tfhdFlags = view.getUint32(tfhd.body) & 0xffffff;
    let at = tfhd.body + 8 + (tfhdFlags & 0x01 ? 8 : 0) + (tfhdFlags & 0x02 ? 4 : 0);
    const defaultDuration = tfhdFlags & 0x08 ? view.getUint32(at) : track.defaultDuration;
    const base = bytes[tfdt.body] === 1 ? Number(view.getBigUint64(tfdt.body + 4)) : view.getUint32(tfdt.body + 4);
    let total = 0;
    for (const trun of readBoxes(bytes, traf.body, traf.end).boxes.filter((b) => b.type === "trun")) {
      const flags = view.getUint32(trun.body) & 0xffffff;
      const count = view.getUint32(trun.body + 4);
      at = trun.body + 8 + (flags & 0x001 ? 4 : 0) + (flags & 0x004 ? 4 : 0);
      const step = [0x100, 0x200, 0x400, 0x800].filter((f) => flags & f).length * 4;
      for (let i = 0; i < count; i++) {
        total += flags & 0x100 ? view.getUint32(at) : defaultDuration;
        at += step;
      }
    }
    return [base / track.timescale, (base + total) / track.timescale];
  };

  // --- Rangos de tiempo -----------------------------------------------------

  class FerrumTimeRanges {
    constructor(ranges) {
      this._ranges = ranges.map((r) => r.slice());
    }
    get length() {
      return this._ranges.length;
    }
    start(index) {
      if (index < 0 || index >= this._ranges.length) throw error("IndexSizeError");
      return this._ranges[index][0];
    }
    end(index) {
      if (index < 0 || index >= this._ranges.length) throw error("IndexSizeError");
      return this._ranges[index][1];
    }
  }

  const addRange = (ranges, start, end) => {
    const all = ranges.concat([[start, end]]).sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const range of all) {
      const last = merged[merged.length - 1];
      // Huecos de menos de 0,1 s se dan por continuos, como en los navegadores.
      if (last && range[0] <= last[1] + 0.1) {
        last[1] = Math.max(last[1], range[1]);
      } else {
        merged.push(range.slice());
      }
    }
    return merged;
  };

  const removeRange = (ranges, start, end) => {
    const out = [];
    for (const [a, b] of ranges) {
      if (b <= start || a >= end) {
        out.push([a, b]);
        continue;
      }
      if (a < start) out.push([a, start]);
      if (b > end) out.push([end, b]);
    }
    return out;
  };

  // --- SourceBuffer -------------------------------------------------------

  class SourceBufferList extends EventTarget {
    constructor() {
      super();
      this._items = [];
    }
    get length() {
      return this._items.length;
    }
    [Symbol.iterator]() {
      return this._items[Symbol.iterator]();
    }
  }

  class SourceBuffer extends EventTarget {
    constructor(source, index, type) {
      super();
      this._source = source;
      this._index = index;
      this._type = type;
      this._updating = false;
      this._ranges = [];
      this._pending = new Uint8Array(0);
      this._track = null;
      this._appended = 0;
      this._appends = 0;
      this.mode = "segments";
      this.timestampOffset = 0;
      this.appendWindowStart = 0;
      this.appendWindowEnd = Infinity;
    }

    get updating() {
      return this._updating;
    }

    get buffered() {
      return new FerrumTimeRanges(this._ranges);
    }

    _parse(bytes) {
      const all = new Uint8Array(this._pending.length + bytes.length);
      all.set(this._pending);
      all.set(bytes, this._pending.length);
      const { boxes, used } = readBoxes(all, 0, all.length);
      for (const box of boxes) {
        if (box.type === "moov") {
          this._track = parseInit(all, box) || this._track;
        } else if (box.type === "moof" && this._track) {
          const span = fragmentSpan(all, box, this._track);
          if (span && span[1] > span[0]) this._ranges = addRange(this._ranges, span[0], span[1]);
        }
      }
      this._pending = all.slice(used);
    }

    _operation(work) {
      if (this._updating) throw error("InvalidStateError", "El SourceBuffer está ocupado");
      if (this._source.readyState === "closed") throw error("InvalidStateError", "MediaSource cerrado");
      if (this._source.readyState === "ended") {
        this._source._ready = "open";
        fire(this._source, "sourceopen");
      }
      work();
      this._updating = true;
      fire(this, "updatestart");
      later(() => {
        if (!this._updating) return;
        this._updating = false;
        fire(this, "update");
        fire(this, "updateend");
      });
    }

    appendBuffer(data) {
      const bytes = toBytes(data);
      this._appended += bytes.length;
      this._appends += 1;
      this._operation(() => {
        this._parse(bytes);
        outbox.push({ t: "data", s: this._source._token, i: this._index, d: toBase64(bytes) });
      });
    }

    remove(start, end) {
      const from = Number(start);
      const to = Number(end);
      this._operation(() => {
        this._ranges = removeRange(this._ranges, from, to);
        outbox.push({ t: "remove", s: this._source._token, i: this._index, a: from, b: Number.isFinite(to) ? to : 1e9 });
      });
    }

    abort() {
      if (this._updating) {
        this._updating = false;
        fire(this, "abort");
        fire(this, "updateend");
      }
      // Lo que quedaba a medias se tira, aquí y en Ferrum.
      this._pending = new Uint8Array(0);
      outbox.push({ t: "reset", s: this._source._token, i: this._index });
    }

    changeType(type) {
      if (!MediaSource.isTypeSupported(type)) throw error("NotSupportedError", "Tipo no admitido: " + type);
      this._type = type;
    }
  }

  // --- MediaSource --------------------------------------------------------

  class MediaSource extends EventTarget {
    constructor() {
      super();
      this._token = token();
      // Todos los nombres que ha tenido su flujo (cambia en cada salto) y en
      // qué segundo del vídeo empieza cada uno.
      this._tokens = new Set([this._token]);
      this._offsets = new Map();
      this._ready = "closed";
      this._duration = NaN;
      this._buffers = new SourceBufferList();
      this._element = null;
      this._events = [];
      // Salto en curso (cambiando de flujo) y a qué segundo.
      this._swapping = false;
      this._seekTarget = null;
      // Cómo estaba el `<video>` antes del salto, para dejarlo igual.
      this._wasPaused = true;
      this._rate = 1;
      this._size = [0, 0];
      // `play()` pedidos durante el salto, y eventos propios que hay que callar.
      this._waiters = [];
      this._mute = new Set();
      // Empujones dados al reproductor desde el último salto (ver `_watch`).
      this._nudges = 0;
      this._watching = false;
      sources.push(this);
    }

    static isTypeSupported(type) {
      const text = String(type).toLowerCase();
      if (!/^(video|audio)\/mp4\b/.test(text)) return false;
      const codecs = (text.match(/codecs\s*=\s*"?([^";]+)/) || [])[1];
      if (!codecs) return true;
      return codecs.split(",").every((codec) => /^(avc1|avc3|mp4a\.40)\./.test(codec.trim()));
    }

    get readyState() {
      return this._ready;
    }

    get sourceBuffers() {
      return this._buffers;
    }

    get activeSourceBuffers() {
      return this._buffers;
    }

    get duration() {
      return this._duration;
    }

    set duration(value) {
      const seconds = Number(value);
      if (Number.isNaN(seconds) || seconds < 0) throw new TypeError("Duración no válida");
      if (this._ready !== "open") throw error("InvalidStateError");
      if (seconds === this._duration) return;
      this._duration = seconds;
      outbox.push({ t: "duration", s: this._token, v: Number.isFinite(seconds) ? seconds : null });
      later(() => {
        if (this._element) fire(this._element, "durationchange");
      });
    }

    addSourceBuffer(type) {
      if (!MediaSource.isTypeSupported(type)) throw error("NotSupportedError", "Tipo no admitido: " + type);
      if (this._ready !== "open") throw error("InvalidStateError", "MediaSource no está abierto");
      const index = this._buffers._items.length;
      const buffer = new SourceBuffer(this, index, type);
      this._buffers._items.push(buffer);
      outbox.push({ t: "add", s: this._token, i: index });
      later(() => fire(this._buffers, "addsourcebuffer"));
      return buffer;
    }

    removeSourceBuffer(buffer) {
      const at = this._buffers._items.indexOf(buffer);
      if (at < 0) throw error("NotFoundError");
      buffer.abort();
      this._buffers._items.splice(at, 1);
      later(() => fire(this._buffers, "removesourcebuffer"));
    }

    endOfStream() {
      if (this._ready !== "open") throw error("InvalidStateError");
      this._ready = "ended";
      outbox.push({ t: "end", s: this._token });
      later(() => fire(this, "sourceended"));
    }

    setLiveSeekableRange() {}

    clearLiveSeekableRange() {}

    _attach(element) {
      this._element = element;
      sourceOf.set(element, this);
      this._ready = "open";
      later(() => fire(this, "sourceopen"));
    }

    // El `<video>` pasa a otra fuente: este MediaSource se cierra y Ferrum
    // suelta su flujo.
    _detach() {
      if (this._element && sourceOf.get(this._element) === this) sourceOf.delete(this._element);
      this._element = null;
      this._release();
      // Ya no se puede volver a poner: se olvida.
      sources.splice(sources.indexOf(this) >>> 0, 1);
      for (const [url, source] of byObjectUrl) {
        if (source === this) byObjectUrl.delete(url);
      }
      if (this._ready !== "closed") {
        this._ready = "closed";
        outbox.push({ t: "close", s: this._token });
        later(() => fire(this, "sourceclose"));
      }
    }

    _log(type, element, muted) {
      if (this._events.length >= 60) this._events.shift();
      this._events.push((muted ? "~" : "") + type + "@" + nativeTime.get.call(element).toFixed(1) + "/r" + nativeReady.get.call(element));
    }

    // Posición en el vídeo: la del reproductor más el inicio de su flujo.
    _position(element) {
      const offset = this._offsets.get(this._token);
      return nativeTime.get.call(element) + (offset === undefined ? 0 : offset);
    }

    _currentTime(element) {
      const offset = this._offsets.get(this._token);
      if (this._seekTarget !== null && offset === undefined) return this._seekTarget;
      const now = this._position(element);
      if (this._seekTarget === null) return now;
      // El flujo nuevo empieza en el fotograma clave anterior y el reproductor
      // tarda en dar su posición: hasta que llega al punto pedido, ese punto.
      if (!this._swapping && now >= this._seekTarget - 0.05) {
        this._seekTarget = null;
        return now;
      }
      return this._seekTarget;
    }

    _seek(element, value) {
      const seconds = Math.max(0, Number(value));
      if (!Number.isFinite(seconds)) return;
      if (!this._swapping && this._seekTarget === null && Math.abs(seconds - this._position(element)) < 0.1) return;
      if (!this._swapping) {
        this._wasPaused = nativePaused.get.call(element);
        this._rate = nativeRate.get.call(element);
        if (element instanceof HTMLVideoElement) this._size = [nativeWidth.get.call(element), nativeHeight.get.call(element)];
      }
      this._swapping = true;
      this._seekTarget = seconds;
      this._nudges = 0;
      const next = token();
      outbox.push({ t: "restart", s: this._token, n: next, v: seconds });
      this._token = next;
      this._tokens.add(next);
      later(() => fire(element, "seeking"));
      src.set.call(element, STREAM + next);
      // La carga devuelve la velocidad a la normal y deja el vídeo en pausa:
      // si se estaba reproduciendo, se pide ya (como hace la página al
      // principio); sus eventos no se ven hasta que acabe el salto.
      nativeRate.set.call(element, this._rate);
      this._resume(element);
    }

    _resume(element) {
      if (this._wasPaused || !nativePaused.get.call(element)) return;
      this._resumeNow(element);
    }

    // El flujo nuevo ya tiene imagen: termina el salto.
    _swapped(element) {
      this._swapping = false;
      if (!this._wasPaused && nativePaused.get.call(element)) {
        this._mute.add("play");
        this._resume(element);
      }
      if (!this._wasPaused) this._watch(element);
      fire(element, "timeupdate");
      fire(element, "seeked");
      this._release();
    }

    // El reproductor de Servo, tras cargar un flujo nuevo, a veces dice que
    // reproduce pero no avanza; una pausa y un play lo arrancan. Se comprueba
    // poco después de cada `playing` (como mucho tres veces por salto).
    _watch(element) {
      if (this._watching || this._nudges >= 3) return;
      this._watching = true;
      const from = nativeTime.get.call(element);
      setTimeout(() => {
        this._watching = false;
        if (this._element !== element || this._swapping || nativePaused.get.call(element)) return;
        if (nativeTime.get.call(element) > from + 0.05) return;
        this._nudges += 1;
        this._mute.add("pause");
        this._mute.add("play");
        nativePause.call(element);
        this._resumeNow(element);
      }, 700);
    }

    _resumeNow(element) {
      const playing = nativePlay.call(element);
      if (playing && typeof playing.catch === "function") playing.catch(() => {});
    }

    _release() {
      this._swapping = false;
      for (const resolve of this._waiters.splice(0)) resolve();
    }
  }

  // --- Unión con <video> y URL.createObjectURL ----------------------------

  const createObjectURL = URL.createObjectURL;
  URL.createObjectURL = function (object) {
    if (object instanceof MediaSource) {
      const url = "blob:" + location.origin + "/" + object._token;
      byObjectUrl.set(url, object);
      return url;
    }
    return createObjectURL.call(this, object);
  };

  const revokeObjectURL = URL.revokeObjectURL;
  URL.revokeObjectURL = function (url) {
    if (byObjectUrl.has(String(url))) return;
    return revokeObjectURL.call(this, url);
  };

  const attach = (element, source, shownUrl) => {
    objectUrlOf.set(element, shownUrl);
    source._attach(element);
    return STREAM + source._token;
  };

  // MediaSource del `<video>`, si lo tiene puesto ahora.
  const active = (element) => {
    const source = sourceOf.get(element);
    return source && source._element === element ? source : null;
  };

  const src = native(media, "src");
  const nativeSrc = native(media, "currentSrc");
  const nativeTime = native(media, "currentTime");
  const nativePaused = native(media, "paused");
  const nativeRate = native(media, "playbackRate");
  const nativeReady = native(media, "readyState");
  const nativeWidth = native(HTMLVideoElement.prototype, "videoWidth");
  const nativeHeight = native(HTMLVideoElement.prototype, "videoHeight");
  const nativePlay = media.play;
  const nativePause = media.pause;

  Object.defineProperty(media, "src", {
    configurable: true,
    enumerable: true,
    get() {
      return objectUrlOf.get(this) || src.get.call(this);
    },
    set(value) {
      const source = byObjectUrl.get(String(value));
      const previous = active(this);
      if (previous && previous !== source) previous._detach();
      objectUrlOf.delete(this);
      src.set.call(this, source ? attach(this, source, String(value)) : value);
    },
  });

  // Propiedades del `<video>` que, con una MediaSource puesta, salen de ella
  // (como en cualquier navegador) o se mantienen durante un salto.
  const override = (proto, name, getter, setter) => {
    const original = native(proto, name);
    if (!original.get) return;
    Object.defineProperty(proto, name, {
      configurable: true,
      enumerable: original.enumerable,
      get() {
        const source = active(this);
        return source ? getter.call(this, source, original.get) : original.get.call(this);
      },
      set: setter
        ? function (value) {
            const source = active(this);
            if (source) setter.call(this, source, value);
            else original.set.call(this, value);
          }
        : original.set,
    });
  };

  override(media, "currentSrc", function (source) {
    return objectUrlOf.get(this) || nativeSrc.get.call(this);
  });
  override(
    media,
    "currentTime",
    function (source) {
      return source._currentTime(this);
    },
    function (source, value) {
      source._seek(this, value);
    },
  );
  override(media, "seeking", function (source, get) {
    return source._swapping || get.call(this);
  });
  override(media, "paused", function (source, get) {
    return source._swapping ? source._wasPaused : get.call(this);
  });
  override(media, "duration", function (source, get) {
    return Number.isNaN(source._duration) ? get.call(this) : source._duration;
  });
  override(media, "readyState", function (source, get) {
    return source._swapping ? Math.max(get.call(this), 1) : get.call(this);
  });
  override(media, "buffered", function (source) {
    const lists = source._buffers._items.map((buffer) => buffer._ranges);
    if (!lists.length) return new FerrumTimeRanges([]);
    const both = (a, b) => {
      const out = [];
      for (const [s1, e1] of a) {
        for (const [s2, e2] of b) {
          if (Math.min(e1, e2) > Math.max(s1, s2)) out.push([Math.max(s1, s2), Math.min(e1, e2)]);
        }
      }
      return out.sort((x, y) => x[0] - y[0]);
    };
    return new FerrumTimeRanges(lists.reduce(both));
  });
  override(media, "seekable", function (source) {
    return new FerrumTimeRanges(Number.isFinite(source._duration) ? [[0, source._duration]] : []);
  });
  override(HTMLVideoElement.prototype, "videoWidth", function (source, get) {
    return source._swapping ? source._size[0] : get.call(this);
  });
  override(HTMLVideoElement.prototype, "videoHeight", function (source, get) {
    return source._swapping ? source._size[1] : get.call(this);
  });

  if (typeof media.fastSeek === "function") {
    const fastSeek = media.fastSeek;
    media.fastSeek = function (seconds) {
      const source = active(this);
      if (source) source._seek(this, seconds);
      else fastSeek.call(this, seconds);
    };
  }

  media.play = function () {
    const source = active(this);
    if (!source || !source._swapping) return nativePlay.call(this);
    if (source._wasPaused) {
      source._wasPaused = false;
      source._resume(this);
      later(() => fire(this, "play"));
    }
    return new Promise((resolve) => source._waiters.push(resolve));
  };

  media.pause = function () {
    const source = active(this);
    if (!source || !source._swapping) return nativePause.call(this);
    if (!source._wasPaused) {
      source._wasPaused = true;
      nativePause.call(this);
      later(() => {
        fire(this, "timeupdate");
        fire(this, "pause");
      });
    }
  };

  // Durante un salto, la página no ve los eventos de la recarga del `<video>`.
  // Se escucha en `window` en fase de captura, antes que cualquier otro.
  const DONE = new Set(["loadeddata", "canplay", "canplaythrough"]);
  for (const type of ["emptied", "abort", "loadstart", "progress", "suspend", "stalled", "waiting", "durationchange", "loadedmetadata", "loadeddata", "canplay", "canplaythrough", "play", "playing", "pause", "timeupdate", "seeking", "seeked", "ratechange", "resize", "ended", "error"]) {
    window.addEventListener(
      type,
      (event) => {
        const element = event.target;
        const source = element instanceof HTMLMediaElement ? active(element) : null;
        if (!source || ours.has(event)) return;
        const muted = source._swapping || source._mute.has(type);
        if (type !== "progress" && type !== "timeupdate") source._log(type, element, muted);
        if (source._mute.delete(type)) {
          event.stopImmediatePropagation();
          return;
        }
        if (!source._swapping) {
          if (type === "playing") source._watch(element);
          return;
        }
        if (type === "error") {
          // El flujo nuevo falló: la página lo tiene que saber.
          source._seekTarget = null;
          source._release();
          return;
        }
        event.stopImmediatePropagation();
        if (DONE.has(type)) source._swapped(element);
      },
      true,
    );
  }

  const setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    if (this instanceof HTMLMediaElement && String(name).toLowerCase() === "src") {
      this.src = value;
      return;
    }
    return setAttribute.call(this, name, value);
  };

  Object.defineProperty(window, Symbol.for("ferrum.mse"), {
    value: Object.freeze({
      // Ferrum recoge lo pendiente y dice qué tramos tiró por memoria.
      drain(notes) {
        for (const note of Array.isArray(notes) ? notes : []) {
          const source = sources.find((item) => item._tokens.has(note.s));
          if (!source) continue;
          if (note.t === "offset") {
            source._offsets.set(note.s, Number(note.o) || 0);
          } else if (note.t === "evict") {
            const buffer = source._buffers._items.find((item) => item._index === note.i);
            if (buffer) buffer._ranges = removeRange(buffer._ranges, note.a, note.b);
          }
        }
        return JSON.stringify(outbox.splice(0, outbox.length));
      },
      // Para depurar (por ejemplo, desde el servidor MCP).
      stats() {
        return JSON.stringify(sources.map((source) => ({
          state: source.readyState,
          duration: source.duration,
          attached: !!source._element,
          swapping: source._swapping,
          target: source._seekTarget,
          time: source._element ? nativeTime.get.call(source._element) : null,
          offset: source._offsets.get(source._token),
          events: source._events,
          src: source._element ? String(nativeSrc.get.call(source._element)).slice(0, 60) : null,
          buffers: source._buffers._items.map((b) => ({
            type: b._type,
            appends: b._appends,
            bytes: b._appended,
            init: !!b._track,
            buffered: b._ranges,
            offset: b.timestampOffset,
          })),
        })));
      },
    }),
  });

  window.MediaSource = MediaSource;
  window.SourceBuffer = SourceBuffer;
  window.SourceBufferList = SourceBufferList;
})();
