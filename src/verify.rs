//! Cross-checking mirrors against each other before they are written out.
//!
//! Speed says nothing about whether a mirror serves the same bytes as everyone
//! else. This pass fingerprints the repository database each ranked mirror is
//! serving and groups mirrors by that fingerprint. A mirror whose fingerprint no
//! other independent mirror corroborates is dropped.
//!
//! Corroboration rather than a single trusted reference is deliberate: a
//! reference host is one DNS answer away from being the attacker, whereas
//! agreeing with N independently-operated mirrors is not something a hijacked
//! resolver or a single bad operator can manufacture.
//!
//! Two fingerprints are used, strongest first:
//!
//!   * the detached signature beside the database (`<db>.sig`), hashed. CachyOS
//!     and Manjaro publish one. It is a few hundred bytes and covers the exact
//!     database contents.
//!   * the database's `Content-Length`. Arch deliberately does not sign its
//!     databases (hence `SigLevel = ... DatabaseOptional`), so for those repos
//!     the size of a given database generation is the corroborating signal.
//!
//! Freshness comes from the database's `Last-Modified`, which is a mirror's own
//! sync time. It is reported always, and enforced only when the operator sets a
//! limit -- distros differ far too much in sync cadence to invent a default.

use crate::config::{default_client_builder, AppError};
use crate::speed_test::SpeedTestResult;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use openssl::sha::sha256;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use url::Url;

/// Signature files are a few hundred bytes; anything far larger is not one, and
/// we refuse to pull it into memory just to hash it.
const MAX_SIG_BYTES: usize = 64 * 1024;

/// Below this many comparable mirrors there is no meaningful majority to be in,
/// so corroboration is not enforced -- with two mirrors, "agreement" is a coin
/// flip, and dropping both would leave the caller with nothing.
const MIN_POOL_FOR_CONSENSUS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Fingerprint {
    /// Hash of the detached signature beside the database.
    Signature(String),
    /// Size of the database itself, for repos that publish no signature.
    Size(u64),
}

impl Fingerprint {
    fn key(&self) -> String {
        match self {
            Fingerprint::Signature(hash) => format!("sig:{}", hash),
            Fingerprint::Size(len) => format!("len:{}", len),
        }
    }

    fn describe(&self) -> String {
        match self {
            Fingerprint::Signature(hash) => format!("db.sig {}", &hash[..12.min(hash.len())]),
            Fingerprint::Size(len) => format!("db size {}", len),
        }
    }
}

#[derive(Debug)]
pub struct MirrorProbe {
    pub fingerprint: Option<Fingerprint>,
    pub age: Option<Duration>,
    pub error: Option<String>,
    /// Whether the mirror's hostname resolves under a DNSSEC-signed zone.
    /// `None` when the check was disabled or the lookup itself failed.
    pub dnssec: Option<bool>,
}

/// Two mirrors within this much of each other are treated as equally fast, so a
/// preference may reorder them. Anything wider and the preference would be
/// costing the user real download speed.
const SPEED_TIE_RATIO: f64 = 0.9;

/// Why a mirror did not make it into the output.
#[derive(Debug)]
pub enum Rejection {
    Unreachable(String),
    Uncorroborated {
        fingerprint: String,
        agreeing: usize,
    },
    Stale {
        age: Duration,
    },
    NoDnssec,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::Unreachable(err) => write!(f, "could not be verified: {}", err),
            Rejection::Uncorroborated {
                fingerprint,
                agreeing,
            } => write!(
                f,
                "serves a database no other mirror has ({}, {} agreeing)",
                fingerprint, agreeing
            ),
            Rejection::Stale { age } => {
                write!(f, "last synced {:.0}h ago", age.as_secs_f64() / 3600.0)
            }
            Rejection::NoDnssec => write!(f, "hostname is not in a DNSSEC-signed zone"),
        }
    }
}

