//! whether a newer curst is out: asked of GitHub at most once a day, and
//! only when the user said yes to it. the request carries nothing about
//! the user or their collections, and its answer is kept in the data
//! folder until the next day
//!
//! a new version is told once, with the command that updates curst the
//! way it was installed

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use curst_core::http::{Cancel, HttpExecutor, RedirectRule};
use curst_core::model::{KeyValue, Request};
use serde_json::{Value, json};

/// where the latest release is described
const LATEST: &str = "https://api.github.com/repos/LichiBruno/curst/releases/latest";

/// how long an answer is good for
const DAY: Duration = Duration::from_hours(24);

/// the version this curst is
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// what the last check found, kept between runs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cache {
    /// when it was, in seconds since 1970
    pub(crate) checked: u64,

    /// the latest version then
    pub(crate) latest: Option<String>,

    /// the newest version the user was told about
    pub(crate) told: Option<String>,
}

impl Cache {
    pub(crate) fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("update-check.json")
    }

    /// the cache in `file`, empty when there is none or it cannot be read
    pub(crate) fn read(file: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(file) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);

        Self {
            checked: value.get("checked").and_then(Value::as_u64).unwrap_or(0),
            latest: text("latest"),
            told: text("told"),
        }
    }

    fn write(&self, file: &Path) {
        let value = json!({
            "checked": self.checked,
            "latest": self.latest,
            "told": self.told,
        });

        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let _ = std::fs::write(file, value.to_string());
    }
}

/// a version as numbers, `None` for one that is not plain `x.y.z`, such
/// as a pre-release
fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let parsed = (next()?, next()?, next()?);

    parts.next().is_none().then_some(parsed)
}

/// whether `latest` is a newer release than `current`
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    matches!((numbers(latest), numbers(current)), (Some(l), Some(c)) if l > c)
}

/// the version GitHub's answer names, without its `v`; a draft or a
/// pre-release is not one
fn latest_in(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);

    if flag("draft") || flag("prerelease") {
        return None;
    }

    let tag = value.get("tag_name")?.as_str()?;

    numbers(tag).map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
}

/// the command that updates curst, judged by where its program is
pub(crate) fn how_to_update(exe: &Path) -> String {
    let path = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    let releases = "https://github.com/LichiBruno/curst/releases/latest";

    if path.contains("/node_modules/") {
        "npm i -g @curst/cli@latest".into()
    } else if path.contains("/cellar/")
        || path.contains("/homebrew/")
        || path.contains("/linuxbrew/")
    {
        "brew upgrade curst".into()
    } else if path.contains("/.cargo/bin/")
        && exe
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        format!("irm {releases}/download/curst-installer.ps1 | iex")
    } else if path.contains("/.cargo/bin/") {
        format!("curl -LsSf {releases}/download/curst-installer.sh | sh")
    } else {
        releases.into()
    }
}

/// asks GitHub for the latest release
fn fetch(executor: &dyn HttpExecutor) -> Option<String> {
    let request = Request {
        url: LATEST.into(),
        headers: vec![KeyValue::new("Accept", "application/vnd.github+json")],
        ..Request::default()
    };
    let response = executor
        .execute(&request, &Cancel::new(), &RedirectRule::Any)
        .ok()?;

    if response.status != 200 {
        return None;
    }

    latest_in(&String::from_utf8_lossy(response.body.preview()))
}

/// seconds since 1970
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// what to tell the user, if anything: a newer version they have not been
/// told about, with how to get it. GitHub is asked only when the last
/// answer is a day old; with no network, nothing is said
pub(crate) fn check(
    executor: &dyn HttpExecutor,
    file: &Path,
    now: u64,
    exe: &Path,
) -> Option<String> {
    let mut cache = Cache::read(file);

    if now.saturating_sub(cache.checked) >= DAY.as_secs() {
        if let Some(latest) = fetch(executor) {
            cache.latest = Some(latest);
        }

        cache.checked = now;
    }

    let latest = cache
        .latest
        .clone()
        .filter(|latest| is_newer(latest, CURRENT))
        .filter(|latest| cache.told.as_ref() != Some(latest));

    if let Some(latest) = &latest {
        cache.told = Some(latest.clone());
    }

    cache.write(file);

    latest.map(|latest| format!("curst {latest} is out: {}", how_to_update(exe)))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use curst_core::http::{HttpError, Response};

    use super::*;

    struct Github {
        body: String,
        asked: Mutex<u32>,
    }

    impl HttpExecutor for Github {
        fn execute(
            &self,
            request: &Request,
            _: &Cancel,
            _: &RedirectRule,
        ) -> Result<Response, HttpError> {
            assert_eq!(request.url, LATEST);

            if let Ok(mut asked) = self.asked.lock() {
                *asked += 1;
            }

            Ok(crate::app::tests::response(&self.body))
        }
    }

    fn github(tag: &str) -> Github {
        Github {
            body: format!("{{\"tag_name\": \"{tag}\", \"draft\": false, \"prerelease\": false}}"),
            asked: Mutex::new(0),
        }
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta.1", "0.1.0"));
        assert_eq!(
            latest_in("{\"tag_name\": \"v0.3.0\"}"),
            Some("0.3.0".into())
        );
        assert_eq!(
            latest_in("{\"tag_name\": \"v0.3.0\", \"prerelease\": true}"),
            None
        );
        assert_eq!(latest_in("<html>"), None);
    }

    #[test]
    fn the_command_follows_how_curst_was_installed() {
        let at = |path: &str| how_to_update(Path::new(path));

        assert_eq!(
            at("/usr/local/lib/node_modules/@curst/cli/node_modules/.bin_real/curst"),
            "npm i -g @curst/cli@latest"
        );
        assert_eq!(
            at("/opt/homebrew/Cellar/curst/0.1.0/bin/curst"),
            "brew upgrade curst"
        );
        assert!(at("/Users/me/.cargo/bin/curst").starts_with("curl -LsSf "));
        assert!(at("C:\\Users\\me\\.cargo\\bin\\curst.exe").starts_with("irm "));
        assert!(at("/somewhere/curst").ends_with("/releases/latest"));
    }

    #[test]
    fn a_new_version_is_told_once_and_github_asked_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let file = Cache::file(dir.path());
        let exe = Path::new("/usr/lib/node_modules/@curst/cli/curst");
        let newer = github("v99.0.0");
        let start = 1_000_000;

        assert_eq!(
            check(&newer, &file, start, exe).as_deref(),
            Some("curst 99.0.0 is out: npm i -g @curst/cli@latest")
        );

        // the same day: no request, and nothing said again
        assert_eq!(check(&newer, &file, start + 60, exe), None);
        assert_eq!(*newer.asked.lock().unwrap(), 1);

        // a day later GitHub is asked again; this version was told already
        assert_eq!(check(&newer, &file, start + DAY.as_secs(), exe), None);
        assert_eq!(*newer.asked.lock().unwrap(), 2);

        // the version running now is never news
        let dir = tempfile::tempdir().unwrap();
        let file = Cache::file(dir.path());

        assert_eq!(check(&github(CURRENT), &file, start, exe), None);
    }
}
