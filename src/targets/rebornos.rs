use crate::config::{AppError, FetchMirrors, LogFormatter};
use crate::mirror::Mirror;
use crate::sources::{fetch_source_text, LocalSource, MirrorSourceChain};
use crate::target_configs::rebornos::RebornOSTarget;
use std::fmt::Display;
use std::sync::mpsc;
use url::Url;

static LOCAL_SOURCES: &[LocalSource] = &[LocalSource::curated(
    "/etc/rate-mirrors/sources/rebornos-mirrorlist.txt",
)];

static SOURCE: MirrorSourceChain = MirrorSourceChain {
    local: LOCAL_SOURCES,
    remote:
        "https://raw.githubusercontent.com/RebornOS-Team/rebornos-mirrorlist/main/reborn-mirrorlist",
};

impl LogFormatter for RebornOSTarget {
    fn format_comment(&self, message: impl Display) -> String {
        format!("{}{}", self.comment_prefix, message)
    }

    fn format_mirror(&self, mirror: &Mirror) -> String {
        format!("Server = {}", mirror.url)
    }
}

impl FetchMirrors for RebornOSTarget {
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

        let urls: Vec<Url> = output
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| line.replace("Server = ", ""))
            .filter(|line| !line.is_empty())
            .filter_map(|line| Url::parse(&line).ok())
            .collect();

        let mirrors: Vec<_> = urls
            .into_iter()
            .map(|url| Mirror {
                country: None,
                url_to_test: url
                    .join(&self.path_to_test)
                    .expect("failed to join path-to-test"),
                url,
            })
            .collect();

        Ok(mirrors)
    }
}