pub struct VerificationReport {
    pub accepted: Vec<SpeedTestResult>,
    pub rejected: Vec<(SpeedTestResult, Rejection)>,
    /// Size of the largest agreeing group, and how many mirrors were comparable.
    pub consensus: Option<(usize, usize)>,
    /// Set when every mirror would have been dropped and the pass was abandoned.
    pub abandoned: Option<String>,
}

pub struct VerifyConfig {
    pub concurrency: usize,
    pub timeout: Duration,
    pub max_age: Option<Duration>,
    /// DNS-over-HTTPS endpoint used to learn whether a mirror's zone is signed.
    /// It is queried out of band precisely because the local resolver is the
    /// thing a DNS attack would have subverted. `None` disables the check.
    pub doh_resolver: Option<String>,
    /// Float DNSSEC-signed mirrors above unsigned ones of comparable speed.
    pub prefer_dnssec: bool,
    /// Drop mirrors that are not DNSSEC-signed. Off by default: signed zones are
    /// a minority among distro mirrors today, so requiring one discards most of
    /// the pool.
    pub require_dnssec: bool,
}

/// Probe every ranked mirror, then keep those whose database another mirror
/// corroborates.
pub fn verify_mirrors(
    results: Vec<SpeedTestResult>,
    config: &VerifyConfig,
    tx_progress: &mpsc::Sender<String>,
) -> Result<VerificationReport, AppError> {
    if results.is_empty() {
        return Ok(VerificationReport {
            accepted: results,
            rejected: Vec::new(),
            consensus: None,
            abandoned: None,
        });
    }

    let probes = probe_all(&results, config)?;

    evaluate_probes(results, probes, config, tx_progress)
}

fn evaluate_probes(
    results: Vec<SpeedTestResult>,
    probes: Vec<MirrorProbe>,
    config: &VerifyConfig,
    tx_progress: &mpsc::Sender<String>,
) -> Result<VerificationReport, AppError> {
    // Group comparable mirrors by what they are serving.
    let mut groups: HashMap<String, usize> = HashMap::new();
    for probe in probes.iter() {
        if let Some(fingerprint) = &probe.fingerprint {
            *groups.entry(fingerprint.key()).or_insert(0) += 1;
        }
    }
    let comparable: usize = groups.values().sum();
    let largest = groups.values().copied().max().unwrap_or(0);

    for (index, probe) in probes.iter().enumerate() {
        let mirror = &results[index].item;
        match (&probe.fingerprint, &probe.error) {
            (Some(fingerprint), _) => {
                let agreeing = groups.get(&fingerprint.key()).copied().unwrap_or(0);
                tx_progress
                    .send(format!(
                        "    {} {} - {} agreeing{}{}",
                        mirror.url,
                        fingerprint.describe(),
                        agreeing,
                        probe
                            .age
                            .map(|a| format!(", synced {:.0}h ago", a.as_secs_f64() / 3600.0))
                            .unwrap_or_default(),
                        match probe.dnssec {
                            Some(true) => ", DNSSEC",
                            Some(false) => "",
                            None => "",
                        }
                    ))
                    .ok();
            }
            (None, Some(err)) => {
                tx_progress
                    .send(format!("    {} unverifiable: {}", mirror.url, err))
                    .ok();
            }
            (None, None) => {}
        }
    }

    let enforce_consensus = comparable >= MIN_POOL_FOR_CONSENSUS;
    let mut accepted = Vec::new();
    let mut accepted_signed = Vec::new();
    let mut rejected = Vec::new();

    for (result, probe) in results.into_iter().zip(probes) {
        let rejection = classify(&probe, &groups, enforce_consensus, config);
        match rejection {
            Some(reason) => rejected.push((result, reason)),
            None => {
                accepted.push(result);
                accepted_signed.push(probe.dnssec.unwrap_or(false));
            }
        }
    }

    if config.prefer_dnssec && accepted_signed.iter().any(|s| *s) {
        accepted = apply_dnssec_preference(accepted, accepted_signed);
    }

    // Preserve the availability fallback only when no explicit verification
    // constraint was requested. An empty accepted set reaches BlankOutput in
    // the caller before a saved mirror list is opened or replaced.
    if accepted.is_empty() && !config.require_dnssec && config.max_age.is_none() {
        let abandoned = format!(
            "every mirror failed verification ({} checked); keeping the ranking unverified",
            rejected.len()
        );
        let recovered = rejected.into_iter().map(|(result, _)| result).collect();
        return Ok(VerificationReport {
            accepted: recovered,
            rejected: Vec::new(),
            consensus: Some((largest, comparable)),
            abandoned: Some(abandoned),
        });
    }

    Ok(VerificationReport {
        accepted,
        rejected,
        consensus: Some((largest, comparable)),
        abandoned: None,
    })
}

