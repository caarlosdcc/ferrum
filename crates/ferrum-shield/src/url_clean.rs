/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Limpieza de parámetros de rastreo en URLs.
//!
//! Redes sociales y plataformas de anuncios añaden identificadores a los
//! enlaces (`?fbclid=…`, `?gclid=…`) para seguirte de un sitio a otro. Quitarlos
//! no cambia la página que ves, pero corta ese seguimiento.

use url::Url;

/// Parámetros que se eliminan siempre (comparación sin distinguir mayúsculas).
const TRACKING_PARAMS: &[&str] = &[
    // Google Ads / Analytics
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "_ga",
    "_gl",
    "srsltid",
    // Meta (Facebook, Instagram)
    "fbclid",
    "igshid",
    "igsh",
    // Microsoft / Bing
    "msclkid",
    // X (Twitter), TikTok, LinkedIn, Snapchat, Yandex
    "twclid",
    "ttclid",
    "li_fat_id",
    "sccid",
    "yclid",
    "ysclid",
    // Mailchimp, HubSpot, Marketo, Vero, Oracle Eloqua
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "__hssc",
    "__hstc",
    "__hsfp",
    "hsctatracking",
    "mkt_tok",
    "vero_id",
    "vero_conv",
    "elqtrackid",
    "elqtrack",
    // Otros identificadores de clic y campañas
    "oly_anon_id",
    "oly_enc_id",
    "rb_clickid",
    "s_cid",
    "_openstat",
    "wickedid",
    "irclickid",
    "epik",
    "ef_id",
    "cmpid",
    "ncid",
    "spm",
];

/// Prefijos de parámetros de rastreo (`utm_source`, `utm_medium`, `pk_campaign`…).
const TRACKING_PREFIXES: &[&str] = &["utm_", "pk_", "mtm_", "hsa_", "trk_"];

/// ¿Es `name` un parámetro de rastreo conocido?
pub fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    TRACKING_PARAMS.contains(&lower.as_str())
        || TRACKING_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// Devuelve la URL sin parámetros de rastreo, o `None` si no había ninguno.
pub fn strip_tracking_params(url: &Url) -> Option<Url> {
    url.query()?;

    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let kept: Vec<&(String, String)> = pairs
        .iter()
        .filter(|(k, _)| !is_tracking_param(k))
        .collect();

    if kept.len() == pairs.len() {
        return None;
    }

    let mut clean = url.clone();
    if kept.is_empty() {
        clean.set_query(None);
    } else {
        clean.query_pairs_mut().clear().extend_pairs(kept);
    }
    Some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(s: &str) -> Option<String> {
        strip_tracking_params(&Url::parse(s).expect("url")).map(|u| u.to_string())
    }

    #[test]
    fn removes_known_params() {
        assert_eq!(
            clean("https://example.com/p?utm_source=news&utm_medium=email&id=42"),
            Some("https://example.com/p?id=42".into())
        );
        assert_eq!(
            clean("https://example.com/?fbclid=abc"),
            Some("https://example.com/".into())
        );
        assert_eq!(
            clean("https://shop.example/item?GCLID=1&color=red#reviews"),
            Some("https://shop.example/item?color=red#reviews".into())
        );
    }

    #[test]
    fn leaves_clean_urls_alone() {
        assert_eq!(clean("https://example.com/search?q=rust"), None);
        assert_eq!(clean("https://example.com/"), None);
    }

    #[test]
    fn does_not_strip_lookalikes() {
        // "utmost" no empieza por "utm_"
        assert_eq!(clean("https://example.com/?utmost=1"), None);
    }
}
