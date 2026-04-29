/*!
[Commander] member functions related to `jj workspace`.

Each workspace is an additional working copy backed by the same
`.jj/repo`. Each has its own `@` (working-copy commit), and other
workspaces' working-copy commits show up in the log as ordinary nodes
labeled `<name>@`.

Used in the [workspaces_tab][crate::ui::workspaces_tab] module.
*/

use crate::commander::{CommandError, Commander, RemoveEndLine, ids::CommitId};

use anyhow::Result;
use regex::Regex;
use std::{path::PathBuf, sync::LazyLock};
use tracing::instrument;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub change_id_short: String,
    pub commit_id_short: String,
    pub empty: bool,
    pub description: String,
    pub root: PathBuf,
    pub is_current: bool,
}

// Template which outputs `[name|change_id_short|commit_id_short|empty|first_line_of_description]`.
const WORKSPACE_TEMPLATE: &str = r#""[" ++ name ++ "|" ++ target.change_id().short() ++ "|" ++ target.commit_id().short() ++ "|" ++ if(target.empty(), "true", "false") ++ "|" ++ target.description().first_line() ++ "]\n""#;

static WORKSPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([^|]*)\|([^|]*)\|([^|]*)\|([^|]*)\|(.*)\]$").unwrap());

impl Commander {
    /// Get all workspaces, with their working-copy commits and a flag
    /// indicating which workspace lazyjj is currently attached to.
    /// Maps to `jj workspace list -T ...` plus `jj workspace root`.
    #[instrument(level = "trace", skip(self))]
    pub fn get_workspaces(&self) -> Result<Vec<Workspace>, CommandError> {
        let template_output = self.execute_jj_command(
            vec!["workspace", "list", "-T", WORKSPACE_TEMPLATE],
            false,
            true,
        )?;

        // Resolve absolute paths up front so comparison with the current
        // workspace root is reliable.
        let current_root = self.get_workspace_root(None).ok();

        let mut workspaces: Vec<Workspace> = Vec::new();
        for line in template_output.lines() {
            let Some(captured) = WORKSPACE_REGEX.captures(line) else {
                continue;
            };
            let name = captured[1].to_owned();
            let change_id_short = captured[2].to_owned();
            let commit_id_short = captured[3].to_owned();
            let empty = &captured[4] == "true";
            let description = captured[5].to_owned();

            let root = self
                .get_workspace_root(Some(&name))
                .unwrap_or_else(|_| PathBuf::new());
            let is_current = current_root
                .as_ref()
                .is_some_and(|current| !current.as_os_str().is_empty() && current == &root);

            workspaces.push(Workspace {
                name,
                change_id_short,
                commit_id_short,
                empty,
                description,
                root,
                is_current,
            });
        }

        Ok(workspaces)
    }

    /// Find the workspace whose working-copy commit (`<name>@`) is the
    /// given commit. Returns the workspace's root, or `None` if no
    /// workspace points at this commit. Used to copy the absolute path
    /// of the workspace that "owns" a selected change in the log.
    #[instrument(level = "trace", skip(self))]
    pub fn get_workspace_root_for_commit(
        &self,
        commit_id: &CommitId,
    ) -> Result<Option<PathBuf>, CommandError> {
        let workspaces = self.get_workspaces()?;
        // `commit_id_short` is jj's short prefix of the full commit hash;
        // `commit_id.as_str()` is the full hash. Match by prefix so callers
        // can pass either form.
        let full = commit_id.as_str();
        Ok(workspaces.into_iter().find_map(|w| {
            if !w.commit_id_short.is_empty() && full.starts_with(&w.commit_id_short) {
                Some(w.root)
            } else {
                None
            }
        }))
    }

    /// Get the absolute root path of a workspace. With `name = None`,
    /// returns the current workspace's root.
    /// Maps to `jj workspace root [--name <NAME>]`.
    #[instrument(level = "trace", skip(self))]
    pub fn get_workspace_root(&self, name: Option<&str>) -> Result<PathBuf, CommandError> {
        let mut args = vec!["workspace", "root"];
        if let Some(name) = name {
            args.push("--name");
            args.push(name);
        }
        let out = self
            .execute_jj_command(args, false, true)?
            .remove_end_line();
        Ok(PathBuf::from(out))
    }

    /// Add a new workspace.
    /// Maps to `jj workspace add <PATH> [--name <NAME>] [-r <REVISION>]`.
    #[instrument(level = "trace", skip(self))]
    pub fn run_workspace_add(
        &self,
        path: &str,
        name: Option<&str>,
        revision: Option<&str>,
    ) -> Result<(), CommandError> {
        let mut args = vec!["workspace", "add"];
        if let Some(name) = name {
            args.push("--name");
            args.push(name);
        }
        if let Some(revision) = revision {
            args.push("-r");
            args.push(revision);
        }
        args.push(path);
        self.execute_jj_command(args, true, true)?;
        Ok(())
    }

    /// Forget a workspace. Does not remove the on-disk directory.
    /// Maps to `jj workspace forget <NAME>`.
    #[instrument(level = "trace", skip(self))]
    pub fn run_workspace_forget(&self, name: &str) -> Result<(), CommandError> {
        self.execute_jj_command(vec!["workspace", "forget", name], true, true)?;
        Ok(())
    }

    /// Rename the *current* workspace. jj only supports renaming the
    /// workspace you're attached to; renaming another requires running
    /// jj from inside it.
    /// Maps to `jj workspace rename <NEW_NAME>`.
    #[instrument(level = "trace", skip(self))]
    pub fn run_workspace_rename(&self, new_name: &str) -> Result<(), CommandError> {
        self.execute_jj_command(vec!["workspace", "rename", new_name], true, true)?;
        Ok(())
    }

    /// Bring a stale workspace up to date with the recorded working-copy commit.
    /// Maps to `jj workspace update-stale` (run inside the target workspace).
    #[instrument(level = "trace", skip(self))]
    pub fn run_workspace_update_stale(&self) -> Result<(), CommandError> {
        self.execute_jj_command(vec!["workspace", "update-stale"], true, true)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commander::tests::TestRepo;

    #[test]
    fn get_workspaces_default_only() -> Result<()> {
        let test_repo = TestRepo::new()?;
        let workspaces = test_repo.commander.get_workspaces()?;
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "default");
        assert!(workspaces[0].is_current);
        Ok(())
    }

    #[test]
    fn add_and_list_workspaces() -> Result<()> {
        let test_repo = TestRepo::new()?;

        // The add destination must not pre-exist; place it as a sibling.
        let dest = test_repo
            .directory
            .path()
            .parent()
            .expect("temp dir has parent")
            .join(format!(
                "{}-extra",
                test_repo
                    .directory
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
            ));

        test_repo
            .commander
            .run_workspace_add(dest.to_str().unwrap(), Some("extra"), None)?;

        let workspaces = test_repo.commander.get_workspaces()?;
        let names: Vec<_> = workspaces.iter().map(|w| w.name.as_str()).collect();
        assert!(names.contains(&"default"));
        assert!(names.contains(&"extra"));

        let current_count = workspaces.iter().filter(|w| w.is_current).count();
        assert_eq!(current_count, 1, "exactly one workspace is current");
        let current = workspaces.iter().find(|w| w.is_current).unwrap();
        assert_eq!(current.name, "default");

        // The auxiliary directory is owned by the test outside the
        // TempDir, so clean it up explicitly.
        let _ = std::fs::remove_dir_all(&dest);

        Ok(())
    }
}
