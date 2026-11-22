//! Finding typescript-language-server, and how to launch it.
//!
//! Order (LANGUAGE_PLUGINS_PLAN D14): the person's `path` setting, the
//! project's own `node_modules`, `PATH`, then the managed install the person
//! approved (`install.rs`). A configured path is used or reported, never
//! silently replaced by another binary.
//!
//! The server is a Node program. It is launched through Node's own entry point
//! (`node <package>/lib/cli.mjs --stdio`), not through the `.cmd` shim npm
//! writes on Windows, so no `cmd.exe` window or batch-file quoting is involved
//! (Q10). The shim (or the script, on Unix) is only the fallback when no entry
//! point can be found beside it.
//!
//! Nothing here is silent: no Node, or no server, is a [`LocateError`] marked
//! missing, whose message says what is missing and what to do about it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pulsar_language::spec::{LocateError, ServerCommand};

/// The npm package and its executable.
pub(crate) const PACKAGE: &str = "typescript-language-server";

/// The oldest Node the server is launched with (what its pinned release asks).
pub(crate) const MIN_NODE_MAJOR: u32 = 18;

/// Where a located server came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Configured,
    ProjectLocal,
    Path,
    Managed,
}

/// A server that can be launched.
#[derive(Clone, Debug)]
pub(crate) struct Located {
    pub command: ServerCommand,
    pub source: Source,
    /// Launched through a `.cmd`/script shim because no Node entry point was
    /// found beside it (the fallback).
    pub via_shim: bool,
}

/// What the search looks at: the process's surroundings, passed in so the
/// search is a function of them (and testable).
pub(crate) struct Surroundings {
    /// The directories of `PATH`.
    pub path_dirs: Vec<PathBuf>,
    /// Places Node is installed that `PATH` may not list.
    pub node_dirs: Vec<PathBuf>,
    /// The managed install's entry point, when it is installed.
    pub managed_entry: Option<PathBuf>,
}

impl Surroundings {
    /// The real process's.
    pub(crate) fn current() -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let mut node_dirs = Vec::new();
        for var in [
            "ProgramFiles",
            "ProgramW6432",
            "LOCALAPPDATA",
            "NVM_SYMLINK",
            "NVM_HOME",
        ] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                node_dirs.push(base.join("nodejs"));
                node_dirs.push(base.join("Programs").join("nodejs"));
                node_dirs.push(base);
            }
        }
        for fixed in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"] {
            node_dirs.push(PathBuf::from(fixed));
        }
        Self {
            path_dirs,
            node_dirs,
            managed_entry: crate::install::managed_entry(),
        }
    }
}

fn node_file_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["node.exe"]
    } else {
        &["node"]
    }
}

fn server_file_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "typescript-language-server.cmd",
            "typescript-language-server.exe",
            "typescript-language-server",
        ]
    } else {
        &["typescript-language-server"]
    }
}

