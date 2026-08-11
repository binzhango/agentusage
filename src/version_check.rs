use std::time::Duration;

use semver::Version;
use serde::Deserialize;

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/binzhango/agentusage/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateNotice {
    pub(crate) current: String,
    pub(crate) latest: String,
    pub(crate) url: String,
}

impl UpdateNotice {
    pub(crate) fn terminal_message(&self) -> String {
        format!(
            "[agentusage] Update available: v{} (you have v{}). Download it at {} or run `cargo install agentusage --locked --force`.",
            self.latest, self.current, self.url
        )
    }
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

pub(crate) fn check() -> Option<UpdateNotice> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(1200)))
        .build();
    let agent: ureq::Agent = config.into();
    let release: LatestRelease = agent
        .get(LATEST_RELEASE_API)
        .header(
            "User-Agent",
            concat!("agentusage/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;

    newer_release(
        env!("CARGO_PKG_VERSION"),
        &release.tag_name,
        &release.html_url,
    )
}

fn newer_release(current: &str, latest_tag: &str, url: &str) -> Option<UpdateNotice> {
    let current_version = Version::parse(current).ok()?;
    let latest_version = Version::parse(latest_tag.trim_start_matches(['v', 'V'])).ok()?;
    (latest_version > current_version).then(|| UpdateNotice {
        current: current_version.to_string(),
        latest: latest_version.to_string(),
        url: url.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_a_newer_release_with_a_v_prefix() {
        let notice = newer_release("1.4.0", "v1.5.0", "https://example.com/release").unwrap();
        assert_eq!(notice.current, "1.4.0");
        assert_eq!(notice.latest, "1.5.0");
        assert!(notice.terminal_message().contains("Update available"));
    }

    #[test]
    fn ignores_equal_older_and_invalid_releases() {
        assert!(newer_release("1.4.0", "v1.4.0", "unused").is_none());
        assert!(newer_release("1.4.0", "v1.3.9", "unused").is_none());
        assert!(newer_release("1.4.0", "latest", "unused").is_none());
    }
}
