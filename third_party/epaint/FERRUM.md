# epaint 0.37.0 con un arreglo para Ferrum

Copia sin cambios de [`epaint` 0.37.0](https://crates.io/crates/epaint/0.37.0)
(parte de [egui](https://github.com/emilk/egui), licencia MIT o Apache-2.0;
aquí se usa bajo MIT, ver `LICENSE-MIT`), salvo un arreglo en
`src/text/font_face.rs`.

## El problema

epaint dibuja cada glifo con `vello_cpu::RenderContext::new(...)` y llama a
`render()` directamente. Servo también usa vello_cpu (para `<canvas>`) y activa
su función `multithreading`. Cargo une las funciones de una dependencia para todo
el programa, así que en Ferrum `RenderContext::new` crea un despachador
multihilo, que:

- exige `flush()` antes de `render()` (si no, pánico: *attempted to rasterize
  before flushing*);
- arrancaría un grupo de hilos por cada letra nueva, algo muy lento.

## El arreglo

Los dos `RenderContext::new` pasan a `RenderContext::new_with(...)` con
`num_threads: 0`, que en vello_cpu significa "dibujar en el hilo actual".

## Cómo actualizar

Al subir de versión egui, comprueba si el problema sigue en la nueva versión de
epaint. Si ya no, borra esta carpeta y la sección `[patch.crates-io]` del
`Cargo.toml` raíz. Si sigue, copia la nueva versión del registro de cargo y
vuelve a aplicar el cambio (busca `Ferrum:` en `src/text/font_face.rs`).
