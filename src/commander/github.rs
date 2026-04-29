/*!
[Commander] member functions related to GitHub integration.

Looks up open pull requests for the current repository so the log tab can
annotate changes that have open PRs and open them in a browser.
*/

use std::{
    collections::HashMap,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::commander::Commander;

/// How long a successful `gh pr list` result is reused before re-fetching.
/// Keeps the TUI responsive on every log refresh while still picking up
/// new PRs within roughly a minute.
const PR_CACHE_TTL: Duration = Duration::from_secs(60);

/// Process-wide cache of the open-PR lookup. Keyed by the repo root so
/// distinct lazyjj sessions in different repos don't pollute each other.
/// `None` for the value means "we tried and the feature is unavailable
/// here" (no GitHub remote, or `gh` not installed/authed) — also cached
/// so we don't re-shell out on every refresh.
static PR_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

struct CacheEntry {
    fetched_at: Instant,
    value: OpenPrs,
}

/// Owner/repo pair parsed from `git remote get-url origin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
}

impl GitHubRepo {
    pub fn pr_url(&self, number: u32) -> String {
        format!(
            "https://github.com/{}/{}/pull/{number}",
            self.owner, self.name
        )
    }
}

/// Information about open PRs for a repo, keyed by head branch name.
#[derive(Clone, Debug, Default)]
pub struct OpenPrs {
    pub repo: Option<GitHubRepo>,
    pub by_branch: HashMap<String, u32>,
}

impl OpenPrs {
    pub fn pr_for_branch(&self, branch: &str) -> Option<u32> {
        self.by_branch.get(branch).copied()
    }
}

impl Commander {
    /// Best-effort fetch of open PRs for the current repo. Never errors — if
    /// `git` or `gh` fail (not installed, not authed, no GitHub remote), we
    /// just return an empty [`OpenPrs`] and the feature is silently disabled.
    /// Result is cached per repo root with [`PR_CACHE_TTL`]; callers can hit
    /// this on the per-refresh hot path without firing a network request
    /// every time.
    pub fn get_open_prs(&self) -> OpenPrs {
        let cache = PR_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let key = self.env.root.clone();

        if let Some(entry) = cache.lock().unwrap().get(&key)
            && entry.fetched_at.elapsed() < PR_CACHE_TTL
        {
            return entry.value.clone();
        }

        let value = match self.detect_github_repo() {
            Some(repo) => OpenPrs {
                by_branch: fetch_open_prs(&repo).unwrap_or_default(),
                repo: Some(repo),
            },
            None => OpenPrs::default(),
        };

        cache.lock().unwrap().insert(
            key,
            CacheEntry {
                fetched_at: Instant::now(),
                value: value.clone(),
            },
        );
        value
    }

    fn detect_github_repo(&self) -> Option<GitHubRepo> {
        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.env.root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let url = String::from_utf8(output.stdout).ok()?;
        parse_github_remote(url.trim())
    }
}

/// Call `gh pr list` and return a map of head branch name -> PR number.
/// Uses `--jq` to emit TSV so we don't need a JSON parser.
fn fetch_open_prs(repo: &GitHubRepo) -> Option<HashMap<String, u32>> {
    let slug = format!("{}/{}", repo.owner, repo.name);
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &slug,
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,headRefName",
            "--jq",
            r#".[] | "\(.number)\t\(.headRefName)""#,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    let mut by_branch = HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let number = match parts.next().and_then(|n| n.trim().parse::<u32>().ok()) {
            Some(n) => n,
            None => continue,
        };
        let branch = match parts.next() {
            Some(b) => b.trim(),
            None => continue,
        };
        if !branch.is_empty() {
            by_branch.insert(branch.to_owned(), number);
        }
    }
    Some(by_branch)
}

/// Parse a GitHub remote URL into (owner, repo). Accepts both SSH and HTTPS
/// forms, with or without a trailing `.git`. Returns None for non-GitHub
/// remotes.
fn parse_github_remote(url: &str) -> Option<GitHubRepo> {
    let stripped = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let stripped = stripped.strip_suffix(".git").unwrap_or(stripped);

    let (owner, name) = stripped.split_once('/')?;
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(GitHubRepo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_remote() {
        assert_eq!(
            parse_github_remote("git@github.com:suno-ai/app-mobile.git"),
            Some(GitHubRepo {
                owner: "suno-ai".into(),
                name: "app-mobile".into(),
            })
        );
    }

    #[test]
    fn parse_https_remote() {
        assert_eq!(
            parse_github_remote("https://github.com/suno-ai/app-mobile"),
            Some(GitHubRepo {
                owner: "suno-ai".into(),
                name: "app-mobile".into(),
            })
        );
    }

    #[test]
    fn parse_ssh_protocol_remote() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/suno-ai/app-mobile.git"),
            Some(GitHubRepo {
                owner: "suno-ai".into(),
                name: "app-mobile".into(),
            })
        );
    }

    #[test]
    fn parse_non_github_returns_none() {
        assert!(parse_github_remote("git@gitlab.com:foo/bar.git").is_none());
    }

    #[test]
    fn pr_url_is_formatted() {
        let repo = GitHubRepo {
            owner: "suno-ai".into(),
            name: "app-mobile".into(),
        };
        assert_eq!(
            repo.pr_url(42),
            "https://github.com/suno-ai/app-mobile/pull/42"
        );
    }
}
