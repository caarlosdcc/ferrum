/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! En Windows, incrusta en `ferrum.exe` el icono y los datos de versión que
//! muestran el Explorador y el Administrador de tareas. El icono se genera con
//! `scripts/generate-icons.py`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=resources/ferrum.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo define OUT_DIR"));
    let icon = out.join("ferrum.ico");
    std::fs::copy("resources/ferrum.ico", &icon).expect("no se pudo copiar el icono");

    let version = env!("CARGO_PKG_VERSION");
    let mut numbers = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    numbers.resize(4, 0);
    let numeric = numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let resource = format!(
        r#"1 ICON "ferrum.ico"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Ferrum contributors"
      VALUE "FileDescription", "Ferrum"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "ferrum"
      VALUE "LegalCopyright", "MPL-2.0"
      VALUE "OriginalFilename", "ferrum.exe"
      VALUE "ProductName", "Ferrum"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
    );
    let script = out.join("ferrum.rc");
    std::fs::write(&script, resource).expect("no se pudo escribir ferrum.rc");
    if let Err(error) = embed_resource::compile(&script, embed_resource::NONE).manifest_optional() {
        panic!("no se pudieron incrustar los recursos de Windows: {error}");
    }
}
