use crate::config::{AppError, FetchMirrors, LogFormatter};
use crate::mirror::Mirror;
use crate::sources::{fetch_source_text, LocalSource, MirrorSourceChain};
use crate::target_configs::archarm::ArcharmTarget;
use std::fmt::Display;
use std::sync::mpsc;
use url::Url;

static LOCAL_SOURCES: &[LocalSource] = &[LocalSource::curated(
    "/etc/rate-mirrors/sources/archarm-mirrorlist.txt",
)];

static SOURCE: MirrorSourceChain = MirrorSourceChain {
    local: LOCAL_SOURCES,
    remote: "https://raw.githubusercontent.com/archlinuxarm/PKGBUILDs/master/core/pacman-mirrorlist/mirrorlist",
};

impl LogFormatter for ArcharmTarget {
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

impl FetchMirrors for ArcharmTarget {
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

        let urls = output
            .lines()
            .filter_map(|line| {
                if line.starts_with("# Server = ") {
                    Some(line.replace("# Server = ", ""))
                } else if line.starts_with("Server = ") {
                    Some(line.replace("Server = ", ""))
                } else {
                    None
                }
            })
            .filter_map(|line| Url::parse(&line.replace("$arch/$repo", "")).ok());
        let result: Vec<_> = urls
            .map(|url| {
                let url_to_test = url
                    .join(&self.path_to_test)
                    .expect("failed to join path_to_test");
                Mirror {
                    country: None,
                    url,
                    url_to_test,
                }
            })
            .collect();

        Ok(result)
    }
}
