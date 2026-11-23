# Cómo contribuir a Ferrum

¡Gracias por tu interés! Ferrum es un proyecto abierto y cualquier ayuda cuenta:
código, listas de bloqueo, pruebas en distintos sistemas, traducciones o
simplemente reportar sitios que no funcionan.

## Primeros pasos

```sh
git clone https://github.com/caarlosdcc/ferrum
cd ferrum

# El motor de privacidad compila en segundos y no necesita Servo:
cargo test -p ferrum-shield

# El navegador completo (la primera vez tarda; ver README para dependencias):
cargo build --release
```

## Antes de abrir un pull request

```sh
cargo fmt --all
cargo clippy -p ferrum-shield --all-targets -- -D warnings
cargo test -p ferrum-shield
```

- Un PR = un cambio. Explica **qué** cambia y **por qué**.
- Toda política nueva de privacidad o seguridad va en `ferrum-shield` y con tests.
- Nada de `unsafe`: el workspace lo prohíbe.
- Nada de telemetría, analítica ni conexiones a servidores propios. Nunca.
- Si añades una dependencia, justifica por qué hace falta: cada crate es código
  de terceros que se ejecuta con tus datos.

## Proponer dominios para la lista de bloqueo

Edita `crates/ferrum-shield/lists/trackers.txt`. En el PR indica:

1. Qué hace el dominio (analítica, publicidad, fingerprinting…).
2. Una fuente que lo respalde (por ejemplo, su aparición en EasyPrivacy,
   Disconnect o la documentación del propio servicio).
3. Que bloquearlo no rompe sitios populares como tercero.

## Reportar un sitio que no funciona

Abre un issue con la URL, qué esperabas y qué pasó. Prueba también con
`--blocklist` vacío y sin `--strict` para saber si es el bloqueo o el motor.
Si es el motor, probablemente sea un fallo de Servo: búscalo en
[su repositorio](https://github.com/servo/servo/issues).

## Vulnerabilidades

Nunca en issues públicos. Ver [SECURITY.md](SECURITY.md).

## Licencia

Al contribuir aceptas que tu aportación se publique bajo la
[MPL-2.0](LICENSE). Añade esta cabecera a los archivos nuevos:

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
```
