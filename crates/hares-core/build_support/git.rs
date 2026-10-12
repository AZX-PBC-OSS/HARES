//! Build-time git resolution, shared through `#[path]` by the build
//! script (`build.rs`) and this crate's `build_support` test target: the
//! git directory a tree resolves to, the commit its `HEAD` names, the
//! dirty flag the porcelain output parses to, and the rerun paths the
//! build script emits. The commit resolution reads files only, never a
//! git process; absent git or `.git` resolves to `None`, never a guess.

use std::fs;
use std::path::{Path, PathBuf};

/// The git directory for a workspace root: `<root>/.git` when it is a
/// directory, and the directory the `gitdir:` line of the `.git` file
/// names when `.git` is a file (a worktree's checkout, as every
/// engine-under-test checkout is). `None` when there is neither.
#[must_use]
pub fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    if dot_git.is_file() {
        let content = fs::read_to_string(&dot_git).ok()?;
        for line in content.lines() {
            let line = line.trim_start();
            if let Some(target) = line.strip_prefix("gitdir:") {
                let target = target.trim();
                if target.is_empty() {
                    return None;
                }
                let named = Path::new(target);
                return Some(if named.is_absolute() {
                    named.to_path_buf()
                } else {
                    root.join(named)
                });
            }
        }
    }
    None
}

/// The common directory of a git directory: the path its `commondir`
/// file names (a worktree's git directory shares refs with the main
/// `.git`), or the directory itself.
fn common_dir(git_dir: &Path) -> PathBuf {
    let Some(content) = fs::read_to_string(git_dir.join("commondir")).ok() else {
        return git_dir.to_path_buf();
    };
    let named = content.trim();
    if named.is_empty() {
        return git_dir.to_path_buf();
    }
    let common = Path::new(named);
    if common.is_absolute() {
        common.to_path_buf()
    } else {
        git_dir.join(common)
    }
}

/// What a `HEAD` file says: a symbolic ref's name, or a detached hash.
enum Head {
    Ref(String),
    Hash(String),
}

/// Reads `git_dir`'s `HEAD`.
fn head_file(git_dir: &Path) -> Option<Head> {
    let content = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let content = content.trim();
    if let Some(name) = content.strip_prefix("ref:") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        return Some(Head::Ref(name.to_string()));
    }
    if content.len() == 40 && content.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(Head::Hash(content.to_ascii_lowercase()));
    }
    None
}

