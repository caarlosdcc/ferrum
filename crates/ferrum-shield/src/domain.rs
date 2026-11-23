/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Utilidades de dominios.
//!
//! Para saber si una petición es de "primera parte" (el propio sitio) o de un
//! tercero, comparamos el *dominio registrable* (eTLD+1): `news.example.co.uk`
//! y `cdn.example.co.uk` comparten `example.co.uk`.
//!
//! Usamos una aproximación compacta de la Public Suffix List con los sufijos de
//! dos niveles más comunes. Equivocarse aquí solo hace que un rastreador se
//! trate como tercero (se bloquea), que es el lado seguro del error.

use url::{Host, Url};

/// Sufijos públicos de dos niveles frecuentes (`co.uk`, `com.br`…).
const TWO_LEVEL_SUFFIXES: &[&str] = &[
    "ac.uk",
    "co.uk",
    "gov.uk",
    "ltd.uk",
    "me.uk",
    "net.uk",
    "org.uk",
    "plc.uk",
    "com.au",
    "net.au",
    "org.au",
    "edu.au",
    "gov.au",
    "co.nz",
    "net.nz",
    "org.nz",
    "co.jp",
    "ne.jp",
    "or.jp",
    "ac.jp",
    "go.jp",
    "co.kr",
    "or.kr",
    "co.in",
    "net.in",
    "org.in",
    "gov.in",
    "co.za",
    "org.za",
    "com.ar",
    "com.bo",
    "com.br",
    "net.br",
    "org.br",
    "gov.br",
    "com.co",
    "com.do",
    "com.ec",
    "com.gt",
    "com.hn",
    "com.mx",
    "org.mx",
    "gob.mx",
    "com.ni",
    "com.pa",
    "com.pe",
    "com.py",
    "com.sv",
    "com.uy",
    "com.ve",
    "gob.ar",
    "gob.es",
    "gov.co",
    "com.es",
    "org.es",
    "nom.es",
    "edu.es",
    "com.cn",
    "net.cn",
    "org.cn",
    "com.hk",
    "com.tw",
    "com.sg",
    "com.my",
    "com.tr",
    "com.ua",
    "com.pl",
    "co.il",
    "co.id",
    "or.id",
    "com.ph",
    "com.vn",
    "co.th",
    "github.io",
    "gitlab.io",
    "pages.dev",
    "netlify.app",
    "vercel.app",
    "herokuapp.com",
    "blogspot.com",
    "appspot.com",
    "web.app",
    "firebaseapp.com",
    "azurewebsites.net",
    "cloudfront.net",
    "s3.amazonaws.com",
];

/// Dominio registrable (eTLD+1) de un nombre de host, en minúsculas.
/// Devuelve el propio host si es una IP o tiene una sola etiqueta.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let take = if TWO_LEVEL_SUFFIXES.contains(&last_two.as_str()) {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(take)..].join(".")
}

/// ¿Pertenecen ambas URLs al mismo sitio (mismo dominio registrable)?
pub fn same_site(a: &Url, b: &Url) -> bool {
    match (a.host(), b.host()) {
        (Some(Host::Domain(x)), Some(Host::Domain(y))) => {
            registrable_domain(x) == registrable_domain(y)
        },
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// ¿Es un host local (localhost, loopback o `.localhost`)?
pub fn is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable_domain("www.example.com"), "example.com");
        assert_eq!(registrable_domain("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(
            registrable_domain("tienda.mercado.com.mx"),
            "mercado.com.mx"
        );
        assert_eq!(registrable_domain("example.com"), "example.com");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("WWW.Example.COM."), "example.com");
    }

    #[test]
    fn same_site_detection() {
        let a = Url::parse("https://news.example.com/").expect("url");
        let b = Url::parse("https://cdn.example.com/x.js").expect("url");
        let c = Url::parse("https://example.org/").expect("url");
        assert!(same_site(&a, &b));
        assert!(!same_site(&a, &c));
    }

    #[test]
    fn local_hosts() {
        assert!(is_local(
            &Url::parse("http://localhost:8080/").expect("url")
        ));
        assert!(is_local(&Url::parse("http://127.0.0.1/").expect("url")));
        assert!(is_local(&Url::parse("http://app.localhost/").expect("url")));
        assert!(!is_local(&Url::parse("https://example.com/").expect("url")));
    }
}
