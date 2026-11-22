//! Unpacking an npm package tarball (`.tgz`): gzip around a tar stream whose
//! entries sit under one top folder (`package/`).
//!
//! Only what a package holds is accepted: regular files and directories. An
//! entry that is a link or a device, or whose path is absolute, has a `..` or a
//! drive prefix, is refused (a tarball is untrusted input until proven
//! otherwise, even with a verified digest: the digest says it is the pinned
//! one, not that this reader may be careless).

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

const BLOCK: usize = 512;

/// One file of a tarball.
struct Entry {
    path: PathBuf,
    data: Vec<u8>,
}

/// Unpack `tgz` into `dest`, dropping the first path component of every entry
/// (npm's `package/`).
// A real machine path: the engine's tools directory, outside any project.
#[allow(clippy::disallowed_methods)]
pub(crate) fn extract_tgz(tgz: &[u8], dest: &Path) -> Result<()> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(tgz)
        .read_to_end(&mut tar)
        .context("the download is not a gzip file")?;
    for entry in entries(&tar)? {
        let target = dest.join(&entry.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(&target, &entry.data)
            .with_context(|| format!("could not write {}", target.display()))?;
    }
    Ok(())
}

/// The files of a tar stream, with the first path component stripped.
fn entries(tar: &[u8]) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut at = 0;
    // A path set by the preceding pax (`x`) or GNU long-name (`L`) record.
    let mut long_name: Option<String> = None;
    while at + BLOCK <= tar.len() {
        let header = &tar[at..at + BLOCK];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let size =
            octal(&header[124..136]).ok_or_else(|| anyhow!("a tar header has a bad size"))?;
        let kind = header[156];
        let body_start = at + BLOCK;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= tar.len())
            .ok_or_else(|| anyhow!("the archive is cut short"))?;
        let body = &tar[body_start..body_end];
        at = body_start + size.div_ceil(BLOCK) * BLOCK;

        match kind {
            // A pax extended header: `<len> key=value\n` records.
            b'x' => {
                long_name = pax_path(body);
                continue;
            }
            // Global pax header: nothing we need.
            b'g' => continue,
            // GNU long name.
            b'L' => {
                long_name = Some(
                    String::from_utf8_lossy(body)
                        .trim_end_matches('\0')
                        .to_string(),
                );
                continue;
            }
            _ => {}
        }
        let name = long_name.take().unwrap_or_else(|| header_name(header));
        match kind {
            b'5' => continue, // a directory: made by the files in it
            b'0' | 0 => {
                let path = safe_path(&name)?;
                if let Some(path) = path {
                    out.push(Entry {
                        path,
                        data: body.to_vec(),
                    });
                }
            }
            other => bail!(
                "the archive holds an entry of kind {:?} ({name}); only files and folders are accepted",
                other as char
            ),
        }
    }
    Ok(out)
}

/// The name in a ustar header: `prefix/name`.
fn header_name(header: &[u8]) -> String {
    let text = |range: std::ops::Range<usize>| {
        let bytes = &header[range];
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).to_string()
    };
    let name = text(0..100);
    let is_ustar = &header[257..262] == b"ustar";
    let prefix = if is_ustar {
        text(345..500)
    } else {
        String::new()
    };
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

/// The `path` record of a pax header body.
fn pax_path(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    text.lines().find_map(|line| {
        let record = line.split_once(' ')?.1;
        record.strip_prefix("path=").map(str::to_string)
    })
}

/// Octal digits, space or NUL padded.
fn octal(field: &[u8]) -> Option<usize> {
    let text: String = field
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(text, 8).ok()
}

/// The entry's path below the package folder: `None` for the folder itself,
/// an error for anything that could leave the destination.
fn safe_path(name: &str) -> Result<Option<PathBuf>> {
    let mut components = Path::new(name).components();
    // npm's `package/`.
    match components.next() {
        Some(Component::Normal(_)) => {}
        _ => bail!("the archive holds an unsafe path: {name}"),
    }
    let mut path = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => bail!("the archive holds an unsafe path: {name}"),
        }
    }
    if name.contains('\\') && cfg!(windows) {
        bail!("the archive holds an unsafe path: {name}");
    }
    Ok((!path.as_os_str().is_empty()).then_some(path))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// A ustar entry of `kind` with `body`.
    fn tar_entry(name: &str, kind: u8, body: &[u8]) -> Vec<u8> {
        let mut header = vec![0u8; BLOCK];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        let mut entry = header;
        entry.extend_from_slice(body);
        entry.resize(entry.len().div_ceil(BLOCK) * BLOCK, 0);
        entry
    }

    /// A `.tgz` of `files` under `package/`, as npm packs them.
    pub(crate) fn tgz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        tar.extend(tar_entry("package/", b'5', b""));
        for (name, body) in files {
            tar.extend(tar_entry(&format!("package/{name}"), b'0', body));
        }
        tar.extend(vec![0u8; BLOCK * 2]);
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn files_are_unpacked_below_the_stripped_top_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tgz(&[
            ("package.json", b"{\"name\":\"x\"}"),
            ("lib/cli.mjs", b"console.log(1)"),
        ]);
        extract_tgz(&archive, dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("package.json")).unwrap(),
            b"{\"name\":\"x\"}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("lib/cli.mjs")).unwrap(),
            b"console.log(1)"
        );
    }

    #[test]
    fn a_path_that_leaves_the_destination_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "package/../evil.js",
            "/abs/evil.js",
            "package/lib/../../evil.js",
        ] {
            let mut tar = tar_entry(name, b'0', b"x");
            tar.extend(vec![0u8; BLOCK * 2]);
            let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
            gz.write_all(&tar).unwrap();
            let archive = gz.finish().unwrap();
            assert!(extract_tgz(&archive, dir.path()).is_err(), "{name}");
        }
        assert!(!dir.path().parent().unwrap().join("evil.js").exists());
    }

    #[test]
    fn a_link_is_refused_and_a_non_gzip_download_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar = tar_entry("package/link", b'2', b"");
        tar.extend(vec![0u8; BLOCK * 2]);
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&tar).unwrap();
        let err = extract_tgz(&gz.finish().unwrap(), dir.path()).unwrap_err();
        assert!(err.to_string().contains("only files and folders"), "{err}");
        assert!(extract_tgz(b"<html>not a tarball</html>", dir.path()).is_err());
    }

    #[test]
    fn a_pax_path_overrides_the_header_name() {
        let record = "30 path=package/lib/long-name.mjs\n";
        let mut tar = tar_entry("pax", b'x', record.as_bytes());
        tar.extend(tar_entry("short", b'0', b"data"));
        tar.extend(vec![0u8; BLOCK * 2]);
        let found = entries(&tar).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, PathBuf::from("lib/long-name.mjs"));
    }
}
