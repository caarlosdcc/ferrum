# servo-net 0.8.0 con un arreglo para Ferrum

Copia sin cambios de [`servo-net` 0.8.0](https://crates.io/crates/servo-net/0.8.0)
(la capa de red de [Servo](https://github.com/servo/servo), licencia MPL-2.0,
la misma que Ferrum), salvo una línea en `connector.rs`.

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

## Cómo actualizar

Al subir de versión Servo, comprueba si la nueva versión de `servo-net` ya fija
este límite. Si lo hace, borra esta carpeta y su línea de `[patch.crates-io]`
en el `Cargo.toml` raíz. Si no, copia la nueva versión del registro de cargo y
vuelve a aplicar el cambio.
