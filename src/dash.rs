//! Server-side DASH manifest rewriting.
//!
//! Every `<BaseURL>` in an upstream MPD is replaced with a same-origin
//! `/api/dash/segment` URL. The browser therefore only ever talks to
//! SourceTube, which kills YouTube embed error 153 at its root.

use regex::Regex;
use std::sync::OnceLock;
use url::Url;

static BASE_URL: OnceLock<Regex> = OnceLock::new();

fn base_url_re() -> &'static Regex {
    BASE_URL.get_or_init(|| {
        Regex::new(r"(?is)(<BaseURL\b[^>]*>)([^<]*)(</BaseURL>)").expect("valid BaseURL regex")
    })
}

/// Percent-encode an upstream URL into the local segment proxy path.
pub fn segment_path(id: &str, upstream: &Url) -> String {
    let mut local = Url::parse("http://sourcetube.local/api/dash/segment").expect("valid base");
    local
        .query_pairs_mut()
        .append_pair("id", id)
        .append_pair("u", upstream.as_str());
    format!(
        "{}{}",
        local.path(),
        local
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    )
}

/// Resolve a manifest URL, which Invidious may return as a relative path.
pub fn absolute_manifest(raw: &str, instance: &str) -> Result<Url, String> {
    let base = Url::parse(instance).map_err(|error| error.to_string())?;
    base.join(raw).map_err(|error| error.to_string())
}

/// MPD is XML, so upstream `<BaseURL>` text can contain `&amp;` even though
/// the real media URL needs a plain `&`. Decode the standard XML entities
/// before resolving or proxying the URL.
fn decode_xml_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#38;", "&")
        .replace("&amp;", "&")
}

/// The generated BaseURL is written back into XML text. Keep the document
/// well-formed by escaping the bare `&` that separates query parameters.
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Rewrite all BaseURL elements of an MPD to point at the local proxy.
pub fn rewrite(mpd: &str, manifest: &Url, id: &str) -> String {
    let mut directory_base = manifest.clone();
    if !directory_base.path().ends_with('/') {
        if let Some((prefix, _)) = directory_base.path().rsplit_once('/') {
            let mut next = directory_base.clone();
            next.set_path(&format!("{prefix}/"));
            directory_base = next;
        }
    }

    base_url_re()
        .replace_all(mpd, |caps: &regex::Captures| {
            let raw = caps
                .get(2)
                .map(|match_| match_.as_str().trim())
                .unwrap_or("");
            if raw.is_empty() {
                return caps[0].to_string();
            }
            let value = decode_xml_entities(raw);
            let resolved = directory_base
                .join(&value)
                .or_else(|_| manifest.join(&value))
                .ok();
            let Some(resolved) = resolved else {
                return caps[0].to_string();
            };
            // A trailing slash denotes a base directory for relative entries
            // that follow it in the same MPD.
            if value.ends_with('/') {
                directory_base = resolved.clone();
            }
            format!(
                "{}{}{}",
                &caps[1],
                escape_xml(&segment_path(id, &resolved)),
                &caps[3]
            )
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_absolute_and_relative_base_urls() {
        let manifest = Url::parse("https://inv.example/videoplayback.mpd?itag=1").unwrap();
        let mpd = r#"<?xml version="1.0"?>
<MPD><BaseURL>https://rr1.googlevideo.com/video/abc/</BaseURL>
<Period><AdaptationSet><Representation><BaseURL>seg-1.m4s</BaseURL></Representation></AdaptationSet></Period></MPD>"#;
        let rewritten = rewrite(mpd, &manifest, "abcdefghijk");
        assert!(!rewritten.contains("https://rr1.googlevideo.com"));
        assert!(
            rewritten
                .matches("/api/dash/segment?id=abcdefghijk")
                .count()
                == 2
        );
        assert!(rewritten.contains("rr1.googlevideo.com%2Fvideo%2Fabc%2Fseg-1.m4s"));
    }

    #[test]
    fn resolves_relative_manifest_against_instance() {
        let url =
            absolute_manifest("/api/manifest/dash/id/abcdefghijk", "https://inv.example").unwrap();
        assert_eq!(
            url.as_str(),
            "https://inv.example/api/manifest/dash/id/abcdefghijk"
        );
    }

    #[test]
    fn segment_path_percent_encodes_query_characters() {
        let url = Url::parse("https://host/video?a=1&b=two%20words").unwrap();
        let path = segment_path("abcdefghijk", &url);
        assert!(path.contains("u=https%3A%2F%2Fhost%2Fvideo%3Fa%3D1%26b%3Dtwo"));
        assert!(path.contains("id=abcdefghijk"));
    }

    #[test]
    fn decodes_upstream_entities_and_emits_well_formed_xml() {
        let manifest = Url::parse("https://inv.example/api/manifest/dash/id/abcdefghijk").unwrap();
        let mpd = r#"<MPD><BaseURL>https://rr1.googlevideo.com/video?a=1&amp;b=2</BaseURL></MPD>"#;
        let rewritten = rewrite(mpd, &manifest, "abcdefghijk");
        // The upstream `&amp;` must not leak into the encoded segment URL.
        assert!(!rewritten.contains("%26amp%3B"));
        assert!(rewritten.contains("a%3D1%26b%3D2"));
        // The generated BaseURL must remain valid XML.
        assert!(rewritten.contains("?id=abcdefghijk&amp;u="));
        assert!(!rewritten.contains("?id=abcdefghijk&u="));
    }
}