// Real machine paths (PATH, the person's Node install), outside any project.
#[allow(clippy::disallowed_methods)]
fn first_file(dirs: impl IntoIterator<Item = PathBuf>, names: &[&str]) -> Option<PathBuf> {
    dirs.into_iter().find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// `node`, preferring the one beside the server (a global prefix keeps them
/// together), then `PATH`, then the usual install places.
pub(crate) fn find_node(surroundings: &Surroundings, beside: Option<&Path>) -> Option<PathBuf> {
    let dirs = beside
        .into_iter()
        .map(Path::to_path_buf)
        .chain(surroundings.path_dirs.iter().cloned())
        .chain(surroundings.node_dirs.iter().cloned());
    first_file(dirs, node_file_names())
}

/// The Node's major version (`v22.22.3` is 22), asked of the executable.
pub(crate) fn node_major(node: &Path) -> Option<u32> {
    let output = crate::install::quiet_command(node)
        .arg("--version")
        .output()
        .ok()?;
    parse_node_major(&String::from_utf8_lossy(&output.stdout))
}

pub(crate) fn parse_node_major(text: &str) -> Option<u32> {
    text.trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// What a located file turns out to be.
#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    /// A JavaScript entry point: run it with Node.
    Entry(PathBuf),
    /// Run the file itself (a shim or script whose entry point is not found).
    Direct(PathBuf),
}

fn is_js(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs"))
}

/// The entry point of a package directory: its `package.json`'s `bin`, else
/// `lib/cli.mjs`.
// Real machine paths: the person's Node packages, outside any project.
#[allow(clippy::disallowed_methods)]
pub(crate) fn package_entry(package_dir: &Path) -> Option<PathBuf> {
    if let Ok(text) = std::fs::read_to_string(package_dir.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let bin = match json.get("bin") {
                Some(serde_json::Value::String(path)) => Some(path.clone()),
                Some(serde_json::Value::Object(map)) => {
                    map.get(PACKAGE).and_then(|v| v.as_str()).map(str::to_owned)
                }
                _ => None,
            };
            // `bin` is a slash-separated relative path (`./lib/cli.mjs`).
            let entry = bin.map(|bin| {
                bin.split('/')
                    .fold(normalize(package_dir), |path, part| path.join(part))
            });
            if let Some(entry) = entry.map(|e| normalize(&e)).filter(|e| e.is_file()) {
                return Some(entry);
            }
        }
    }
    let fallback = normalize(&package_dir.join("lib").join("cli.mjs"));
    fallback.is_file().then_some(fallback)
}

/// `path` with `.` and `..` folded away, without asking the disk (a path
/// handed to Node should read as the plain path it is).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolve a located file (a shim, a link, a script, an entry) to how it runs.
// Real machine paths: the person's Node packages, outside any project.
#[allow(clippy::disallowed_methods)]
fn resolve(file: &Path) -> Resolved {
    if is_js(file) {
        return Resolved::Entry(file.to_path_buf());
    }
    // A Unix `bin/typescript-language-server` is a link to the entry point.
    if let Ok(real) = std::fs::canonicalize(file) {
        if is_js(&real) {
            return Resolved::Entry(real);
        }
    }
    let Some(dir) = file.parent() else {
        return Resolved::Direct(file.to_path_buf());
    };
    // Where npm puts the package relative to its shim:
    // - Windows global prefix: `<prefix>/typescript-language-server.cmd`,
    //   `<prefix>/node_modules/typescript-language-server`;
    // - a project's `node_modules/.bin/<shim>`: `node_modules/typescript-language-server`;
    // - Unix global prefix: `<prefix>/bin/<shim>`,
    //   `<prefix>/lib/node_modules/typescript-language-server`.
    let candidates = [
        dir.join("node_modules").join(PACKAGE),
        dir.join("..").join(PACKAGE),
        dir.join("..")
            .join("lib")
            .join("node_modules")
            .join(PACKAGE),
    ];
    for candidate in candidates {
        if let Some(entry) = package_entry(&candidate) {
            return Resolved::Entry(entry);
        }
    }
    Resolved::Direct(file.to_path_buf())
}

fn missing_node() -> LocateError {
    LocateError::missing(format!(
        "Node.js was not found. TypeScript support runs the TypeScript language server on Node.js {MIN_NODE_MAJOR} or newer: install it from https://nodejs.org and restart the editor, or set this server's executable path in Settings."
    ))
}

fn missing_server(installable: bool) -> LocateError {
    let install = if installable {
        "Use Install on this entry to download a pinned, checksum-verified copy, or r"
    } else {
        "R"
    };
    LocateError::missing(format!(
        "The TypeScript language server (typescript-language-server) was not found: it is not in the project's node_modules and not on PATH. {install}un `npm i -g typescript typescript-language-server`, or set its path in Settings."
    ))
}

