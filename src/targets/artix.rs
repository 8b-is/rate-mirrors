use crate::config::{AppError, FetchMirrors, LogFormatter};
use crate::countries::Country;
use crate::mirror::Mirror;
use crate::sources::{fetch_source_text, LocalSource, MirrorSourceChain};
use crate::target_configs::artix::ArtixTarget;
use std::fmt::Display;
use std::sync::mpsc;
use url::Url;

static LOCAL_SOURCES: &[LocalSource] = &[LocalSource::curated(
    "/etc/rate-mirrors/sources/artix-mirrorlist.txt",
)];

static SOURCE: MirrorSourceChain = MirrorSourceChain {
    local: LOCAL_SOURCES,
    remote: "https://packages.artixlinux.org/mirrorlist/all/",
};

impl LogFormatter for ArtixTarget {
    fn format_comment(&self, message: impl Display) -> String {
        format!("{}{}", self.comment_prefix, message)
    }

    fn format_mirror(&self, mirror: &Mirror) -> String {
        format!("Server = {}$repo/os/$arch", mirror.url)
    }
}

impl FetchMirrors for ArtixTarget {
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
            let trimmed = line.trim_start();

            if trimmed.starts_with("##") {
                let country_name = trimmed
                    .trim_start_matches('#')
                    .trim_start_matches('#')
                    .trim_start();
                current_country = Country::from_str(country_name);
                continue;
            }

            let uncommented = trimmed.trim_start_matches('#').trim_start();
            if !uncommented.starts_with("Server = ") {
                continue;
            }

            let cleaned = uncommented
                .trim_start_matches("Server = ")
                .replace("$repo/os/$arch", "");

            if cleaned.is_empty() {
                continue;
            }

            if let Ok(url) = Url::parse(&cleaned) {
                mirrors.push(Mirror {
                    country: current_country,
                    url_to_test: url
                        .join(&self.path_to_test)
                        .expect("failed to join path_to_test"),
                    url,
                });
            }
        }

        Ok(mirrors)
    }
}
