/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Protección contra descargas peligrosas.
//!
//! La mayoría del malware llega como un archivo que el usuario ejecuta. Ferrum
//! bloquea la navegación directa a tipos de archivo capaces de ejecutar código
//! en Windows, macOS o Linux. Es una barrera por extensión: no sustituye a un
//! antivirus, pero corta el vector más común ("descarga este .exe").

use url::Url;

/// Extensiones que pueden ejecutar código al abrirse.
const DANGEROUS_EXTENSIONS: &[&str] = &[
    // Windows: ejecutables e instaladores
    "exe",
    "msi",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "com",
    "scr",
    "pif",
    "cpl",
    "dll",
    "sys",
    "drv",
    "ocx",
    // Windows: scripts y accesos directos
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "psd1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "lnk",
    "url",
    "scf",
    "inf",
    "reg",
    "msc",
    "msp",
    "mst",
    "application",
    "gadget",
    "appref-ms",
    "settingcontent-ms",
    "library-ms",
    // Office con macros
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "xlam",
    "pptm",
    "potm",
    "ppam",
    "sldm",
    // Imágenes de disco que Windows monta y ejecuta con un clic
    "iso",
    "img",
    "vhd",
    "vhdx",
    // macOS
    "dmg",
    "pkg",
    "mpkg",
    "app",
    "command",
    "workflow",
    // Linux
    "sh",
    "run",
    "bin",
    "deb",
    "rpm",
    "appimage",
    "desktop",
    // Multiplataforma
    "jar",
    "jnlp",
    "apk",
    "xapk",
];

/// Si la URL apunta directamente a un archivo peligroso, devuelve su extensión.
///
/// Solo mira la ruta (no los parámetros): `https://x.com/setup.exe` se bloquea,
/// `https://x.com/page?file=setup.exe` no, porque ahí la ruta es una página.
pub fn dangerous_extension(url: &Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https" | "ftp") {
        return None;
    }
    let last_segment = url.path_segments()?.next_back()?;
    let decoded = percent_decode(last_segment);
    let (_, extension) = decoded.rsplit_once('.')?;
    let extension = extension.trim().to_ascii_lowercase();
    DANGEROUS_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(extension)
}

/// Decodificación `%XX` mínima, para que `setup%2Eexe` no esquive el filtro.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(s: &str) -> Option<String> {
        dangerous_extension(&Url::parse(s).expect("url"))
    }

    #[test]
    fn detects_executables() {
        assert_eq!(ext("https://example.com/setup.exe"), Some("exe".into()));
        assert_eq!(
            ext("https://example.com/a/b/Invoice.PDF.js"),
            Some("js".into())
        );
        assert_eq!(ext("https://example.com/tool.msi?v=2"), Some("msi".into()));
        assert_eq!(ext("https://example.com/setup%2Eexe"), Some("exe".into()));
        assert_eq!(
            ext("https://example.com/game.AppImage"),
            Some("appimage".into())
        );
    }

    #[test]
    fn allows_documents_and_pages() {
        assert_eq!(ext("https://example.com/"), None);
        assert_eq!(ext("https://example.com/report.pdf"), None);
        assert_eq!(ext("https://example.com/index.html"), None);
        assert_eq!(ext("https://example.com/page?file=setup.exe"), None);
        assert_eq!(ext("https://example.com/photo.jpg"), None);
    }

    #[test]
    fn ignores_non_network_schemes() {
        assert_eq!(ext("data:text/plain,hola.exe"), None);
    }
}