fn classify(
    probe: &MirrorProbe,
    groups: &HashMap<String, usize>,
    enforce_consensus: bool,
    config: &VerifyConfig,
) -> Option<Rejection> {
    let Some(fingerprint) = &probe.fingerprint else {
        return Some(Rejection::Unreachable(
            probe
                .error
                .clone()
                .unwrap_or_else(|| "no response".to_string()),
        ));
    };

    if let (Some(limit), Some(age)) = (config.max_age, probe.age) {
        if age > limit {
            return Some(Rejection::Stale { age });
        }
    }

    if config.require_dnssec && probe.dnssec != Some(true) {
        return Some(Rejection::NoDnssec);
    }

    if enforce_consensus {
        let agreeing = groups.get(&fingerprint.key()).copied().unwrap_or(0);
        if agreeing < 2 {
            return Some(Rejection::Uncorroborated {
                fingerprint: fingerprint.describe(),
                agreeing,
            });
        }
    }

    None
}

fn probe_all(
    results: &[SpeedTestResult],
    config: &VerifyConfig,
) -> Result<Vec<MirrorProbe>, AppError> {
    let runtime = Runtime::new()?;
    let urls: Vec<Url> = results.iter().map(|r| r.item.url_to_test.clone()).collect();
    let timeout = config.timeout;
    let semaphore = Arc::new(Semaphore::new(config.concurrency.max(1)));
    let doh_resolver = config.doh_resolver.clone();

    let probes = runtime.block_on(async move {
        let client = default_client_builder()?;

        // One lookup per distinct host, not per mirror: several mirrors often
        // share a hostname, and the zone's signing status is a property of the
        // name, not of the URL.
        let dnssec_by_host = match &doh_resolver {
            Some(resolver) => {
                let hosts: Vec<String> = urls
                    .iter()
                    .filter_map(|u| u.host_str().map(|h| h.to_string()))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let lookups = hosts.iter().map(|host| {
                    let client = client.clone();
                    let resolver = resolver.clone();
                    async move {
                        (
                            host.clone(),
                            zone_is_signed(&client, &resolver, host, timeout).await,
                        )
                    }
                });
                join_all(lookups).await.into_iter().collect()
            }
            None => HashMap::new(),
        };
        let dnssec_by_host = Arc::new(dnssec_by_host);

        let tasks = urls.into_iter().map(|url| {
            let client = client.clone();
            let semaphore = Arc::clone(&semaphore);
            let dnssec_by_host = Arc::clone(&dnssec_by_host);
            async move {
                let _permit = semaphore.acquire().await;
                let dnssec = url
                    .host_str()
                    .and_then(|host| dnssec_by_host.get(host).copied())
                    .flatten();
                let mut probe = probe_mirror(&client, url, timeout).await;
                probe.dnssec = dnssec;
                probe
            }
        });
        Ok::<_, AppError>(join_all(tasks).await)
    });

    runtime.shutdown_timeout(Duration::from_secs(1));
    probes
}

