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

  const token = () =>
    Array.from(crypto.getRandomValues(new Uint8Array(12)), (b) => b.toString(16).padStart(2, "0")).join("");

  const later = (task) => setTimeout(task, 0);

  const fire = (target, type) => {
    const event = new Event(type);
    target.dispatchEvent(event);
    const handler = target["on" + type];
    if (typeof handler === "function") {
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
      this._operation(() => {
        this._ranges = removeRange(this._ranges, Number(start), Number(end));
      });
    }

    abort() {
      if (this._updating) {
        this._updating = false;
        fire(this, "abort");
        fire(this, "updateend");
      }
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
      this._ready = "closed";
      this._duration = NaN;
      this._buffers = new SourceBufferList();
      this._element = null;
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
      this._events = [];
      for (const type of ["loadstart", "loadedmetadata", "canplay", "playing", "waiting", "error", "abort", "emptied", "seeking", "seeked", "stalled", "suspend"]) {
        element.addEventListener(type, () => {
          if (this._events.length < 40) {
            this._events.push(type + "@" + element.currentTime.toFixed(1) + "/n" + element.networkState + "/r" + element.readyState);
          }
        });
      }
      this._ready = "open";
      later(() => fire(this, "sourceopen"));
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

  const src = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "src");
  Object.defineProperty(HTMLMediaElement.prototype, "src", {
    configurable: true,
    enumerable: true,
    get() {
      return objectUrlOf.get(this) || src.get.call(this);
    },
    set(value) {
      const source = byObjectUrl.get(String(value));
      objectUrlOf.delete(this);
      src.set.call(this, source ? attach(this, source, String(value)) : value);
    },
  });

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
      drain() {
        return JSON.stringify(outbox.splice(0, outbox.length));
      },
      // Para depurar (por ejemplo, desde el servidor MCP).
      stats() {
        return JSON.stringify(sources.map((source) => ({
          state: source.readyState,
          duration: source.duration,
          attached: !!source._element,
          events: source._events || [],
          src: source._element ? String(Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "currentSrc").get.call(source._element)).slice(0, 60) : null,
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
