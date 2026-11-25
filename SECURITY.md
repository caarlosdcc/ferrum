# Política de seguridad

## Informar de una vulnerabilidad

**No abras un issue público.** Usa el formulario privado de GitHub:
pestaña *Security* → *Report a vulnerability* en este repositorio.

Incluye, si puedes: versión o commit de Ferrum, sistema operativo, pasos para
reproducirlo y el impacto que crees que tiene. Intentaremos responder en un
plazo de 7 días y publicar una corrección coordinada contigo.

Si el fallo está en el motor Servo (y no en el código de Ferrum), repórtalo
también a Servo siguiendo su [política de seguridad](https://github.com/servo/servo/security/policy).

## Modelo de amenazas

### Contra qué protege Ferrum

| Amenaza | Defensa |
|---|---|
| Rastreo entre sitios (publicidad, analítica, *data brokers*) | Bloqueo de peticiones de terceros por lista; limpieza de parámetros de rastreo; sin cookies persistentes |
| Recolección de datos tras cerrar el navegador | Almacenamiento temporal: cookies, `localStorage`, IndexedDB y caché se borran al salir; sin caché en disco |
| Filtración de IP real (por ejemplo, tras una VPN) | WebRTC desactivado |
| Interceptación de tráfico en redes Wi-Fi públicas | Modo solo-HTTPS en el motor; TLS con rustls + aws-lc-rs |
| Descarga e instalación de malware | Bloqueo de enlaces directos a ejecutables, scripts, instaladores, imágenes de disco y documentos con macros |
| Phishing y dominios maliciosos conocidos | Listas de malware cargadas con `--malware-list`; dominios internacionales en punycode |
| Suplantación de la interfaz | La interfaz se dibuja fuera del motor web (la página no puede taparla ni imitarla); dirección completa en punycode; caracteres bidi y de control eliminados del texto de la página; `alert()` marcado como texto de la página |
| Spam de ventanas y diálogos | Pop-ups solo tras un gesto del usuario; diálogos que se cierran solos |
| Abuso de permisos (cámara, micrófono, ubicación, notificaciones) | Denegados siempre |
| Acceso a archivos locales desde la web | Navegación a `file://` bloqueada; `javascript:`, `data:` y `file:` no se abren desde la barra |
| Exploits de memoria en el motor | Rust en casi todo Servo; `--strict` apaga el JIT y WebAssembly de SpiderMonkey y aísla el contenido en procesos (con sandbox en Linux x86_64 y macOS) |
| Fingerprinting | User-Agent común; WebGL, WebGPU, gamepad, batería desactivados; pantalla, núcleos y memoria normalizados |

### Fuera del alcance

- **Anonimato de red.** Tu IP es visible para los sitios y tu proveedor. Usa Tor
  si necesitas anonimato.
- **Un equipo ya comprometido.** Si tu sistema tiene malware, ningún navegador
  puede protegerte de él.
- **Análisis de contenido.** El bloqueo de descargas es por tipo de archivo; no
  sustituye a un antivirus.
- **Phishing desconocido.** Sin listas de malware cargadas, Ferrum no consulta
  ningún servicio de reputación (lo haría a costa de tu privacidad).
- **Ataques de canal lateral** (Spectre y similares) más allá de lo que mitiga
  el motor y el modo `--strict`.

### Limitaciones conocidas

- **Windows no tiene sandbox de procesos** en Servo: `--multiprocess` separa el
  contenido en otros procesos, pero con los mismos privilegios del usuario.
- **SpiderMonkey** (motor JavaScript) está escrito en C++. Es el componente con
  más superficie de ataque; `--strict` reduce mucho el riesgo al desactivar el JIT.
- **Servo es software joven** y no ha pasado por las mismas auditorías ni el
  mismo volumen de *fuzzing* que Chromium o Firefox.
- El **script anti-huella** se ejecuta dentro de la página; una web decidida
  puede detectar que los valores están normalizados.
- Con **`--mcp`**, quien lanzó Ferrum (un agente de IA) lo controla por
  completo: navega, lee las páginas y ejecuta JavaScript en ellas. El canal es
  solo la entrada y la salida estándar, sin puertos de red, y la barra muestra
  la pastilla «Agente» mientras dura. Úsalo solo con agentes en los que confíes,
  y recuerda que el texto de las páginas que lee puede intentar manipularlo
  (*prompt injection*).

## Principios de diseño

1. **Seguro por defecto.** Ninguna protección requiere configuración.
2. **Mínimo privilegio.** Toda API que no hace falta para leer la web está apagada.
3. **Cero telemetría.** Ferrum no contacta con ningún servidor propio, nunca.
4. **Nada en disco.** El estado de navegación vive en memoria.
5. **Código auditable.** Sin `unsafe` en el código de Ferrum; las políticas viven
   en `ferrum-shield`, separado del motor y cubierto por tests. El código de
   terceros modificado vive en `third_party/` con sus cambios documentados.
6. **Honestidad.** No prometemos lo que no podemos garantizar.