/// Ask a validating resolver whether the answer for `host` was DNSSEC-signed.
///
/// The `AD` (Authenticated Data) flag is set only when the resolver validated
/// the chain of trust, so it answers "is this zone signed" and "did validation
/// succeed" in one query. `None` means the lookup failed and nothing should be
/// concluded either way.
async fn zone_is_signed(
    client: &reqwest::Client,
    resolver: &str,
    host: &str,
    timeout: Duration,
) -> Option<bool> {
    #[derive(serde::Deserialize)]
    struct DohAnswer {
        #[serde(rename = "AD")]
        ad: Option<bool>,
        #[serde(rename = "Status")]
        status: Option<i32>,
    }

    let response = client
        .get(resolver)
        .query(&[("name", host), ("type", "A")])
        .header("accept", "application/dns-json")
        .timeout(timeout)
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let answer: DohAnswer = response.json().await.ok()?;
    // Status 0 is NOERROR; anything else means we did not get a usable answer.
    if answer.status.unwrap_or(-1) != 0 {
        return None;
    }
    Some(answer.ad.unwrap_or(false))
}

/// Float DNSSEC-signed mirrors above unsigned ones they are effectively tied
/// with on speed. Ordering is otherwise untouched, so a meaningfully faster
/// mirror is never demoted for being unsigned.
///
/// Mirrors are walked fastest-first and grouped into bands, each running while
/// members stay within [`SPEED_TIE_RATIO`] of the band leader; signed mirrors
/// are moved to the front of their own band. Banding rather than a comparator
/// keeps the ordering a total one -- "within 10% of each other" is not
/// transitive, and sorting on it can panic.
fn apply_dnssec_preference(
    accepted: Vec<SpeedTestResult>,
    signed: Vec<bool>,
) -> Vec<SpeedTestResult> {
    let mut remaining: Vec<(SpeedTestResult, bool)> = accepted.into_iter().zip(signed).collect();
    let mut ordered = Vec::with_capacity(remaining.len());

    while !remaining.is_empty() {
        let leader = remaining[0].0.speed;
        let band_len = remaining
            .iter()
            .take_while(|(result, _)| result.speed >= leader * SPEED_TIE_RATIO)
            .count()
            .max(1);

        // `partition` is stable, so relative speed order survives within each half.
        let (mut band, rest): (Vec<_>, Vec<_>) = remaining
            .drain(..band_len)
            .partition(|(_, is_signed)| *is_signed);
        band.extend(rest);
        ordered.extend(band.into_iter().map(|(result, _)| result));
    }

    ordered
}

async fn probe_mirror(client: &reqwest::Client, db_url: Url, timeout: Duration) -> MirrorProbe {
    // One HEAD gives both the size fingerprint and the sync time, and works on
    // every repo layout because it targets the file we already speed-test.
    let head = match client.head(db_url.clone()).timeout(timeout).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            return MirrorProbe {
                fingerprint: None,
                age: None,
                error: Some(format!("HTTP {}", response.status().as_u16())),
                dnssec: None,
            }
        }
        Err(e) => {
            return MirrorProbe {
                fingerprint: None,
                age: None,
                error: Some(short_error(&e)),
                dnssec: None,
            }
        }
    };

    // Read the header directly: `Response::content_length` reports the decoded
    // body length, which is 0 for a HEAD, and a zero-valued fingerprint would
    // make every mirror agree with every other one for the wrong reason.
    let size = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|len| *len > 0);
    let age = head
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
        .and_then(|modified| {
            Utc::now()
                .signed_duration_since(modified.with_timezone(&Utc))
                .to_std()
                .ok()
        });

    // Prefer the signature when the repo publishes one.
    let signature = fetch_signature(client, &db_url, timeout).await;
    let fingerprint = match signature {
        Some(hash) => Some(Fingerprint::Signature(hash)),
        None => size.map(Fingerprint::Size),
    };

    let error = if fingerprint.is_none() {
        Some("no signature and no content-length".to_string())
    } else {
        None
    };

    MirrorProbe {
        fingerprint,
        age,
        error,
        dnssec: None,
    }
}