fn old_node(major: u32) -> LocateError {
    LocateError::missing(format!(
        "Node.js {major} is too old for the TypeScript language server, which needs {MIN_NODE_MAJOR} or newer. Update Node.js from https://nodejs.org, or set this server's executable path in Settings."
    ))
}

/// A launchable command for `resolved`, with Node found beside or on `PATH`.
fn command_for(
    resolved: Resolved,
    surroundings: &Surroundings,
    node_major_of: &dyn Fn(&Path) -> Option<u32>,
) -> Result<(ServerCommand, bool), LocateError> {
    match resolved {
        Resolved::Entry(entry) => {
            let beside = entry
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            let node = find_node(surroundings, beside.as_deref()).ok_or_else(missing_node)?;
            match node_major_of(&node) {
                Some(major) if major < MIN_NODE_MAJOR => return Err(old_node(major)),
                _ => {}
            }
            let command = ServerCommand::new(node)
                .arg(entry.into_os_string())
                .arg(OsString::from("--stdio"));
            Ok((command, false))
        }
        // The shim (a `.cmd`, or a script with its own shebang) runs itself.
        Resolved::Direct(file) => Ok((ServerCommand::new(file).arg("--stdio"), true)),
    }
}

/// Find the server and say how to launch it.
// Real machine paths: the project's node_modules, PATH, the person's setting.
#[allow(clippy::disallowed_methods)]
pub(crate) fn locate_with(
    surroundings: &Surroundings,
    project_root: &Path,
    configured: Option<&Path>,
    node_major_of: &dyn Fn(&Path) -> Option<u32>,
) -> Result<Located, LocateError> {
    let launch = |file: &Path, source: Source| -> Result<Located, LocateError> {
        let (command, via_shim) = command_for(resolve(file), surroundings, node_major_of)?;
        Ok(Located {
            command,
            source,
            via_shim,
        })
    };

    // 1. The person's setting: used, or reported; never replaced.
    if let Some(path) = configured {
        if !path.is_file() {
            return Err(LocateError::missing(format!(
                "The configured TypeScript language server path {} does not exist. Fix or clear it in Settings.",
                path.display()
            )));
        }
        return launch(path, Source::Configured);
    }

    // 2. The project's own: `node_modules/.bin/<shim>`, else the package.
    let modules = project_root.join("node_modules");
    if let Some(shim) = first_file([modules.join(".bin")], server_file_names()) {
        return launch(&shim, Source::ProjectLocal);
    }
    if let Some(entry) = package_entry(&modules.join(PACKAGE)) {
        return launch(&entry, Source::ProjectLocal);
    }

    // 3. PATH.
    if let Some(found) = first_file(surroundings.path_dirs.iter().cloned(), server_file_names()) {
        return launch(&found, Source::Path);
    }

    // 4. The managed install the person approved.
    if let Some(entry) = &surroundings.managed_entry {
        return launch(entry, Source::Managed);
    }

    // Nothing: say what is missing. Without Node there is nothing to install
    // it with either, which the message says first.
    if find_node(surroundings, None).is_none() {
        return Err(missing_node());
    }
    Err(missing_server(true))
}

