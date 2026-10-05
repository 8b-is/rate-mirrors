use crate::mirror::Mirror;
use crate::target_configs::arch4edu::Arch4eduTarget;
use crate::target_configs::archarm::ArcharmTarget;
use crate::target_configs::archlinux::ArchTarget;
use crate::target_configs::archlinuxcn::ArchCNTarget;
use crate::target_configs::arcolinux::ArcoLinuxTarget;
use crate::target_configs::artix::ArtixTarget;
use crate::target_configs::blackarch::BlackArchTarget;
use crate::target_configs::cachyos::CachyOSTarget;
use crate::target_configs::chaotic::ChaoticTarget;
// use crate::target_configs::debian::DebianTarget;
use crate::target_configs::endeavouros::EndeavourOSTarget;
use crate::target_configs::manjaro::ManjaroTarget;
use crate::target_configs::openbsd::OpenBSDTarget;
use crate::target_configs::rebornos::RebornOSTarget;
use crate::target_configs::stdin::StdinTarget;
// use crate::target_configs::ubuntu::UbuntuTarget;
use ambassador::{delegatable_trait, Delegate};
use clap::{Parser, Subcommand};
use openssl::sha::sha256;
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::sync::mpsc;
use std::time::Duration;
use thiserror::Error;
use tokio::runtime::Runtime;
use url::Url;

#[derive(Debug, PartialEq, Clone)]
pub enum Protocol {
    Http,
    Https,
}

impl FromStr for Protocol {
    type Err = &'static str;
    fn from_str(protocol: &str) -> Result<Self, Self::Err> {
        match protocol {
            "http" => Ok(Protocol::Http),
            "https" => Ok(Protocol::Https),
            _ => Err("could not parse protocol"),
        }
    }
}

#[derive(Error)]
pub enum AppError {
    #[error("do not run rate-mirrors with root permissions")]
    Root,
    #[error("failed to connect to {0}, consider increasing fetch-mirrors-timeout")]
    RequestTimeout(String),
    #[error("{0}")]
    RequestError(String),
    #[error("HTTP {status} from {url}")]
    HttpError { status: u16, url: String },
    #[error("--no-remote-sources is set, so {0} cannot be fetched")]
    RemoteSourcesDisabled(String),
    #[error(
        "--no-remote-sources is set and no local mirror source was usable (tried: {tried}); \
         install one of those files, or drop --no-remote-sources to fall back to {remote}"
    )]
    NoLocalMirrorSource { remote: String, tried: String },
    #[error("invalid --mirror-source-sha256 value: expected 64 hex chars")]
    InvalidSourceHash,
    #[error(
        "mirror source integrity check failed for {path_or_url}: expected {expected}, got {actual}"
    )]
    SourceIntegrityMismatch {
        path_or_url: String,
        expected: String,
        actual: String,
    },
    #[error("no mirrors after filtering")]
    NoMirrorsAfterFiltering,
    #[error("all speed tests failed")]
    SpeedTestsFailed,
    #[error("no mirror output produced")]
    BlankOutput,
    #[error("stdout closed")]
    StdoutBrokenPipe,
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> AppError {
        if err.is_timeout() {
            AppError::RequestTimeout(err.url().map(|u| u.to_string()).unwrap_or_default())
        } else {
            AppError::RequestError(err.to_string())
        }
    }
}

#[delegatable_trait]
pub trait LogFormatter {
    fn format_comment(&self, message: impl fmt::Display) -> String;
    fn format_mirror(&self, mirror: &Mirror) -> String;
}

#[delegatable_trait]
pub trait FetchMirrors {
    fn fetch_mirrors(
        &self,
        tx_progress: mpsc::Sender<String>,
        source_security: &SourceSecurityConfig,
    ) -> Result<Vec<Mirror>, AppError>;
}

#[derive(Debug, Clone)]
pub struct SourceSecurityConfig {
    pub allow_remote_sources: bool,
    pub mirror_source_sha256: Option<String>,
}

