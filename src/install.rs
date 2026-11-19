//! The managed install of typescript-language-server: a pinned release of the
//! server and of TypeScript, downloaded from the npm registry, verified
//! against pinned digests, unpacked into the engine's tools directory.
//!
//! It is offered, never run on its own: the footer's Install item calls
//! [`INSTALLER`] after the person has read what it will do
//! ([`describe`]). It runs as an editor task (the host registers it), reports
//! what it is doing, and leaves nothing behind when it fails: each download
//! is checked against its pinned digest *before* anything is unpacked, and
//! the install is built beside its destination and moved into place whole.
//!
//! The layout is npm's own (`node_modules/typescript-language-server`,
//! `node_modules/typescript`), so the server finds TypeScript the way it does
//! in any project: as its sibling package.

use std::path::{Path, PathBuf};
use std::process::Command;

use pulsar_language::spec::{InstallContext, LocateError, ServerInstaller};
use sha2::{Digest as _, Sha256, Sha512};

use crate::archive::extract_tgz;
use crate::locate::{self, PACKAGE};

/// A digest a download must have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Digest {
    /// Lower-case hex, 64 characters. The verifier takes either; the pins use
    /// the one npm publishes (sha512), and a pin moved to a sha256 of the
    /// tarball needs no other change.
    #[allow(dead_code)]
    Sha256(&'static str),
    /// Lower-case hex, 128 characters (npm publishes this one, as its
    /// `dist.integrity`).
    Sha512(&'static str),
}

impl Digest {
    fn name(self) -> &'static str {
        match self {
            Self::Sha256(_) => "sha256",
            Self::Sha512(_) => "sha512",
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::Sha256(hex) | Self::Sha512(hex) => hex,
        }
    }

    /// The digest of `bytes`, in the same algorithm, as lower-case hex.
    pub(crate) fn of(self, bytes: &[u8]) -> String {
        match self {
            Self::Sha256(_) => hex(&Sha256::digest(bytes)),
            Self::Sha512(_) => hex(&Sha512::digest(bytes)),
        }
    }

    /// Whether `bytes` are the pinned download.
    pub(crate) fn matches(self, bytes: &[u8]) -> bool {
        self.of(bytes).eq_ignore_ascii_case(self.expected())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// One package of the install: where it comes from and what it must hash to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pin {
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub digest: Digest,
}

/// The pinned release (npm's published `dist.integrity` of each tarball, as
/// hex). Moving a pin is a deliberate edit of this table: new version, new
/// URL, new digest, and `RELEASE` so the install folder changes with it.
pub(crate) const PINS: [Pin; 2] = [
    Pin {
        name: "typescript-language-server",
        version: "4.4.0",
        url: "https://registry.npmjs.org/typescript-language-server/-/typescript-language-server-4.4.0.tgz",
        digest: Digest::Sha512(
            "7a75a1a658475fb3c0d2af8870a1c13294d087d2360666f72f8e6bc279004c732c67b60b76ecb209d3a655458949865f9166813222b08bf7b6150a38c6c29e5b",
        ),
    },
    Pin {
        name: "typescript",
        version: "5.9.3",
        url: "https://registry.npmjs.org/typescript/-/typescript-5.9.3.tgz",
        digest: Digest::Sha512(
            "8e5d6f6733c38a72ebf5e52ddc9feded5e8580d130f508ef04f772b33f4a7d00c3e357d0ac2d98e2f290762694a454f86d795bd511e12e9a7cc2d9ba3394e04b",
        ),
    },
];

/// The install folder's name: the pinned versions, so a new pin installs
/// beside the old one instead of over it.
fn release() -> String {
    PINS.iter()
        .map(|pin| format!("{}-{}", pin.name, pin.version))
        .collect::<Vec<_>>()
        .join("+")
}

/// `<tools>/typescript-language-server/<release>`.
pub(crate) fn install_dir() -> PathBuf {
    pulsar_install_layout::tools_dir()
        .join(PACKAGE)
        .join(release())
}

/// The server's entry point inside an install folder.
fn entry_in(dir: &Path) -> PathBuf {
    dir.join("node_modules").join(PACKAGE).join("lib").join("cli.mjs")
}

/// TypeScript's server inside an install folder (what the language server
/// launches as its `tsserver`).
fn tsserver_in(dir: &Path) -> PathBuf {
    dir.join("node_modules")
        .join("typescript")
        .join("lib")
        .join("tsserver.js")
}

/// Whether `dir` holds a complete install.
// A real machine path: the engine's tools directory, outside any project.
#[allow(clippy::disallowed_methods)]
fn is_complete(dir: &Path) -> bool {
    entry_in(dir).is_file() && tsserver_in(dir).is_file()
}

/// The installed server's entry point, when the person has installed it.
pub(crate) fn managed_entry() -> Option<PathBuf> {
    let dir = install_dir();
    is_complete(&dir).then(|| entry_in(&dir))
}

/// A `Command` for a helper process that must not flash a console window.
pub(crate) fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// What the person is told before they approve the install.
pub(crate) fn describe() -> String {
    format!(
        "Downloads {} {} and {} {} from the npm registry (registry.npmjs.org), checks each against a {} digest pinned in this plugin, and unpacks them into {}. Nothing is installed if a check fails. Needs Node.js {} or newer to run.",
        PINS[0].name,
        PINS[0].version,
        PINS[1].name,
        PINS[1].version,
        PINS[0].digest.name(),
        install_dir().display(),
        locate::MIN_NODE_MAJOR,
    )
}

/// The footer's install offer.
pub(crate) static INSTALLER: ServerInstaller = ServerInstaller {
    label: "Install typescript-language-server",
    describe,
    install: install,
};

/// Download every pin with `fetch` (url, destination file), verify, and
/// install into `dest`. `fetch` is the network: injected so the verification
/// and the unpacking are tested without one.
// A real machine path: the engine's tools directory, outside any project.
#[allow(clippy::disallowed_methods)]
pub(crate) fn install_pins(
    pins: &[Pin],
    dest: &Path,
    report: &dyn Fn(&str),
    fetch: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<(), LocateError> {
    let parent = dest
        .parent()
        .ok_or_else(|| LocateError::new("the install folder has no parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| LocateError::new(format!("could not create {}: {e}", parent.display())))?;
    let staging = parent.join(format!(
        "{}.partial",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| LocateError::new(format!("could not create {}: {e}", staging.display())))?;

    let result = (|| {
        for pin in pins {
            report(&format!("Downloading {} {}...", pin.name, pin.version));
            let file = staging.join(format!("{}-{}.tgz", pin.name, pin.version));
            fetch(pin.url, &file).map_err(LocateError::new)?;
            let bytes = std::fs::read(&file)
                .map_err(|e| LocateError::new(format!("could not read the download: {e}")))?;
            report(&format!("Verifying {} {}...", pin.name, pin.version));
            if !pin.digest.matches(&bytes) {
                return Err(LocateError::new(format!(
                    "the download of {} {} does not match the digest pinned in this plugin ({} {} expected, {} found); nothing was installed",
                    pin.name,
                    pin.version,
                    pin.digest.name(),
                    pin.digest.expected(),
                    pin.digest.of(&bytes),
                )));
            }
            report(&format!("Unpacking {} {}...", pin.name, pin.version));
            extract_tgz(&bytes, &staging.join("node_modules").join(pin.name))
                .map_err(|e| LocateError::new(format!("{} could not be unpacked: {e:#}", pin.name)))?;
            let _ = std::fs::remove_file(&file);
        }
        if !is_complete(&staging) {
            return Err(LocateError::new(
                "the unpacked packages do not hold the server's entry point (lib/cli.mjs) and TypeScript's tsserver; nothing was installed",
            ));
        }
        // Whole or not at all.
        let _ = std::fs::remove_dir_all(dest);
        std::fs::rename(&staging, dest)
            .map_err(|e| LocateError::new(format!("could not move the install into place: {e}")))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// The approved install: Node first (the server cannot run without it), then
/// the pinned packages.
fn install(ctx: &InstallContext) -> Result<(), LocateError> {
    let surroundings = locate::Surroundings::current();
    let Some(node) = locate::find_node(&surroundings, None) else {
        return Err(LocateError::missing(
            "Node.js was not found, and the TypeScript language server needs it to run. Install Node.js from https://nodejs.org first, then Install again.",
        ));
    };
    match locate::node_major(&node) {
        Some(major) if major < locate::MIN_NODE_MAJOR => {
            return Err(LocateError::missing(format!(
                "Node.js {major} is too old for the TypeScript language server, which needs {} or newer.",
                locate::MIN_NODE_MAJOR
            )))
        }
        _ => {}
    }
    install_pins(&PINS, &install_dir(), &|text| ctx.report(text), &download)
}

/// Fetch `url` into `dest` over HTTPS with the system's `curl` (every
/// supported OS ships it; Windows also has PowerShell, tried second). The
/// transport is not trusted: the caller checks the digest.
fn download(url: &str, dest: &Path) -> Result<(), String> {
    let curl = quiet_command("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https", "--max-time", "900"])
        .arg("--output")
        .arg(dest)
        .arg(url)
        .output();
    let mut failure = match curl {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
        Err(error) => format!("curl could not run: {error}"),
    };
    if cfg!(windows) {
        // Inside a PowerShell single-quoted string only `'` is special.
        let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
        let script = format!(
            "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -UseBasicParsing -Uri {} -OutFile {}",
            quote(url),
            quote(&dest.to_string_lossy())
        );
        match quiet_command("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                failure = format!("{failure}; powershell: {}", String::from_utf8_lossy(&output.stderr).trim())
            }
            Err(error) => failure = format!("{failure}; powershell could not run: {error}"),
        }
    }
    Err(format!("could not download {url}: {failure}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::tgz;
    use std::cell::RefCell;

    fn server_tgz() -> Vec<u8> {
        tgz(&[
            ("package.json", br#"{"name":"typescript-language-server","bin":{"typescript-language-server":"lib/cli.mjs"}}"#),
            ("lib/cli.mjs", b"// server"),
        ])
    }

    fn typescript_tgz() -> Vec<u8> {
        tgz(&[("package.json", br#"{"name":"typescript"}"#), ("lib/tsserver.js", b"// tsserver")])
    }

    /// Pins of the synthetic tarballs, with their real digests.
    fn pins_for(server: &[u8], ts: &[u8]) -> [Pin; 2] {
        // Leaked: a pin holds `&'static str`s and these are test data.
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        [
            Pin {
                name: "typescript-language-server",
                version: "1.0.0",
                url: "https://example.test/server.tgz",
                digest: Digest::Sha256(leak(Digest::Sha256("").of(server))),
            },
            Pin {
                name: "typescript",
                version: "1.0.0",
                url: "https://example.test/ts.tgz",
                digest: Digest::Sha512(leak(Digest::Sha512("").of(ts))),
            },
        ]
    }

    fn fetcher<'a>(server: &'a [u8], ts: &'a [u8]) -> impl Fn(&str, &Path) -> Result<(), String> + 'a {
        move |url, dest| {
            let body = if url.contains("server") { server } else { ts };
            std::fs::write(dest, body).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn a_verified_download_is_unpacked_whole_into_place() {
        let (server, ts) = (server_tgz(), typescript_tgz());
        let tools = tempfile::tempdir().unwrap();
        let dest = tools.path().join("typescript-language-server").join("release");
        let said = RefCell::new(Vec::new());
        install_pins(&pins_for(&server, &ts), &dest, &|t| said.borrow_mut().push(t.to_string()), &fetcher(&server, &ts))
            .unwrap();
        assert!(is_complete(&dest));
        assert_eq!(std::fs::read(entry_in(&dest)).unwrap(), b"// server");
        assert!(!tools.path().join("typescript-language-server").join("release.partial").exists());
        let said = said.into_inner();
        assert!(said.iter().any(|t| t.starts_with("Verifying")), "{said:?}");
        // The entry point is what discovery uses afterwards.
        assert_eq!(entry_in(&dest), dest.join("node_modules/typescript-language-server/lib/cli.mjs"));
    }

    #[test]
    fn a_checksum_mismatch_is_refused_and_nothing_is_left_behind() {
        let (server, ts) = (server_tgz(), typescript_tgz());
        let tools = tempfile::tempdir().unwrap();
        let dest = tools.path().join("tsls").join("release");
        // The pins say one thing, the network delivers another.
        let tampered = tgz(&[("package.json", b"{}"), ("lib/cli.mjs", b"// something else")]);
        let error = install_pins(
            &pins_for(&server, &ts),
            &dest,
            &|_| {},
            &fetcher(&tampered, &ts),
        )
        .unwrap_err();
        assert!(error.message.contains("does not match the digest pinned"), "{error}");
        assert!(error.message.contains("nothing was installed"), "{error}");
        assert!(!dest.exists(), "no install");
        assert!(!tools.path().join("tsls").join("release.partial").exists(), "no staging left");
        assert!(managed_like_entry_absent(&dest));
    }

    fn managed_like_entry_absent(dest: &Path) -> bool {
        !is_complete(dest)
    }

    #[test]
    fn a_failed_download_is_reported_and_leaves_nothing() {
        let (server, ts) = (server_tgz(), typescript_tgz());
        let tools = tempfile::tempdir().unwrap();
        let dest = tools.path().join("tsls").join("release");
        let error = install_pins(
            &pins_for(&server, &ts),
            &dest,
            &|_| {},
            &|url, _| Err(format!("could not download {url}: offline")),
        )
        .unwrap_err();
        assert!(error.message.contains("offline"), "{error}");
        assert!(!dest.exists());
    }

    #[test]
    fn an_older_install_is_replaced_whole_not_merged() {
        let (server, ts) = (server_tgz(), typescript_tgz());
        let tools = tempfile::tempdir().unwrap();
        let dest = tools.path().join("tsls").join("release");
        std::fs::create_dir_all(dest.join("node_modules/stale")).unwrap();
        std::fs::write(dest.join("node_modules/stale/old.js"), "old").unwrap();
        install_pins(&pins_for(&server, &ts), &dest, &|_| {}, &fetcher(&server, &ts)).unwrap();
        assert!(is_complete(&dest));
        assert!(!dest.join("node_modules/stale").exists());
    }

    #[test]
    fn the_pins_are_well_formed_and_named_in_what_the_person_approves() {
        for pin in PINS {
            assert!(pin.url.starts_with("https://registry.npmjs.org/"), "{}", pin.url);
            assert!(pin.url.contains(pin.version), "{}", pin.url);
            let want = match pin.digest {
                Digest::Sha256(_) => 64,
                Digest::Sha512(_) => 128,
            };
            let hex = pin.digest.expected();
            assert_eq!(hex.len(), want, "{}", pin.name);
            assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", pin.name);
        }
        let text = describe();
        assert!(text.contains("registry.npmjs.org") && text.contains("4.4.0") && text.contains("5.9.3"), "{text}");
        assert!(install_dir().ends_with(release()));
    }

    #[test]
    fn digests_know_their_known_answers() {
        // sha256("abc") and sha512("abc"), the standard test vectors.
        assert!(Digest::Sha256("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").matches(b"abc"));
        assert!(Digest::Sha512(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        )
        .matches(b"abc"));
        assert!(!Digest::Sha256("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").matches(b"abd"));
    }
}