/// Find the server on this machine, without installing anything.
pub(crate) fn locate_here(
    project_root: &Path,
    configured: Option<&Path>,
) -> Result<Located, LocateError> {
    locate_with(
        &Surroundings::current(),
        project_root,
        configured,
        &node_major,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn node_name() -> &'static str {
        node_file_names()[0]
    }

    fn shim_name() -> &'static str {
        server_file_names()[0]
    }

    /// An npm-style layout: `<prefix>/<shim>` beside
    /// `<prefix>/node_modules/typescript-language-server/{package.json,lib/cli.mjs}`,
    /// and a `node` beside the shim (as a global prefix keeps them).
    fn windows_style_prefix(root: &Path) -> PathBuf {
        touch(&root.join(node_name()), "");
        touch(&root.join(shim_name()), "@echo off");
        let package = root.join("node_modules").join(PACKAGE);
        touch(
            &package.join("package.json"),
            r#"{"bin":{"typescript-language-server":"lib/cli.mjs"}}"#,
        );
        touch(&package.join("lib").join("cli.mjs"), "// server");
        package.join("lib").join("cli.mjs")
    }

    fn surroundings(path_dirs: Vec<PathBuf>) -> Surroundings {
        Surroundings {
            path_dirs,
            node_dirs: Vec::new(),
            managed_entry: None,
        }
    }

    const NODE_22: &dyn Fn(&Path) -> Option<u32> = &|_| Some(22);

    #[test]
    fn a_shim_on_path_is_run_through_nodes_entry_point_not_through_the_shim() {
        let prefix = tempfile::tempdir().unwrap();
        let entry = windows_style_prefix(prefix.path());
        let project = tempfile::tempdir().unwrap();
        let located = locate_with(
            &surroundings(vec![prefix.path().to_path_buf()]),
            project.path(),
            None,
            NODE_22,
        )
        .unwrap();
        assert_eq!(located.source, Source::Path);
        assert!(!located.via_shim, "the entry point was found, so no shim");
        let program = located
            .command
            .program
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        assert!(program.starts_with("node"), "{program}");
        assert!(!program.ends_with(".cmd") && !program.ends_with(".bat"));
        assert_eq!(located.command.args[0], entry.as_os_str());
        assert_eq!(located.command.args[1], OsString::from("--stdio"));
        // Node is the one beside the shim, not some other.
        assert_eq!(located.command.program.parent(), Some(prefix.path()));
    }

    #[test]
    fn a_shim_with_no_entry_point_beside_it_is_the_fallback() {
        let prefix = tempfile::tempdir().unwrap();
        touch(&prefix.path().join(shim_name()), "@echo off");
        let project = tempfile::tempdir().unwrap();
        let located = locate_with(
            &surroundings(vec![prefix.path().to_path_buf()]),
            project.path(),
            None,
            NODE_22,
        )
        .unwrap();
        assert!(located.via_shim);
        assert_eq!(located.command.program, prefix.path().join(shim_name()));
        assert_eq!(located.command.args, vec![OsString::from("--stdio")]);
    }

    #[test]
    fn the_projects_node_modules_beats_path() {
        let global = tempfile::tempdir().unwrap();
        windows_style_prefix(global.path());
        let project = tempfile::tempdir().unwrap();
        let modules = project.path().join("node_modules");
        touch(&modules.join(".bin").join(shim_name()), "shim");
        let package = modules.join(PACKAGE);
        touch(&package.join("package.json"), r#"{"bin":"./lib/cli.mjs"}"#);
        touch(&package.join("lib").join("cli.mjs"), "// local");
        let located = locate_with(
            &surroundings(vec![global.path().to_path_buf()]),
            project.path(),
            None,
            NODE_22,
        )
        .unwrap();
        assert_eq!(located.source, Source::ProjectLocal);
        assert_eq!(
            located.command.args[0],
            package.join("lib").join("cli.mjs").as_os_str()
        );
    }

    #[test]
    fn the_configured_path_wins_and_a_bad_one_is_reported_not_replaced() {
        let prefix = tempfile::tempdir().unwrap();
        windows_style_prefix(prefix.path());
        let project = tempfile::tempdir().unwrap();
        let mine = project.path().join("mine").join("cli.mjs");
        touch(&mine, "// mine");
        touch(&project.path().join("mine").join(node_name()), "");
        let around = surroundings(vec![prefix.path().to_path_buf()]);
        let located = locate_with(&around, project.path(), Some(&mine), NODE_22).unwrap();
        assert_eq!(located.source, Source::Configured);
        assert_eq!(located.command.args[0], mine.as_os_str());

        let error = locate_with(
            &around,
            project.path(),
            Some(&project.path().join("nope")),
            NODE_22,
        )
        .unwrap_err();
        assert!(
            error.missing && error.message.contains("does not exist"),
            "{error}"
        );
    }

    #[test]
    fn the_managed_install_is_the_last_resort() {
        let managed = tempfile::tempdir().unwrap();
        let entry = managed.path().join("lib").join("cli.mjs");
        touch(&entry, "// managed");
        let node = tempfile::tempdir().unwrap();
        touch(&node.path().join(node_name()), "");
        let project = tempfile::tempdir().unwrap();
        let around = Surroundings {
            path_dirs: vec![node.path().to_path_buf()],
            node_dirs: Vec::new(),
            managed_entry: Some(entry.clone()),
        };
        let located = locate_with(&around, project.path(), None, NODE_22).unwrap();
        assert_eq!(located.source, Source::Managed);
        assert_eq!(located.command.args[0], entry.as_os_str());
    }

    #[test]
    fn no_server_is_an_actionable_missing_status_that_offers_the_install() {
        let node = tempfile::tempdir().unwrap();
        touch(&node.path().join(node_name()), "");
        let project = tempfile::tempdir().unwrap();
        let error = locate_with(
            &surroundings(vec![node.path().to_path_buf()]),
            project.path(),
            None,
            NODE_22,
        )
        .unwrap_err();
        assert!(error.missing);
        assert!(
            error.message.contains("typescript-language-server"),
            "{error}"
        );
        assert!(
            error.message.contains("Install"),
            "it points at the install action: {error}"
        );
        assert!(
            error.message.contains("npm i -g"),
            "and at the manual way: {error}"
        );
    }

    #[test]
    fn no_node_is_reported_first_and_says_where_to_get_it() {
        let project = tempfile::tempdir().unwrap();
        let error =
            locate_with(&surroundings(Vec::new()), project.path(), None, NODE_22).unwrap_err();
        assert!(error.missing);
        assert!(error.message.contains("Node.js was not found"), "{error}");
        assert!(error.message.contains("nodejs.org"), "{error}");

        // A server that is there but with no Node to run it is the same report.
        let prefix = tempfile::tempdir().unwrap();
        let package = prefix.path().join("node_modules").join(PACKAGE);
        touch(&package.join("lib").join("cli.mjs"), "//");
        touch(&prefix.path().join(shim_name()), "shim");
        let error = locate_with(
            &surroundings(vec![prefix.path().to_path_buf()]),
            project.path(),
            None,
            NODE_22,
        )
        .unwrap_err();
        assert!(error.message.contains("Node.js was not found"), "{error}");
    }

    #[test]
    fn a_node_older_than_the_servers_minimum_is_reported() {
        let prefix = tempfile::tempdir().unwrap();
        windows_style_prefix(prefix.path());
        let project = tempfile::tempdir().unwrap();
        let error = locate_with(
            &surroundings(vec![prefix.path().to_path_buf()]),
            project.path(),
            None,
            &|_| Some(14),
        )
        .unwrap_err();
        assert!(
            error.missing && error.message.contains("Node.js 14 is too old"),
            "{error}"
        );
    }

    #[test]
    fn node_versions_parse() {
        assert_eq!(parse_node_major("v22.22.3\n"), Some(22));
        assert_eq!(parse_node_major("18.0.0"), Some(18));
        assert_eq!(parse_node_major("garbage"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_unix_link_to_the_entry_point_resolves_to_it() {
        let prefix = tempfile::tempdir().unwrap();
        let package = prefix.path().join("lib/node_modules").join(PACKAGE);
        touch(&package.join("lib").join("cli.mjs"), "//");
        std::fs::create_dir_all(prefix.path().join("bin")).unwrap();
        std::os::unix::fs::symlink(
            package.join("lib").join("cli.mjs"),
            prefix.path().join("bin").join(PACKAGE),
        )
        .unwrap();
        assert!(matches!(
            resolve(&prefix.path().join("bin").join(PACKAGE)),
            Resolved::Entry(entry) if entry.ends_with("cli.mjs")
        ));
    }
}