#[derive(Debug, Subcommand, Clone, Delegate)]
#[delegate(FetchMirrors)]
#[delegate(LogFormatter)]
pub enum Target {
    /// accepts lines of urls OR lines with tab-separated urls and countries
    Stdin(StdinTarget),

    /// test archlinux mirrors
    Arch(ArchTarget),

    /// test arch4edu mirrors
    #[command(name = "arch4edu")]
    Arch4edu(Arch4eduTarget),

    /// test archlinuxcn mirrors
    #[command(name = "archlinuxcn")]
    ArchCN(ArchCNTarget),

    /// test archlinuxarm mirrors
    Archarm(ArcharmTarget),

    /// test artix mirrors
    Artix(ArtixTarget),

    /// test arcolinux mirrors
    #[command(name = "arcolinux")]
    ArcoLinux(ArcoLinuxTarget),

    /// test blackarch mirrors
    #[command(name = "blackarch")]
    BlackArch(BlackArchTarget),

    /// test cachyos mirrors
    #[command(name = "cachyos")]
    CachyOS(CachyOSTarget),

    /// test chaotic-aur mirrors
    #[command(name = "chaotic-aur")]
    Chaotic(ChaoticTarget),

    /// test endeavouros mirrors
    #[command(name = "endeavouros")]
    EndeavourOS(EndeavourOSTarget),

    /// test manjaro mirrors
    Manjaro(ManjaroTarget),

    /// test OpenBSD mirrors
    #[command(name = "openbsd")]
    OpenBSD(OpenBSDTarget),

    /// test rebornos mirrors
    #[command(name = "rebornos")]
    RebornOS(RebornOSTarget),
}

fn parse_positive_usize(s: &str) -> Result<usize, String> {
    let n: usize = s.parse().map_err(|e| format!("{e}"))?;
    if n == 0 {
        return Err("value must be at least 1".into());
    }
    Ok(n)
}

#[derive(Debug, Parser)]
#[command(
    name = "rate-mirrors config",
    about,
    version,
    rename_all = "kebab-case",
    rename_all_env = "SCREAMING_SNAKE_CASE"
)]
pub struct Config {
    /// Per-mirror speed test timeout in milliseconds
    #[command(subcommand)]
    pub target: Target,

    /// Test only specified protocols (can be passed multiple times)
    #[arg(env = "RATE_MIRRORS_PROTOCOL", long = "protocol", name = "protocol")]
    pub protocols: Vec<Protocol>,

    /// Per-mirror speed test timeout in milliseconds
    #[arg(env = "RATE_MIRRORS_PER_MIRROR_TIMEOUT", long, default_value = "8000")]
    pub per_mirror_timeout: u64,

    /// Minimum downloading time, required to measure mirror speed,
    /// in milliseconds
    #[arg(env = "RATE_MIRRORS_MIN_PER_MIRROR", long, default_value = "300")]
    pub min_per_mirror: u64,

    /// Maximum downloading time, required to measure mirror speed,
    /// in milliseconds
    #[arg(env = "RATE_MIRRORS_MAX_PER_MIRROR", long, default_value = "1000")]
    pub max_per_mirror: u64,

