//! Mirror-source resolution.
//!
//! Every target resolves its mirror list through an ordered chain:
//!
//!   1. an explicit `--mirror-source` / `--mirror-list-file` (or its env var)
//!   2. local files, most trusted first: an admin-curated file under
//!      `/etc/rate-mirrors/sources/`, then the distro's own packaged
//!      mirrorlist where the on-disk format matches what the target parses
//!   3. the upstream URL the distro publishes
//!
//! Local files win so an administrator can pin exactly which mirrors a machine
//! is willing to talk to, and so ranking still works on a host with no route to
//! GitHub. The remote step is a fallback, not a requirement -- an absent local
//! file is the normal case, not an error. Distro installers invoke this tool
//! non-interactively with no flags, so the zero-config path has to work.
//!
//! `--no-remote-sources` turns the chain into local-only and makes a missing
//! local file fatal, for hosts that must never fetch a mirror list.

use crate::config::{fetch_text, verify_source_integrity, AppError, SourceSecurityConfig};
use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use url::Url;

/// A local candidate within a source chain.
pub struct LocalSource {
    pub path: &'static str,

    /// Marker the file must contain before we trust it as a source.
    ///
    /// Some candidates sit at paths rate-mirrors itself writes -- the CachyOS
    /// wrapper rewrites `/etc/pacman.d/cachyos-mirrorlist` with our ranked
    /// output. Reading one of those back would rank only the handful of mirrors
    /// the previous run kept, narrowing the pool a little more on every run.
    /// Requiring a marker that only the pristine file carries breaks the loop:
    /// once the file has been rewritten, the marker is gone and we fall through
    /// to the next candidate.
    pub must_contain: Option<&'static str>,
}

impl LocalSource {
    /// A file only ever written by an administrator, so it is taken as-is.
    pub const fn curated(path: &'static str) -> Self {
        Self {
            path,
            must_contain: None,
        }
    }

    /// A distro-packaged file that our own output may have replaced.
    pub const fn packaged(path: &'static str, must_contain: &'static str) -> Self {
        Self {
            path,
            must_contain: Some(must_contain),
        }
    }
}

pub struct MirrorSourceChain {
    pub local: &'static [LocalSource],
    pub remote: &'static str,
}

/// Resolve a target's mirror source and return its contents.
pub fn fetch_source_text(
    explicit: Option<&str>,
    chain: &MirrorSourceChain,
    timeout_ms: u64,
    security: &SourceSecurityConfig,
    tx_progress: &mpsc::Sender<String>,
) -> Result<String, AppError> {
    let (origin, content) = resolve_source(explicit, chain, timeout_ms, security, tx_progress)?;
    verify_source_integrity(&origin, &content, security.mirror_source_sha256.as_deref())?;
    tx_progress.send(format!("MIRROR SOURCE: {}", origin)).ok();
    Ok(content)
}

/// Same as [`fetch_source_text`], for targets whose source is JSON.
pub fn fetch_source_json<T: DeserializeOwned>(
    explicit: Option<&str>,
    chain: &MirrorSourceChain,
    timeout_ms: u64,
    security: &SourceSecurityConfig,
    tx_progress: &mpsc::Sender<String>,
) -> Result<T, AppError> {
    let content = fetch_source_text(explicit, chain, timeout_ms, security, tx_progress)?;
    serde_json::from_str(&content)
        .map_err(|e| AppError::RequestError(format!("failed to decode JSON mirror source: {}", e)))
}

fn resolve_source(
    explicit: Option<&str>,
    chain: &MirrorSourceChain,
    timeout_ms: u64,
    security: &SourceSecurityConfig,
    tx_progress: &mpsc::Sender<String>,
) -> Result<(String, String), AppError> {
    // An explicit source is used exactly as given, with no fallback: the
    // operator named one source, so silently ranking a different one would be
    // worse than failing.
    if let Some(source) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let content = read_explicit(source, timeout_ms, security)?;
        return Ok((source.to_string(), content));
    }

    let mut tried: Vec<String> = Vec::new();
    for candidate in chain.local {
        match read_local_candidate(candidate) {
            Ok(content) => return Ok((format!("{} (local)", candidate.path), content)),
            Err(reason) => {
                tx_progress
                    .send(format!(
                        "MIRROR SOURCE: skipping {} ({})",
                        candidate.path, reason
                    ))
                    .ok();
                tried.push(format!("{} ({})", candidate.path, reason));
            }
        }
    }

    if !security.allow_remote_sources {
        return Err(AppError::NoLocalMirrorSource {
            remote: chain.remote.to_string(),
            tried: if tried.is_empty() {
                "no local candidates configured".to_string()
            } else {
                tried.join(", ")
            },
        });
    }

    let content = fetch_text(chain.remote, timeout_ms)?;
    Ok((format!("{} (remote)", chain.remote), content))
}

fn read_explicit(
    source: &str,
    timeout_ms: u64,
    security: &SourceSecurityConfig,
) -> Result<String, AppError> {
    if is_remote(source) {
        if !security.allow_remote_sources {
            return Err(AppError::RemoteSourcesDisabled(source.to_string()));
        }
        return fetch_text(source, timeout_ms);
    }

    fs::read_to_string(source).map_err(|e| {
        AppError::RequestError(format!("failed to read mirror source {}: {}", source, e))
    })
}