async fn fetch_signature(
    client: &reqwest::Client,
    db_url: &Url,
    timeout: Duration,
) -> Option<String> {
    let sig_url = Url::parse(&format!("{}.sig", db_url)).ok()?;
    let response = client.get(sig_url).timeout(timeout).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response.content_length().unwrap_or(0) > MAX_SIG_BYTES as u64 {
        return None;
    }

    let body = response.bytes().await.ok()?;
    if body.is_empty() || body.len() > MAX_SIG_BYTES {
        return None;
    }

    Some(sha256(&body).iter().map(|b| format!("{:02x}", b)).collect())
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_connect() {
        "connection failed".to_string()
    } else {
        "request failed".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(fingerprint: Option<Fingerprint>, age: Option<Duration>) -> MirrorProbe {
        MirrorProbe {
            fingerprint,
            age,
            error: None,
            dnssec: None,
        }
    }

    fn cfg(max_age: Option<Duration>) -> VerifyConfig {
        VerifyConfig {
            concurrency: 4,
            timeout: Duration::from_secs(5),
            max_age,
            doh_resolver: None,
            prefer_dnssec: false,
            require_dnssec: false,
        }
    }

    fn groups(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn corroborated_mirror_is_kept() {
        let g = groups(&[("sig:abc", 8)]);
        let p = probe(Some(Fingerprint::Signature("abc".into())), None);
        assert!(classify(&p, &g, true, &cfg(None)).is_none());
    }

    #[test]
    fn lone_fingerprint_is_rejected() {
        // The case this pass exists for: one endpoint serving a database that
        // nothing else in the pool has.
        let g = groups(&[("sig:abc", 8), ("sig:odd", 1)]);
        let p = probe(Some(Fingerprint::Signature("odd".into())), None);
        assert!(matches!(
            classify(&p, &g, true, &cfg(None)),
            Some(Rejection::Uncorroborated { agreeing: 1, .. })
        ));
    }

    #[test]
    fn a_second_agreeing_mirror_is_enough() {
        // Small honest minorities (a repo generation two mirrors have and the
        // rest have not synced yet) must survive.
        let g = groups(&[("len:100", 6), ("len:200", 2)]);
        let p = probe(Some(Fingerprint::Size(200)), None);
        assert!(classify(&p, &g, true, &cfg(None)).is_none());
    }

    #[test]
    fn consensus_is_not_enforced_in_a_tiny_pool() {
        let g = groups(&[("len:100", 1), ("len:200", 1)]);
        let p = probe(Some(Fingerprint::Size(100)), None);
        assert!(classify(&p, &g, false, &cfg(None)).is_none());
    }

    #[test]
    fn unreachable_mirror_is_rejected() {
        let p = MirrorProbe {
            fingerprint: None,
            age: None,
            error: Some("timed out".into()),
            dnssec: None,
        };
        assert!(matches!(
            classify(&p, &groups(&[]), true, &cfg(None)),
            Some(Rejection::Unreachable(_))
        ));
    }

    #[test]
    fn staleness_is_only_enforced_when_a_limit_is_set() {
        let g = groups(&[("len:100", 5)]);
        let old = probe(
            Some(Fingerprint::Size(100)),
            Some(Duration::from_secs(80 * 3600)),
        );

        assert!(classify(&old, &g, true, &cfg(None)).is_none());
        assert!(matches!(
            classify(&old, &g, true, &cfg(Some(Duration::from_secs(24 * 3600)))),
            Some(Rejection::Stale { .. })
        ));
    }

    fn result(host: &str, speed_bytes_per_sec: usize) -> SpeedTestResult {
        let url = Url::parse(&format!("https://{}/repo/", host)).unwrap();
        let mirror = crate::mirror::Mirror {
            url_to_test: url.join("db").unwrap(),
            url,
            country: None,
        };
        SpeedTestResult::new(
            mirror,
            speed_bytes_per_sec,
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
    }

    fn hosts(results: &[SpeedTestResult]) -> Vec<String> {
        results
            .iter()
            .map(|r| r.item.url.host_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn dnssec_preference_promotes_within_a_speed_tie() {
        // 100 and 95 are effectively tied, so the signed one leads; the signed
        // but much slower mirror stays where its speed puts it.
        let results = vec![
            result("unsigned-fast", 100),
            result("signed-fast", 95),
            result("signed-slow", 10),
        ];
        let ordered = apply_dnssec_preference(results, vec![false, true, true]);

        assert_eq!(
            hosts(&ordered),
            vec!["signed-fast", "unsigned-fast", "signed-slow"]
        );
    }

    #[test]
    fn dnssec_preference_never_demotes_a_much_faster_mirror() {
        let results = vec![result("unsigned-fast", 100), result("signed-slow", 20)];
        let ordered = apply_dnssec_preference(results, vec![false, true]);

        assert_eq!(hosts(&ordered), vec!["unsigned-fast", "signed-slow"]);
    }

    #[test]
    fn dnssec_preference_is_stable_among_equals() {
        let results = vec![result("a", 100), result("b", 98), result("c", 96)];
        let ordered = apply_dnssec_preference(results, vec![false, false, false]);

        assert_eq!(hosts(&ordered), vec!["a", "b", "c"]);
    }

    #[test]
    fn requiring_dnssec_rejects_unsigned_mirrors() {
        let g = groups(&[("len:100", 5)]);
        let mut config = cfg(None);
        config.require_dnssec = true;

        let mut unsigned = probe(Some(Fingerprint::Size(100)), None);
        unsigned.dnssec = Some(false);
        assert!(matches!(
            classify(&unsigned, &g, true, &config),
            Some(Rejection::NoDnssec)
        ));

        let mut signed = probe(Some(Fingerprint::Size(100)), None);
        signed.dnssec = Some(true);
        assert!(classify(&signed, &g, true, &config).is_none());

        // A failed lookup must not pass as signed.
        let unknown = probe(Some(Fingerprint::Size(100)), None);
        assert!(matches!(
            classify(&unknown, &g, true, &config),
            Some(Rejection::NoDnssec)
        ));
    }

    #[test]
    fn signature_beats_size_as_a_fingerprint() {
        assert_eq!(Fingerprint::Signature("abcdef".into()).key(), "sig:abcdef");
        assert_eq!(Fingerprint::Size(42).key(), "len:42");
        assert_ne!(
            Fingerprint::Signature("42".into()).key(),
            Fingerprint::Size(42).key()
        );
    }
    #[test]
    fn explicit_constraints_do_not_restore_rejected_mirrors() {
        for require_dnssec in [false, true] {
            let mut config = cfg(Some(Duration::from_secs(10)));
            config.require_dnssec = require_dnssec;
            let (tx, _rx) = mpsc::channel();
            let report = evaluate_probes(
                vec![result("example.invalid", 100)],
                vec![probe(
                    Some(Fingerprint::Size(100)),
                    Some(Duration::from_secs(20)),
                )],
                &config,
                &tx,
            )
            .unwrap();
            assert!(report.accepted.is_empty());
            assert_eq!(report.rejected.len(), 1);
            assert!(report.abandoned.is_none());
        }
    }

    #[test]
    fn dnssec_only_constraint_does_not_restore_unsigned_mirrors() {
        let mut config = cfg(None);
        config.require_dnssec = true;
        let (tx, _rx) = mpsc::channel();
        let report = evaluate_probes(
            vec![result("example.invalid", 100)],
            vec![probe(Some(Fingerprint::Size(100)), None)],
            &config,
            &tx,
        )
        .unwrap();
        assert!(report.accepted.is_empty());
    }

    #[test]
    fn default_unverified_fallback_is_preserved() {
        let (tx, _rx) = mpsc::channel();
        let report = evaluate_probes(
            vec![result("example.invalid", 100)],
            vec![probe(None, None)],
            &cfg(None),
            &tx,
        )
        .unwrap();
        assert_eq!(report.accepted.len(), 1);
        assert!(report.abandoned.is_some());
    }
}
