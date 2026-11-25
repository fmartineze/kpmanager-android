//! Aviso de versiones nuevas: consulta las releases públicas del repositorio en GitHub.
//!
//! Es una petición GET anónima (sin datos del usuario, con un agente genérico) que hace Rust, no la interfaz:
//! así la política de contenido de la interfaz no tiene que permitir conexiones a ningún sitio. Solo se
//! devuelven enlaces del propio repositorio.

use serde::Serialize;

use crate::error::{AppError, Result};

const REPO: &str = "fmartineze/kpmanager-android";
/// Las versiones de Android se publican como preliminares: `releases/latest` no las devuelve.
const API: &str = "https://api.github.com/repos/fmartineze/kpmanager-android/releases?per_page=10";

#[derive(Serialize, Debug, PartialEq)]
pub struct Release {
    /// Sin la «v» inicial: `0.4.3`.
    pub version: String,
    pub url: String,
}

/// `x.y.z` con sufijo de preversión opcional (`-beta.1`). `None` si no es una versión.
fn parse(raw: &str) -> Option<([u64; 3], String)> {
    let s = raw.trim().trim_start_matches('v');
    let s = s.split('+').next()?;
    let (nums, pre) = match s.split_once('-') {
        Some((n, p)) => (n, p.to_owned()),
        None => (s, String::new()),
    };
    let mut out = [0u64; 3];
    let parts: Vec<&str> = nums.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.parse().ok()?;
    }
    Some((out, pre))
}

/// `latest` es posterior a `current`. Una preversión es anterior a su versión estable; lo que no se entiende
/// no se trata como más nuevo.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some((a, pa)), Some((b, pb))) = (parse(latest), parse(current)) else { return false };
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (pa.is_empty(), pb.is_empty()) {
            (true, false) => true,
            (false, false) => pa > pb,
            _ => false,
        },
    }
}

fn safe_url(url: &str) -> String {
    let prefix = format!("https://github.com/{REPO}/");
    if url.starts_with(&prefix) { url.to_owned() } else { format!("{prefix}releases") }
}

/// La versión más alta publicada (sin borradores), si es más nueva que la instalada.
pub fn newest(releases: &serde_json::Value, current: &str) -> Option<Release> {
    releases
        .as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(true))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            parse(tag)?;
            Some(Release { version: tag.trim_start_matches('v').to_owned(), url: safe_url(r["html_url"].as_str().unwrap_or("")) })
        })
        .filter(|r| is_newer(&r.version, current))
        .reduce(|a, b| if is_newer(&b.version, &a.version) { b } else { a })
}

pub async fn check(http: &reqwest::Client) -> Result<Option<Release>> {
    let resp = http
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "KP-Manager-Android")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::msg("updateFailed"));
    }
    let v: serde_json::Value = resp.json().await?;
    Ok(newest(&v, env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_semver() {
        assert!(is_newer("0.4.3", "0.4.2"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"), "la estable va después de su preversión");
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("0.4.2", "0.4.2"));
        assert!(!is_newer("basura", "0.1.0"));
    }

    #[test]
    fn newest_skips_drafts_and_foreign_links() {
        let v = serde_json::json!([
            { "tag_name": "v0.5.0", "draft": true, "html_url": "https://github.com/fmartineze/kpmanager-android/releases/tag/v0.5.0" },
            { "tag_name": "v0.4.4", "draft": false, "html_url": "https://evil.example/x" },
            { "tag_name": "v0.4.3", "draft": false, "html_url": "https://github.com/fmartineze/kpmanager-android/releases/tag/v0.4.3" },
            { "tag_name": "v0.4.2", "draft": false, "html_url": "" }
        ]);
        let r = newest(&v, "0.4.2").unwrap();
        assert_eq!(r.version, "0.4.4");
        assert_eq!(r.url, "https://github.com/fmartineze/kpmanager-android/releases", "enlace ajeno: el de las releases");
        assert!(newest(&v, "0.4.4").is_none());
        assert!(newest(&serde_json::json!({"message": "rate limit"}), "0.1.0").is_none());
    }
}