/// The hash a ref name resolves to: the loose ref file in the git
/// directory or its common directory, else the `packed-refs` entry.
fn resolve_ref(git_dir: &Path, name: &str) -> Option<String> {
    let common = common_dir(git_dir);
    for base in [git_dir, &common] {
        if let Ok(hash) = fs::read_to_string(base.join(name)) {
            let hash = hash.trim();
            if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(hash.to_ascii_lowercase());
            }
        }
    }
    for base in [&common, git_dir] {
        if let Ok(packed) = fs::read_to_string(base.join("packed-refs")) {
            for line in packed.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((hash, ref_name)) = line.split_once(' ')
                    && ref_name == name
                    && hash.len() == 40
                    && hash.chars().all(|c| c.is_ascii_hexdigit())
                {
                    return Some(hash.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

/// The commit `git_dir`'s `HEAD` names: the hash itself for a detached
/// HEAD, the ref file's (or the packed refs entry's) hash for a symbolic
/// ref. `None` when `HEAD` names neither.
#[must_use]
pub fn resolve_head(git_dir: &Path) -> Option<String> {
    match head_file(git_dir)? {
        Head::Hash(hash) => Some(hash),
        Head::Ref(name) => resolve_ref(git_dir, &name),
    }
}

/// The loose ref file `git_dir`'s symbolic `HEAD` resolves through, in
/// the git directory or its common directory; `None` for a detached HEAD
/// or a ref only the packed refs hold.
#[must_use]
pub fn head_ref_file(git_dir: &Path) -> Option<PathBuf> {
    match head_file(git_dir)? {
        Head::Hash(_) => None,
        Head::Ref(name) => {
            let common = common_dir(git_dir);
            [git_dir, &common]
                .into_iter()
                .map(|base| base.join(&name))
                .find(|path| path.is_file())
        }
    }
}

/// Whether the output of `git status --porcelain --untracked-files=no`
/// records an edit: every entry is one line, so an output without a
/// non-empty line is a clean tree. Pure: the caller runs git.
#[must_use]
pub fn porcelain_is_dirty(output: &str) -> bool {
    output.lines().any(|line| !line.trim().is_empty())
}

/// The rerun paths for a workspace root: the git directory's `HEAD`, the
/// resolved ref file, the git directory's `index`, and the workspace's
/// `crates` tree, `Cargo.toml` and `Cargo.lock`. Only paths that exist
/// come back, because a missing rerun path makes cargo rerun the script
/// on every build.
#[must_use]
pub fn rerun_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(git_dir) = resolve_git_dir(workspace_root) {
        paths.push(git_dir.join("HEAD"));
        if let Some(ref_file) = head_ref_file(&git_dir) {
            paths.push(ref_file);
        }
        paths.push(git_dir.join("index"));
    }
    paths.push(workspace_root.join("crates"));
    paths.push(workspace_root.join("Cargo.toml"));
    paths.push(workspace_root.join("Cargo.lock"));
    paths.retain(|path| path.exists());
    paths
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{porcelain_is_dirty, rerun_paths, resolve_git_dir, resolve_head};

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Writes a file with its parent directories.
    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("the parent exists")).expect("the directories");
        fs::write(path, content).expect("the file writes");
    }

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    /// `head_commit_resolution` against synthetic `.git` trees written as
    /// plain files in a temporary directory: a detached HEAD, a symbolic
    /// ref, a packed ref, a worktree `gitdir:` file, and no `.git`. No
    /// git process is started: the resolution reads files.
    #[test]
    fn head_commit_resolution() {
        let tmp = tempfile::tempdir().expect("the temp directory");
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).expect("the repo root");

        // Detached HEAD: the hash is the file's content.
        write(&root.join(".git/HEAD"), &format!("{HASH}\n"));
        let git_dir = resolve_git_dir(&root).expect("the .git directory");
        assert_eq!(resolve_head(&git_dir).as_deref(), Some(HASH));

        // A symbolic ref: the ref file's hash.
        fs::remove_dir_all(git_dir).expect("the git directory");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/refs/heads/main"), &format!("{HASH}\n"));
        let git_dir = resolve_git_dir(&root).expect("the .git directory");
        assert_eq!(resolve_head(&git_dir).as_deref(), Some(HASH));

        // A packed ref: no loose ref file, the hash in packed-refs.
        fs::remove_dir_all(root.join(".git")).expect("the git directory");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &root.join(".git/packed-refs"),
            &format!("# pack-refs with: peeled fully-peeled sorted \n{HASH} refs/heads/main\n"),
        );
        let git_dir = resolve_git_dir(&root).expect("the .git directory");
        assert_eq!(resolve_head(&git_dir).as_deref(), Some(HASH));

        // A worktree's `gitdir:` file: HEAD follows the named directory
        // and the branch ref the common directory its commondir names.
        fs::remove_dir_all(root.join(".git")).expect("the git file");
        let gitdir = tmp.path().join("main/.git/worktrees/probe");
        write(
            &root.join(".git"),
            &format!("gitdir: {}\n", gitdir.display()),
        );
        write(&gitdir.join("HEAD"), "ref: refs/heads/row/probe\n");
        write(&gitdir.join("commondir"), "../..\n");
        write(&gitdir.join("index"), "");
        write(
            &tmp.path().join("main/.git/refs/heads/row/probe"),
            &format!("{HASH}\n"),
        );
        let git_dir = resolve_git_dir(&root).expect("the worktree's git directory");
        assert_ne!(
            git_dir,
            root.join(".git"),
            "the resolution follows the gitdir: file"
        );
        assert_eq!(resolve_head(&git_dir).as_deref(), Some(HASH));

        // No `.git`: none, never a guess.
        fs::remove_file(root.join(".git")).expect("the git file");
        assert_eq!(resolve_git_dir(&root), None);
    }

    /// `rerun_paths_follow_the_worktree_gitdir`: in the worktree case the
    /// rerun paths name the `gitdir:` directory's HEAD and index, not
    /// `.git/index`, and only paths that exist come back.
    #[test]
    fn rerun_paths_follow_the_worktree_gitdir() {
        let tmp = tempfile::tempdir().expect("the temp directory");
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).expect("the repo root");
        let gitdir = tmp.path().join("main/.git/worktrees/probe");
        write(
            &root.join(".git"),
            &format!("gitdir: {}\n", gitdir.display()),
        );
        write(&gitdir.join("HEAD"), "ref: refs/heads/row/probe\n");
        write(&gitdir.join("commondir"), "../..\n");
        write(&gitdir.join("index"), "");
        write(
            &tmp.path().join("main/.git/refs/heads/row/probe"),
            &format!("{HASH}\n"),
        );
        fs::create_dir_all(root.join("crates/hares-core")).expect("the crates tree");
        write(&root.join("Cargo.toml"), "[workspace]\n");
        write(&root.join("Cargo.lock"), "");

        let paths = rerun_paths(&root);
        let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();

        let head = gitdir.join("HEAD").display().to_string();
        let index = gitdir.join("index").display().to_string();
        assert!(
            names.contains(&head),
            "the gitdir's HEAD is a rerun path: {names:?}"
        );
        assert!(
            names.contains(&index),
            "the gitdir's index is a rerun path: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|p| *p == root.join(".git/index").display().to_string()),
            ".git/index is never a rerun path: {names:?}"
        );
        for name in ["crates", "Cargo.toml", "Cargo.lock"] {
            assert!(
                names.iter().any(|p| p.ends_with(name)),
                "{name} is a rerun path: {names:?}"
            );
        }

        // A workspace file that does not exist is not emitted: a missing
        // path would make cargo rerun the script on every build.
        fs::remove_file(root.join("Cargo.lock")).expect("the lock file");
        let paths = rerun_paths(&root);
        assert!(
            !paths.iter().any(|p| p.ends_with("Cargo.lock")),
            "only paths that exist are emitted: {:?}",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// `porcelain_parse_reports_dirty`: an empty porcelain output is
    /// clean, every non-empty line is a change.
    #[test]
    fn porcelain_parse_reports_dirty() {
        assert!(!porcelain_is_dirty(""));
        assert!(!porcelain_is_dirty("\n"));
        assert!(porcelain_is_dirty(" M crates/hares-core/src/result.rs\n"));
        assert!(porcelain_is_dirty("M  Cargo.toml\n?? build.rs\n"));
        assert!(porcelain_is_dirty(" M a\n M b\n"));
    }

    /// `build_info_commit_matches_source_tree`: the recorded commit
    /// equals the file-based resolution of the workspace root at test
    /// time.
    #[test]
    fn build_info_commit_matches_source_tree() {
        let root = repo_root();
        let git_dir = resolve_git_dir(&root).expect("the workspace is a git work tree");
        let commit = resolve_head(&git_dir).expect("HEAD resolves");
        let info = crate::BuildInfo::get();
        assert_eq!(info.commit, Some(commit.as_str()));
    }

    /// The recorded identity fields exist and match the build: the
    /// version is the package's, the profile the running one, the rustc
    /// line and the dirty flag recorded, and every feature a feature
    /// name.
    #[test]
    fn build_info_fields_exist_and_match_the_build() {
        let info = crate::BuildInfo::get();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        assert_eq!(info.profile, profile);
        assert!(
            info.rustc.starts_with("rustc ") && !info.rustc.is_empty(),
            "{}",
            info.rustc
        );
        let commit = info
            .commit
            .expect("the build records a commit in a git work tree");
        assert_eq!(commit.len(), 40, "the commit is a full hash: {commit}");
        assert!(
            commit.chars().all(|c| c.is_ascii_hexdigit()),
            "the commit is hex: {commit}"
        );
        assert!(
            info.dirty.is_some(),
            "the dirty flag is recorded, never guessed"
        );
        for feature in info.features {
            assert!(!feature.is_empty());
            assert!(
                feature.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "features are recorded in their package spelling: {feature}"
            );
        }
    }
}