    /// Minimum number of bytes to be downloaded,
    /// required to measure mirror speed
    #[arg(
        env = "RATE_MIRRORS_MIN_BYTES_PER_MIRROR",
        long,
        default_value = "70000"
    )]
    pub min_bytes_per_mirror: usize,

    /// Per-mirror: sigma to mean speed ratio
    ///
    ///   1.0 -- 68% probability (1 sigma), no 100% error
    ///   0.5 -- 68% probability (1 sigma), no 50% error;
    ///   0.25 -- 68% probability (1 sigma), no 25% error;
    ///   0.125 -- 95% probability (2 sigmas), no 25% error;
    ///   0.0625 -- 95% probability (2 sigmas), no 12.5% error:
    #[arg(
        env = "RATE_MIRRORS_EPS",
        long,
        default_value = "0.0625",
        verbatim_doc_comment
    )]
    pub eps: f64,

    /// Per-mirror: after min measurement time elapsed, check such number of
    /// subsequently downloaded data chunks whether speed variations are less
    /// then "eps"
    #[arg(env = "RATE_MIRRORS_EPS_CHECKS", long, default_value = "40")]
    pub eps_checks: usize,

    /// Number of simultaneous speed tests
    #[arg(env = "RATE_MIRRORS_CONCURRENCY", long, default_value = "16")]
    pub concurrency: usize,

    /// Number of simultaneous speed tests for mirrors with unknown country
    #[arg(
        env = "RATE_MIRRORS_CONCURRENCY_FOR_UNLABELED",
        long,
        default_value = "40"
    )]
    pub concurrency_for_unlabeled: usize,

    /// Max number of jumps between countries, when finding top mirrors
    #[arg(env = "RATE_MIRRORS_MAX_JUMPS", long, default_value = "7")]
    pub max_jumps: usize,

    /// Entry country - first country (+ its neighbours) to test.
    /// You don't need to change it unless you are just curious.
    #[arg(
        env = "RATE_MIRRORS_ENTRY_COUNTRY",
        long,
        default_value = "US",
        verbatim_doc_comment
    )]
    pub entry_country: String,

    /// Exclude countries from mirror selection (comma-separated 2-letter ISO country codes).
    /// Use ZZ to filter out mirrors with undefined country.
    #[arg(
        env = "RATE_MIRRORS_EXCLUDE_COUNTRIES",
        long = "exclude-countries",
        name = "country-codes",
        verbatim_doc_comment
    )]
    pub exclude_countries: Option<String>,

    /// Neighbor country to test per country
    #[arg(
        env = "RATE_MIRRORS_COUNTRY_NEIGHBORS_PER_COUNTRY",
        long,
        default_value = "3"
    )]
    pub country_neighbors_per_country: usize,

    /// Number of mirrors to test per country
    #[arg(
        env = "RATE_MIRRORS_COUNTRY_TEST_MIRRORS_PER_COUNTRY",
        long,
        default_value = "2"
    )]
    pub country_test_mirrors_per_country: usize,

    /// Number of top mirrors to retest
    #[arg(
        env = "RATE_MIRRORS_TOP_MIRRORS_NUMBER_TO_RETEST",
        long,
        default_value = "5"
    )]
    pub top_mirrors_number_to_retest: usize,

    /// Max number of mirrors to output
    #[arg(env = "RATE_MIRRORS_MAX_MIRRORS_TO_OUTPUT", long, value_parser = parse_positive_usize)]
    pub max_mirrors_to_output: Option<usize>,

    /// Filename to save the output to in case of success
    #[arg(env = "RATE_MIRRORS_SAVE", long = "save", verbatim_doc_comment)]
    pub save_to_file: Option<String>,

    /// Allow running by root
    #[arg(env = "RATE_MIRRORS_ALLOW_ROOT", long)]
    pub allow_root: bool,

    /// Disable printing comments
    #[arg(env = "RATE_MIRRORS_DISABLE_COMMENTS", long)]
    pub disable_comments: bool,

    /// Disable printing comments to output file
    #[arg(env = "RATE_MIRRORS_DISABLE_COMMENTS_IN_FILE", long)]
    pub disable_comments_in_file: bool,

    /// Exit with error instead of outputting untested mirrors when all speed tests fail
    #[arg(env = "RATE_MIRRORS_DISABLE_UNTESTED_FALLBACK", long)]
    pub disable_untested_fallback: bool,

    /// Never fetch mirror lists over the network: use only local files, and
    ///   fail if none is usable. Mirror lists are otherwise read from local
    ///   files when present and fetched from the distro upstream if not.
    #[arg(env = "RATE_MIRRORS_NO_REMOTE_SOURCES", long, verbatim_doc_comment)]
    pub no_remote_sources: bool,

    /// Deprecated no-op: fetching a mirror list upstream is the default
    /// fallback. Kept so existing wrappers and scripts keep parsing.
    #[arg(env = "RATE_MIRRORS_ALLOW_REMOTE_SOURCES", long, hide = true)]
    pub allow_remote_sources: bool,

    /// Expected SHA-256 hex digest for the selected mirror source input
    #[arg(env = "RATE_MIRRORS_MIRROR_SOURCE_SHA256", long, value_name = "HEX64")]
    pub mirror_source_sha256: Option<String>,

    /// Skip cross-checking ranked mirrors against each other. By default the
    ///   repository database each mirror serves is fingerprinted, and a mirror
    ///   serving one no other mirror has is dropped from the output.
    #[arg(env = "RATE_MIRRORS_NO_VERIFY_MIRRORS", long, verbatim_doc_comment, conflicts_with_all = ["require_dnssec", "max_mirror_age"])]
    pub no_verify_mirrors: bool,

    /// Drop mirrors whose repository database was last synced more than this
    ///   many hours ago. Unset means staleness is reported but not enforced.
    #[arg(
        env = "RATE_MIRRORS_MAX_MIRROR_AGE",
        long,
        value_name = "HOURS",
        verbatim_doc_comment
    )]
    pub max_mirror_age: Option<f64>,

    /// Per-mirror timeout for verification requests, in milliseconds
    #[arg(env = "RATE_MIRRORS_VERIFY_TIMEOUT", long, default_value = "10000")]
    pub verify_timeout: u64,

    /// Skip the DNSSEC lookup. By default each mirror's hostname is checked
    ///   against a validating resolver over HTTPS, and mirrors in signed zones
    ///   are preferred over unsigned ones of comparable speed.
    #[arg(env = "RATE_MIRRORS_NO_DNSSEC_CHECK", long, verbatim_doc_comment)]
    pub no_dnssec_check: bool,

    /// Drop mirrors whose hostname is not in a DNSSEC-signed zone. Most distro
    ///   mirrors are still unsigned, so this discards much of the pool.
    #[arg(
        env = "RATE_MIRRORS_REQUIRE_DNSSEC",
        long,
        verbatim_doc_comment,
        conflicts_with = "no_dnssec_check"
    )]
    pub require_dnssec: bool,

    /// DNS-over-HTTPS endpoint used for the DNSSEC lookup. Queried out of band
    ///   so a subverted local resolver cannot vouch for itself.
    #[arg(
        env = "RATE_MIRRORS_DOH_RESOLVER",
        long,
        default_value = "https://cloudflare-dns.com/dns-query",
        verbatim_doc_comment
    )]
    pub doh_resolver: String,

    /// Pre-parsed set of excluded country codes (lowercase)
    #[arg(skip)]
    pub excluded_countries_set: HashSet<String>,
}

