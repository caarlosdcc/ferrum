# servo-net 0.8.0 con arreglos para Ferrum

Copia de [`servo-net` 0.8.0](https://crates.io/crates/servo-net/0.8.0) (la capa
de red de [Servo](https://github.com/servo/servo), licencia MPL-2.0, la misma
que Ferrum) con dos cambios: el límite de cabeceras HTTP/2 (`connector.rs`) y
las respuestas interceptadas de audio y vídeo por partes
(`request_interceptor.rs`). Busca `Ferrum:` en ambos.

## El problema

Servo crea su cliente HTTP con `hyper_util::client::legacy::Client` sin cambiar
el límite de cabeceras de HTTP/2, que en hyper es de 16 KB por respuesta. Al
conectar se lo anuncia al servidor (`SETTINGS_MAX_HEADER_LIST_SIZE`), y si una
respuesta trae más, la biblioteca `h2` la rechaza (`REQUEST_HEADER_FIELDS_TOO_LARGE`)
y corta el flujo con `PROTOCOL_ERROR`. La página no carga y Servo muestra
«client error (SendRequest)».

YouTube ya manda más de 16 KB de cabeceras (solo
`connection-allowlist-report-only` ocupa más de 7 KB), así que no abría.
Chrome acepta 256 KB.

## El arreglo

`create_http_client` llama a `.http2_max_header_list_size(256 * 1024)` (busca
`Ferrum:` en `connector.rs`).

Además, `Cargo.toml` silencia los avisos `unused` del código que solo usan las
devtools (Ferrum no las compila): desde el registro de cargo no se mostraban,
pero desde una carpeta local sí.

## Respuestas interceptadas de audio y vídeo, por partes

Cuando el navegador (Ferrum) responde él mismo a una petición
(`WebResourceLoad::intercept`), Servo acumulaba todo el cuerpo hasta el final
(`accumulated_body`, con un `TODO` en su código) antes de entregarlo. Para un
vídeo que no termina, como el flujo de las Media Source Extensions de Ferrum
(`--mse`), eso significa que el reproductor nunca recibe nada.

Ahora, si la petición es de audio o vídeo (`Destination::Audio | Video`), al
llegar la cabecera se crea un `done_chan` y cada trozo se pasa como
`Data::Payload`, igual que con las respuestas de red; al terminar, `Data::Done`.
El cuerpo no se guarda en memoria (un flujo puede durar horas). El resto de
respuestas interceptadas no cambia. `fetch/methods.rs` pasa su `done_chan` al
interceptor.

## Cómo actualizar

Al subir de versión Servo, comprueba si la nueva versión de `servo-net` ya fija
este límite. Si lo hace, borra esta carpeta y su línea de `[patch.crates-io]`
en el `Cargo.toml` raíz. Si no, copia la nueva versión del registro de cargo y
vuelve a aplicar el cambio.