fn is_remote(source: &str) -> bool {
    Url::parse(source)
        .map(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or(false)
}

fn read_local_candidate(candidate: &LocalSource) -> Result<String, String> {
    if !Path::new(candidate.path).exists() {
        return Err("not present".to_string());
    }

    let content = fs::read_to_string(candidate.path).map_err(|e| format!("unreadable: {}", e))?;

    if content.trim().is_empty() {
        return Err("empty".to_string());
    }

    if let Some(marker) = candidate.must_contain {
        if !content.contains(marker) {
            return Err(format!(
                "no `{}` metadata, so it has been rewritten and is not a mirror source",
                marker
            ));
        }
    }

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn security(allow_remote: bool) -> SourceSecurityConfig {
        SourceSecurityConfig {
            allow_remote_sources: allow_remote,
            mirror_source_sha256: None,
        }
    }

    fn write_temp(name: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn chain(local: &'static [LocalSource]) -> MirrorSourceChain {
        MirrorSourceChain {
            local,
            remote: "https://example.invalid/mirrorlist",
        }
    }

    #[test]
    fn missing_local_candidates_fall_through_to_remote() {
        static LOCAL: &[LocalSource] = &[LocalSource::curated("/nonexistent/rate-mirrors/source")];
        let (tx, _rx) = mpsc::channel();

        // Resolution reaches the remote step; the fetch itself fails because the
        // host does not resolve, which is what proves we got past the local ones.
        let err = resolve_source(None, &chain(LOCAL), 500, &security(true), &tx).unwrap_err();
        assert!(
            !matches!(err, AppError::NoLocalMirrorSource { .. }),
            "expected to reach the remote fallback, got {:?}",
            err
        );
    }

    #[test]
    fn local_candidate_wins_over_remote() {
        let path = write_temp(
            "rate-mirrors-chain-local.txt",
            "Server = https://a.example/\n",
        );
        let local: &'static [LocalSource] = Box::leak(Box::new([LocalSource::curated(Box::leak(
            path.clone().into_boxed_str(),
        ))]));
        let (tx, _rx) = mpsc::channel();

        let (origin, content) =
            resolve_source(None, &chain(local), 500, &security(true), &tx).unwrap();

        assert!(origin.ends_with("(local)"), "origin was {}", origin);
        assert!(content.contains("a.example"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rewritten_packaged_file_is_skipped() {
        // What /etc/pacman.d/cachyos-mirrorlist looks like after the wrapper has
        // written our own ranked output over it: Server lines, no country codes.
        let path = write_temp(
            "rate-mirrors-chain-rewritten.txt",
            "# FINISHED AT: whenever\nServer = https://survivor.example/$arch/$repo\n",
        );
        let local: &'static [LocalSource] = Box::leak(Box::new([LocalSource::packaged(
            Box::leak(path.clone().into_boxed_str()),
            "code=",
        )]));
        let (tx, _rx) = mpsc::channel();

        let err = resolve_source(None, &chain(local), 500, &security(false), &tx).unwrap_err();

        match err {
            AppError::NoLocalMirrorSource { tried, .. } => {
                assert!(tried.contains("rewritten"), "tried was {}", tried)
            }
            other => panic!("expected NoLocalMirrorSource, got {:?}", other),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pristine_packaged_file_is_accepted() {
        let path = write_temp(
            "rate-mirrors-chain-pristine.txt",
            "## tier=1 code=US\nServer = https://us.example/$arch/$repo\n",
        );
        let local: &'static [LocalSource] = Box::leak(Box::new([LocalSource::packaged(
            Box::leak(path.clone().into_boxed_str()),
            "code=",
        )]));
        let (tx, _rx) = mpsc::channel();

        let (origin, _) = resolve_source(None, &chain(local), 500, &security(false), &tx).unwrap();

        assert!(origin.ends_with("(local)"), "origin was {}", origin);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn empty_local_file_is_skipped() {
        let path = write_temp("rate-mirrors-chain-empty.txt", "   \n");
        let local: &'static [LocalSource] = Box::leak(Box::new([LocalSource::curated(Box::leak(
            path.clone().into_boxed_str(),
        ))]));
        let (tx, _rx) = mpsc::channel();

        let err = resolve_source(None, &chain(local), 500, &security(false), &tx).unwrap_err();

        match err {
            AppError::NoLocalMirrorSource { tried, .. } => {
                assert!(tried.contains("empty"), "tried was {}", tried)
            }
            other => panic!("expected NoLocalMirrorSource, got {:?}", other),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_remote_sources_fails_closed_instead_of_fetching() {
        static LOCAL: &[LocalSource] = &[LocalSource::curated("/nonexistent/rate-mirrors/source")];
        let (tx, _rx) = mpsc::channel();

        let err = resolve_source(None, &chain(LOCAL), 500, &security(false), &tx).unwrap_err();

        assert!(matches!(err, AppError::NoLocalMirrorSource { .. }));
    }

    #[test]
    fn explicit_local_source_does_not_fall_back() {
        static LOCAL: &[LocalSource] = &[];
        let (tx, _rx) = mpsc::channel();

        let err = resolve_source(
            Some("/nonexistent/explicit-source.txt"),
            &chain(LOCAL),
            500,
            &security(true),
            &tx,
        )
        .unwrap_err();

        assert!(
            matches!(err, AppError::RequestError(_)),
            "explicit sources must fail rather than silently use another source"
        );
    }

    #[test]
    fn explicit_remote_source_is_blocked_under_no_remote_sources() {
        static LOCAL: &[LocalSource] = &[];
        let (tx, _rx) = mpsc::channel();

        let err = resolve_source(
            Some("https://example.com/mirrorlist"),
            &chain(LOCAL),
            500,
            &security(false),
            &tx,
        )
        .unwrap_err();

        assert!(matches!(err, AppError::RemoteSourcesDisabled(_)));
    }

    #[test]
    fn paths_are_not_mistaken_for_urls() {
        assert!(!is_remote("/etc/pacman.d/cachyos-mirrorlist"));
        assert!(!is_remote("mirrorlist.txt"));
        assert!(is_remote("https://example.com/mirrorlist"));
        assert!(is_remote("http://example.com/mirrorlist"));
    }
}