impl Config {
    pub fn allows_untested_fallback(&self) -> bool {
        !self.disable_untested_fallback && !self.require_dnssec && self.max_mirror_age.is_none()
    }

    pub fn new() -> Self {
        let mut config = Self::parse();
        config.excluded_countries_set = config
            .exclude_countries
            .as_ref()
            .map(|s| {
                s.split(',')
                    .map(|c| c.trim().to_ascii_lowercase())
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        config
    }

    pub fn is_country_excluded(&self, code: &str) -> bool {
        self.excluded_countries_set
            .contains(&code.to_ascii_lowercase())
    }

    pub fn is_protocol_allowed_for_url(&self, url: &Url) -> bool {
        if self.protocols.is_empty() {
            matches!(url.scheme(), "http" | "https")
        } else {
            url.scheme()
                .parse()
                .map(|p| self.protocols.contains(&p))
                .unwrap_or(false)
        }
    }

    pub fn source_security_config(&self) -> Result<SourceSecurityConfig, AppError> {
        let mirror_source_sha256 = self
            .mirror_source_sha256
            .as_ref()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());

        if let Some(ref hash) = mirror_source_sha256 {
            let is_valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            if !is_valid {
                return Err(AppError::InvalidSourceHash);
            }
        }

        Ok(SourceSecurityConfig {
            allow_remote_sources: !self.no_remote_sources,
            mirror_source_sha256,
        })
    }
}

pub fn default_client_builder() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .user_agent(format!(
            "{}/{}",
            env!("CARGO_PKG_NAME").replace('_', "-"),
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|e| AppError::RequestError(format!("failed to build HTTP client: {}", e)))
}

fn convert_reqwest_error(e: reqwest::Error, url: &str) -> AppError {
    if e.is_timeout() {
        AppError::RequestTimeout(url.to_string())
    } else {
        AppError::RequestError(format!("failed to connect to {}: {}", url, e))
    }
}

pub fn fetch_text(url: &str, timeout_ms: u64) -> Result<String, AppError> {
    let runtime = Runtime::new().unwrap();
    let result = runtime.block_on(async {
        let client = default_client_builder()?;
        let response = client
            .get(url)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|e| convert_reqwest_error(e, url))?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::HttpError {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }

        response.text_with_charset("utf-8").await.map_err(|e| {
            AppError::RequestError(format!("failed to read response from {}: {}", url, e))
        })
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

pub(crate) fn verify_source_integrity(
    path_or_url: &str,
    source_content: &str,
    expected_sha256: Option<&str>,
) -> Result<(), AppError> {
    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(source_content.as_bytes());
        if actual != expected {
            return Err(AppError::SourceIntegrityMismatch {
                path_or_url: path_or_url.to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    sha256(content)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::archlinux::{selected_source_chain, ARCH_SOURCE, ARCH_TIER_1_SOURCE};
    use clap::error::ErrorKind;
    use std::sync::Mutex;

    static MIRROR_SOURCE_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn parse_arch_with_mirror_source_env(
        env_value: Option<&str>,
        args: &[&str],
    ) -> Result<Config, clap::Error> {
        let _guard = MIRROR_SOURCE_ENV_LOCK.lock().unwrap();
        let old_value = std::env::var_os("RATE_MIRRORS_MIRROR_SOURCE");

        unsafe {
            match env_value {
                Some(value) => std::env::set_var("RATE_MIRRORS_MIRROR_SOURCE", value),
                None => std::env::remove_var("RATE_MIRRORS_MIRROR_SOURCE"),
            }
        }

        let result = Config::try_parse_from(args);

        unsafe {
            match old_value {
                Some(value) => std::env::set_var("RATE_MIRRORS_MIRROR_SOURCE", value),
                None => std::env::remove_var("RATE_MIRRORS_MIRROR_SOURCE"),
            }
        }

        result
    }

    fn arch_target(config: &Config) -> &ArchTarget {
        match &config.target {
            Target::Arch(target) => target,
            other => panic!("expected Arch target, got {other:?}"),
        }
    }

    #[test]
    fn arch_fetch_first_tier_only_succeeds_and_selects_tier_1_source() {
        let config = parse_arch_with_mirror_source_env(
            None,
            &["rate-mirrors", "arch", "--fetch-first-tier-only"],
        )
        .unwrap();

        assert_eq!(
            selected_source_chain(arch_target(&config)).remote,
            ARCH_TIER_1_SOURCE.remote
        );
    }

    #[test]
    fn arch_mirror_source_argument_succeeds() {
        let config = parse_arch_with_mirror_source_env(
            None,
            &[
                "rate-mirrors",
                "arch",
                "--mirror-source",
                "local-status.json",
            ],
        )
        .unwrap();

        assert_eq!(
            arch_target(&config).mirror_source.as_deref(),
            Some("local-status.json")
        );
    }

    #[test]
    fn arch_mirror_source_argument_conflicts_with_fetch_first_tier_only() {
        let err = parse_arch_with_mirror_source_env(
            None,
            &[
                "rate-mirrors",
                "arch",
                "--mirror-source",
                "local-status.json",
                "--fetch-first-tier-only",
            ],
        )
        .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn arch_mirror_source_env_conflicts_with_fetch_first_tier_only() {
        let err = parse_arch_with_mirror_source_env(
            Some("local-status.json"),
            &["rate-mirrors", "arch", "--fetch-first-tier-only"],
        )
        .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn arch_defaults_to_the_full_status_chain_with_no_explicit_source() {
        let config = parse_arch_with_mirror_source_env(None, &["rate-mirrors", "arch"]).unwrap();

        assert_eq!(arch_target(&config).mirror_source, None);
        assert_eq!(
            selected_source_chain(arch_target(&config)).remote,
            ARCH_SOURCE.remote
        );
    }

    /// Distro installers call this tool with no flags at all. Remote fallback
    /// has to be on by default or mirror setup fails on a machine that has no
    /// pre-seeded local list.
    #[test]
    fn remote_fallback_is_allowed_by_default() {
        let config = Config::try_parse_from(["rate-mirrors", "stdin"]).unwrap();
        assert!(
            config
                .source_security_config()
                .unwrap()
                .allow_remote_sources
        );
    }

    #[test]
    fn no_remote_sources_disables_the_fallback() {
        let config =
            Config::try_parse_from(["rate-mirrors", "--no-remote-sources", "stdin"]).unwrap();
        assert!(
            !config
                .source_security_config()
                .unwrap()
                .allow_remote_sources
        );
    }

    /// Wrappers and scripts in the wild still pass the old flag; it must keep
    /// parsing, and must not turn the fallback off.
    #[test]
    fn deprecated_allow_remote_sources_flag_still_parses() {
        let config =
            Config::try_parse_from(["rate-mirrors", "--allow-remote-sources", "stdin"]).unwrap();
        assert!(
            config
                .source_security_config()
                .unwrap()
                .allow_remote_sources
        );
    }

    #[test]
    fn source_security_rejects_invalid_sha256() {
        let config =
            Config::try_parse_from(["rate-mirrors", "--mirror-source-sha256=abc", "stdin"])
                .unwrap();
        assert!(matches!(
            config.source_security_config(),
            Err(AppError::InvalidSourceHash)
        ));
    }

    #[test]
    fn source_hash_matching_passes_and_mismatching_fails() {
        let expected = "46fae342146d1a05b2d6fa7d29f1390ccf1d8c7eefba5e5919516d0316d70f18";

        assert!(verify_source_integrity("src", "mirror-source", Some(expected)).is_ok());
        assert!(matches!(
            verify_source_integrity("src", "tampered", Some(expected)),
            Err(AppError::SourceIntegrityMismatch { .. })
        ));
        assert!(verify_source_integrity("src", "anything", None).is_ok());
    }
    #[test]
    fn explicit_verification_disables_untested_fallback() {
        for flag in [
            "--require-dnssec",
            "--max-mirror-age=24",
            "--disable-untested-fallback",
        ] {
            let config = Config::try_parse_from(["rate-mirrors", flag, "stdin"]).unwrap();
            assert!(!config.allows_untested_fallback());
        }
        assert!(Config::try_parse_from(["rate-mirrors", "stdin"])
            .unwrap()
            .allows_untested_fallback());
    }

    #[test]
    fn contradictory_verification_flags_are_rejected() {
        for (a, b) in [
            ("--require-dnssec", "--no-verify-mirrors"),
            ("--max-mirror-age=24", "--no-verify-mirrors"),
            ("--require-dnssec", "--no-dnssec-check"),
        ] {
            assert!(Config::try_parse_from(["rate-mirrors", a, b, "stdin"]).is_err());
        }
    }
}
