# Ferrum

**Navegador web ligero, seguro y privado, escrito en Rust sobre el motor [Servo](https://servo.org).**

Ferrum parte de una idea simple: un navegador debería proteger a quien lo usa
sin que tenga que configurar nada. Bloquea rastreadores, no guarda nada al
cerrarse, apaga las APIs que se usan para espiar y frena las descargas que
instalan malware. Todo con un consumo de memoria contenido.

> **Estado: alfa experimental.** Ferrum funciona, pero Servo todavía no muestra
> bien todos los sitios y faltan piezas de interfaz (ver [Limitaciones](#limitaciones)).
> No lo uses aún como único navegador para banca u otras tareas críticas.

---

## Qué te protege

| Protección | Qué hace | Dónde está |
|---|---|---|
| 🛡 **Bloqueo de rastreadores** | Corta las peticiones de terceros a publicidad, analítica, grabación de sesiones, fingerprinting y criptominería. Ampliable con listas hosts / Adblock. | `ferrum-shield/src/blocklist.rs` |
| 🔒 **Solo HTTPS** | Toda conexión `http://` se mejora a `https://` dentro del motor (incluidas imágenes y scripts). | `hardening.rs` |
| 🧹 **Enlaces limpios** | Quita `utm_*`, `fbclid`, `gclid`, `msclkid` y otros 50 identificadores antes de que la petición salga. | `ferrum-shield/src/url_clean.rs` |
| 🚫 **Descargas peligrosas** | Bloquea enlaces directos a `.exe`, `.msi`, `.bat`, `.ps1`, `.scr`, `.js`, `.lnk`, `.iso`, macros de Office, `.dmg`, `.apk`… | `ferrum-shield/src/downloads.rs` |
| ☠ **Listas de malware** | Dominios de phishing o malware que cargues con `--malware-list` se bloquean incluso al visitarlos directamente. | `ferrum-shield/src/lib.rs` |
| 🗑 **Sin rastro** | Cookies, `localStorage`, IndexedDB y caché viven en almacenamiento temporal: al cerrar, desaparecen. Sin historial en disco. | `hardening.rs` |
| 📵 **APIs de espionaje apagadas** | WebRTC (filtra tu IP real tras una VPN), geolocalización, notificaciones, Bluetooth, gamepad, WebGL/WebGPU, service workers, lectura del portapapeles. | `hardening.rs` |
| 🙅 **Permisos denegados** | Cámara, micrófono, ubicación y notificaciones se deniegan sin preguntar. | `browser.rs` |
| 🪟 **Anti pop-ups** | Una página solo abre ventanas nuevas justo después de un clic tuyo. | `browser.rs` |
| 🎭 **Anti huella digital** | User-Agent común, núcleos/memoria/pantalla normalizados, sin API de batería, señal *Global Privacy Control*. | `resources/privacy.js` |
| 🧯 **Diálogos que no secuestran** | `alert()`/`confirm()` se muestran en una franja bajo la barra, marcados como texto de la página, y se cierran solos: adiós a los bucles de "¡tu PC está infectado!". | `browser.rs` |
| 🔤 **Anti suplantación** | La interfaz se dibuja fuera del motor web: ninguna página puede taparla ni imitarla. Los dominios internacionales se muestran en *punycode* y se eliminan caracteres bidi de los títulos, para que `аpple.com` (con "а" cirílica) no pase por `apple.com`. | `gui.rs`, `browser.rs` |
| 🔐 **Modo estricto** | `--strict` apaga el JIT de JavaScript y WebAssembly (la vía de la mayoría de exploits), aísla cada página en su propio proceso y, en Linux/macOS, lo encierra en un sandbox. | `hardening.rs` |

### Por qué Rust y Servo

- **Seguridad de memoria.** La mayoría de vulnerabilidades graves de los navegadores
  son errores de memoria en C++ (uso tras liberar, desbordamientos). Servo está
  escrito en Rust, que elimina esa clase de errores en tiempo de compilación; la
  excepción es su motor JavaScript, SpiderMonkey (C++, el mismo de Firefox), y por
  eso existe el modo `--strict`. El código de Ferrum prohíbe `unsafe` por completo
  (`unsafe_code = "forbid"`).
- **Ligero.** Sin telemetría, sin servicios en segundo plano, sin sincronización,
  sin actualizador residente. Lo que no está, no consume RAM ni se puede atacar.
- **Sin telemetría. Ninguna.** Ferrum no se conecta a ningún servidor propio.
  Las únicas conexiones son las páginas que tú visitas.

## Limitaciones

Ser honestos con lo que **no** hace es parte de la seguridad:

- **Compatibilidad.** Servo es un motor joven: algunos sitios complejos se ven
  mal o no funcionan. Mejora cada mes.
- **Sin vídeo ni audio.** La compilación aún no incluye el reproductor
  multimedia (GStreamer): YouTube y cualquier `<video>` o `<audio>` no se
  reproducen. Tampoco hay DRM, así que Netflix o Spotify no funcionarán.
- **Interfaz sencilla.** Hay pestañas, barra de herramientas, barra de
  direcciones, listas desplegables, selector de color y subida de archivos. Aún
  no hay descargas, marcadores, historial ni menú de clic derecho.
- **Windows sin sandbox.** En Windows el modo `--multiprocess` separa procesos,
  pero Servo aún no restringe sus privilegios (sí en Linux x86_64 y macOS).
- **No es un antivirus.** El bloqueo de descargas es por tipo de archivo y las
  listas de malware son las que tú cargues; no hay análisis de contenido.
- **No es anonimato.** Ferrum evita el rastreo entre sitios, pero tu proveedor
  de internet y los sitios que visitas ven tu IP. Para anonimato usa Tor.
- **Fingerprinting.** Las mitigaciones reducen la huella, pero un motor poco
  común como Servo sigue siendo distinguible.

Más detalle en [SECURITY.md](SECURITY.md).

## Uso

```
ferrum [OPCIONES] [URL o búsqueda]
```

| Atajo | Acción |
|---|---|
| `Ctrl+L` / `F6` / `Alt+D` | Ir a la barra de direcciones |
| `Ctrl+T` / `Ctrl+W` | Nueva pestaña / cerrar pestaña (también clic central en la pestaña) |
| `Ctrl+Tab` / `Ctrl+Mayús+Tab` | Pestaña siguiente / anterior |
| `Alt+←` / `Alt+→` | Atrás / adelante (también los botones laterales del ratón) |
| `F5` / `Ctrl+R` | Recargar |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Zoom |

| Opción | Efecto |
|---|---|
| `--strict` | Sin JIT ni WebAssembly, multiproceso y sandbox donde exista |
| `--multiprocess` | Aísla el contenido web en procesos separados |
| `--blocklist <archivo>` | Lista extra de rastreadores (hosts, dominios o `\|\|dominio^`) |
| `--malware-list <archivo>` | Lista de dominios maliciosos (se bloquean siempre) |
| `--search ddg\|mojeek` | Buscador de la barra (por defecto DuckDuckGo HTML) |
| `--proxy <url>` | Enviar el tráfico por un proxy HTTP |
| `--screenshot <archivo.png>` | Guardar una captura al terminar de cargar y cerrar (pruebas) |
| `--click <X,Y>` | Con `--screenshot`: hacer clic en ese punto antes de la captura |

Ejemplo con listas públicas de la comunidad (descárgalas antes):

```
ferrum --blocklist easyprivacy.txt --malware-list urlhaus-hosts.txt
```

## Compilar

Necesitas Rust estable (1.95 o superior). Compilar Servo es pesado: reserva
~30 GB de disco y 8 GB de RAM o más; la primera compilación tarda bastante.

### Windows

1. [Rust](https://rustup.rs) con el toolchain MSVC.
2. Visual Studio 2022 Build Tools con: *MSVC v143*, *C++ ATL* y *Windows 11 SDK*.
   ```powershell
   winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.ATL --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --includeRecommended"
   ```
3. LLVM (Servo lo usa para generar bindings de SpiderMonkey):
   ```powershell
   winget install LLVM.LLVM
   ```
4. Compila y abre el navegador con el script, que configura LLVM y, si el
   proyecto está en OneDrive o en una ruta con espacios, compila fuera de ella
   (la carpeta `target` ocupa decenas de GB y no conviene sincronizarla):
   ```powershell
   .\scripts\build-windows.ps1 -Run
   ```
   O a mano:
   ```powershell
   $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
   cargo build --release
   .\target\release\ferrum.exe
   ```

### Linux (Debian/Ubuntu)

```sh
sudo apt install build-essential clang libclang-dev cmake ninja-build pkg-config \
    libx11-dev libxkbcommon-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev
cargo build --release
```

### macOS

```sh
xcode-select --install
brew install llvm cmake ninja
cargo build --release
```

Binario optimizado para distribuir (LTO, sin símbolos): `cargo build --profile production`.

## Arquitectura

```
ferrum/
├── crates/
│   ├── ferrum-shield/      Motor de privacidad. Sin dependencias del motor web:
│   │   ├── lists/          se prueba en segundos y se audita fácilmente.
│   │   │   └── trackers.txt  Lista integrada (edítala con un PR)
│   │   └── src/
│   │       ├── blocklist.rs  Dominios y rutas bloqueadas, O(etiquetas) por consulta
│   │       ├── url_clean.rs  Parámetros de rastreo
│   │       ├── downloads.rs  Extensiones peligrosas
│   │       ├── omnibox.rs    Barra de direcciones segura (https por defecto)
│   │       └── domain.rs     Primera parte vs. terceros
│   └── ferrum/             La aplicación
│       ├── resources/privacy.js  Script anti-huella inyectado en cada página
│       └── src/
│           ├── main.rs       Arranque y procesos de contenido
│           ├── app.rs        Bucle de la ventana: reparte teclado y ratón
│           ├── gui.rs        Interfaz (egui): pestañas, barra, desplegables
│           ├── browser.rs    Pestañas + filtro de cada petición de red
│           ├── hardening.rs  Preferencias endurecidas de Servo
│           ├── pages.rs      Páginas internas (inicio, bloqueo)
│           └── input.rs      Traducción de eventos
└── third_party/epaint/     Copia de epaint (egui) con un arreglo: ver FERRUM.md
```

La página se pinta en un búfer aparte y la interfaz la copia debajo de la barra.
La interfaz nunca pasa por el motor web, así que una página no puede leerla,
taparla ni dibujar una barra de direcciones falsa encima.

Cada petición de red pasa por `Browser::filter_request` **antes** de salir del
equipo. El documento principal se compara con las listas de malware, descargas
peligrosas y parámetros de rastreo; los recursos secundarios (scripts, imágenes,
iframes) con la lista de rastreadores, salvo que sean del propio sitio.

## Hoja de ruta

- [x] Barra de herramientas gráfica (egui) con pestañas visibles
- [x] Desplegables `<select>`, selector de color y subida de archivos
- [x] Web Crypto (`crypto.subtle`) para inicios de sesión y apps cifradas
- [ ] Reproducción de vídeo y audio
- [ ] Menú de clic derecho y diálogos propios
- [ ] Panel del escudo: qué se bloqueó y excepciones por sitio
- [ ] Descargas con cuarentena y verificación
- [ ] Motor de reglas Adblock completo (`adblock-rust`)
- [ ] Actualización automática y firmada de listas de bloqueo
- [ ] DNS sobre HTTPS
- [ ] Contenedores por sitio (cookies aisladas por dominio)
- [ ] Ruido en canvas y audio contra fingerprinting
- [ ] Paquetes firmados para Windows, Linux y macOS

## Contribuir

Las contribuciones son bienvenidas: ver [CONTRIBUTING.md](CONTRIBUTING.md).
Para vulnerabilidades, **no abras un issue público**: sigue [SECURITY.md](SECURITY.md).

## Licencia

[Mozilla Public License 2.0](LICENSE), la misma que Servo y Firefox.
