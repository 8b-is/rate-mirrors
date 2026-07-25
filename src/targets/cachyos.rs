use crate::config::{AppError, FetchMirrors, LogFormatter};
use crate::countries::Country;
use crate::mirror::Mirror;
use crate::sources::{fetch_source_text, LocalSource, MirrorSourceChain};
use crate::target_configs::cachyos::CachyOSTarget;
use std::fmt::Display;
use std::sync::mpsc;
use url::Url;

/// Parse `code=XX` from CachyOS metadata comments like:
///   `## tier=1 code=US`
///   `## tier=2 code=RU`
/// Returns None for GLOBAL / unknown / missing codes.
fn parse_country_code_from_line(line: &str) -> Option<&'static Country> {
    let code = line
        .split_whitespace()
        .find_map(|token| token.strip_prefix("code="))?;
    // ISO 3166-1 alpha-2 only; skip CDN pseudo-codes like GLOBAL
    if code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
        Country::from_str(code)
    } else {
        None
    }
}

// The packaged list is preferred over fetching from GitHub when it is still the
// one CachyOS shipped: it is signed as part of the package and needs no network.
// `cachyos-rate-mirrors` writes our ranked output back over that same path, so
// the `code=` marker is what tells the two apart -- without it we would re-rank
// only the survivors of the previous run, shrinking the pool every time.
static LOCAL_SOURCES: &[LocalSource] = &[
    LocalSource::curated("/etc/rate-mirrors/sources/cachyos-mirrorlist.txt"),
    LocalSource::packaged("/etc/pacman.d/cachyos-mirrorlist", "code="),
];

static SOURCE: MirrorSourceChain = MirrorSourceChain {
    local: LOCAL_SOURCES,
    remote: "https://raw.githubusercontent.com/CachyOS/CachyOS-PKGBUILDS/master/cachyos-mirrorlist/cachyos-mirrorlist",
};

impl LogFormatter for CachyOSTarget {
    fn format_comment(&self, message: impl Display) -> String {
        format!("{}{}", self.comment_prefix, message)
    }

    fn format_mirror(&self, mirror: &Mirror) -> String {
        let arch = if self.arch == "auto" {
            "$arch"
        } else {
            &self.arch
        };

        format!("Server = {}{}/$repo", mirror.url, arch)
    }
}

impl FetchMirrors for CachyOSTarget {
    fn fetch_mirrors(
        &self,
        tx_progress: mpsc::Sender<String>,
        source_security: &crate::config::SourceSecurityConfig,
    ) -> Result<Vec<Mirror>, AppError> {
        let output = fetch_source_text(
            self.mirror_list_file.as_deref(),
            &SOURCE,
            self.fetch_mirrors_timeout,
            source_security,
            &tx_progress,
        )?;

        let mut current_country = None;
        let mut mirrors = Vec::new();

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Metadata comments carry country: `## tier=2 code=US`
            if trimmed.starts_with('#') {
                if let Some(country) = parse_country_code_from_line(trimmed) {
                    current_country = Some(country);
                } else if trimmed.contains("code=GLOBAL") {
                    // Explicit CDN / worldwide hosts stay unlabeled
                    current_country = None;
                }
                // Other comments (attribution, disabled servers) leave country sticky
                // until the next code= line — matches how CachyOS groups Server lines.
                continue;
            }

            let Some(rest) = trimmed.strip_prefix("Server = ") else {
                continue;
            };
            let cleaned = rest.replace("$arch/$repo", "");

            match Url::parse(&cleaned) {
                Ok(url) => {
                    let url_to_test = url
                        .join(&self.path_to_test)
                        .expect("failed to join path_to_test");
                    mirrors.push(Mirror {
                        country: current_country,
                        url,
                        url_to_test,
                    });
                }
                Err(e) => {
                    tx_progress
                        .send(format!(
                            "cachyos: skipping unparseable URL {}: {}",
                            cleaned, e
                        ))
                        .ok();
                }
            }
        }

        Ok(mirrors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_country_codes() {
        assert_eq!(
            parse_country_code_from_line("## tier=1 code=US").map(|c| c.code),
            Some("US")
        );
        assert_eq!(
            parse_country_code_from_line("## tier=2 code=RU").map(|c| c.code),
            Some("RU")
        );
        assert_eq!(
            parse_country_code_from_line("## tier=1 code=AT").map(|c| c.code),
            Some("AT")
        );
    }

    #[test]
    fn ignores_global_and_junk() {
        assert!(parse_country_code_from_line("## tier=1 code=GLOBAL").is_none());
        assert!(parse_country_code_from_line("## USA Mirror much thanks to Soulharsh!").is_none());
        assert!(parse_country_code_from_line("Server = https://example/").is_none());
    }
}
