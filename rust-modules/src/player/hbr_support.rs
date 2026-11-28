//! HBR support on rooted webOS 6 TVs, installed from Settings only when the owner asks for it.
//!
//! Native Jelly ships no kernel module. When the owner turns on *Lossless audio to receiver* and then
//! explicitly chooses *Download and install* under the HBR row (whose description says what that
//! installs and that it runs in the TV's kernel), this:
//!
//! 1. downloads `earc_route-4.4.ko` and its loader from this repository at the pinned commit
//!    [`SOURCE_COMMIT`] (`companion/hbr/earc_route/`, GPL-2.0-only, complete source beside it);
//! 2. refuses either file unless its SHA-256 is the one compiled in here ([`FILES`]);
//! 3. writes them, with the installer compiled into this binary (`companion/hbr/nativejelly-hbr.sh`,
//!    the same script the companion app runs), to a directory under the runtime root;
//! 4. runs `nativejelly-hbr.sh install DIR KO_SHA LOADER_SHA` as root through Homebrew Channel
//!    (`nj_platform::tv::root_exec`). The installer copies the files into its own root-owned
//!    directory and checks the copies against the same hashes before loading anything, so nothing
//!    that changes the download directory afterwards can reach the kernel.
//!
//! The module's loader checks the kernel it was built for and refuses any other; the module refuses
//! to arm on instructions it does not expect. Once installed, the module's lease node in this app's
//! sandbox is what [`super::es_carrier::hbr_available`] sees. *Remove* runs the installer's
//! `remove`, which deletes everything it installed and switches the module off.

use std::path::{Path, PathBuf};

/// The commit the module is downloaded from: `companion/hbr` as it landed (`4efd65aa`).
pub(crate) const SOURCE_COMMIT: &str = "4efd65aab44e34c4a8cfc54c14eb065639f60561";
const RAW_BASE: &str = "https://raw.githubusercontent.com/sostk/native-jelly";

/// (file name in the installer's layout, path in the repository, SHA-256)
pub(crate) const FILES: [(&str, &str, &str); 2] = [
    ("earc_route-4.4.ko", "companion/hbr/earc_route/out/earc_route-4.4.ko",
        "f0795a347d5a504bf999b01e6df00a31f3c1d7a66f5cfcb3bab8379bc3ee064d"),
    ("earc-route-load.sh", "companion/hbr/earc_route/earc-route-load.sh",
        "709777ec9a690b58cea9b0273101503dc94aa3498ba94c8eb72f0bbf37355df9"),
];

/// The installer, the companion app's own copy.
const INSTALLER: &str = include_str!("../../../companion/hbr/nativejelly-hbr.sh");
const INSTALLER_NAME: &str = "nativejelly-hbr.sh";

/// Why an install or removal did not finish, for the Settings row's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HbrError {
    /// Homebrew Channel did not run the command: not a rooted TV, or it is not installed.
    NoRoot,
    /// A file could not be downloaded, or it was not the pinned file.
    Download,
    /// The installer ran and refused, with its reason (`hbr: REFUSED: <why>`).
    Refused(String),
}

/// May this TV show the HBR row at all? The module is built for webOS 6 (kernel 4.4) only.
pub(crate) fn offered() -> bool {
    super::es_carrier::carrier_firmware()
}

/// Download, verify and install. Blocking (a download and a root command): a worker thread's job.
pub(crate) fn install() -> Result<(), HbrError> {
    let dir = stage_dir();
    let result = stage(&dir, fetch).and_then(|()| run(&install_command(&dir)));
    let _ = std::fs::remove_dir_all(&dir);
    nj_base::eventlog::log(&format!("hbr: install: {result:?}"));
    result
}

/// Remove what [`install`] (or the companion app) installed.
pub(crate) fn remove() -> Result<(), HbrError> {
    let dir = stage_dir();
    let result = write_installer(&dir).map_err(|_| HbrError::Refused("could not write the installer".into()))
        .and_then(|()| run(&format!("sh '{}' remove", dir.join("hbr").join(INSTALLER_NAME).display())));
    let _ = std::fs::remove_dir_all(&dir);
    nj_base::eventlog::log(&format!("hbr: remove: {result:?}"));
    result
}

/// Under the runtime root, which the root shell outside the sandbox sees at the same path. Not
/// `nativejelly-*`: on the stable install the runtime root is `/tmp` itself, where that prefix
/// marks an automated boot.
fn stage_dir() -> PathBuf {
    nj_base::paths::runtime_dir().join("nj-hbr")
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The download of one repository file at [`SOURCE_COMMIT`].
fn fetch(repo_path: &str) -> Option<Vec<u8>> {
    let url = format!("{RAW_BASE}/{SOURCE_COMMIT}/{repo_path}");
    let resp = nj_net::net::https_get_public(&url)?;
    resp.ok().then_some(resp.body)
}

/// Write the pinned files (each only if its bytes hash to the pinned SHA-256) and the installer
/// into `dir/hbr/`, the layout `nativejelly-hbr.sh install DIR` reads.
fn stage(dir: &Path, get: impl Fn(&str) -> Option<Vec<u8>>) -> Result<(), HbrError> {
    let _ = std::fs::remove_dir_all(dir);
    write_installer(dir).map_err(|_| HbrError::Download)?;
    for (name, repo_path, sha) in FILES {
        let bytes = get(repo_path).ok_or(HbrError::Download)?;
        if hex(&nj_base::sha256::sha256(&bytes)) != sha {
            nj_base::eventlog::log(&format!("hbr: {repo_path} does not match its pinned SHA-256"));
            return Err(HbrError::Download);
        }
        std::fs::write(dir.join("hbr").join(name), bytes).map_err(|_| HbrError::Download)?;
    }
    Ok(())
}

fn write_installer(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join("hbr"))?;
    std::fs::write(dir.join("hbr").join(INSTALLER_NAME), INSTALLER)
}

fn install_command(dir: &Path) -> String {
    let d = dir.display();
    format!("sh '{d}/hbr/{INSTALLER_NAME}' install '{d}' {} {}", FILES[0].2, FILES[1].2)
}

/// Run an installer command as root and read its last line.
fn run(command: &str) -> Result<(), HbrError> {
    let out = nj_platform::tv::root_exec(command).map_err(|why| {
        nj_base::eventlog::log(&format!("hbr: root command not run: {why}"));
        HbrError::NoRoot
    })?;
    outcome(&out)
}

/// `hbr: active` / `hbr: removed` is success; `hbr: REFUSED: <why>` and anything else is not.
fn outcome(out: &str) -> Result<(), HbrError> {
    let line = out.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    match line {
        "hbr: active" | "hbr: removed" => Ok(()),
        _ => Err(HbrError::Refused(line.strip_prefix("hbr: REFUSED: ").unwrap_or(line).to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hashes compiled in are those of the files in this tree, which are the ones at
    /// SOURCE_COMMIT (`companion/hbr/earc_route` has not changed them since): a rebuilt module has
    /// to update both, or every install refuses.
    #[test]
    fn the_pinned_hashes_are_the_repository_files() {
        let ko = include_bytes!("../../../companion/hbr/earc_route/out/earc_route-4.4.ko");
        let loader = include_bytes!("../../../companion/hbr/earc_route/earc-route-load.sh");
        assert_eq!(hex(&nj_base::sha256::sha256(ko)), FILES[0].2);
        assert_eq!(hex(&nj_base::sha256::sha256(loader)), FILES[1].2);
        let recorded = include_str!("../../../companion/hbr/earc_route/out/earc_route-4.4.json");
        assert!(recorded.contains(FILES[0].2), "earc_route-4.4.json records another module");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nj-hbr-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn only_the_pinned_files_are_staged() {
        let dir = scratch("good");
        let real = |path: &str| -> Option<Vec<u8>> {
            Some(match path {
                p if p.ends_with(".ko") => include_bytes!("../../../companion/hbr/earc_route/out/earc_route-4.4.ko").to_vec(),
                _ => include_bytes!("../../../companion/hbr/earc_route/earc-route-load.sh").to_vec(),
            })
        };
        assert_eq!(stage(&dir, real), Ok(()));
        for name in ["earc_route-4.4.ko", "earc-route-load.sh", INSTALLER_NAME] {
            assert!(dir.join("hbr").join(name).is_file(), "{name} missing");
        }
        let _ = std::fs::remove_dir_all(&dir);

        let dir = scratch("bad");
        let tampered = |path: &str| real(path).map(|mut b| { b[0] ^= 1; b });
        assert_eq!(stage(&dir, tampered), Err(HbrError::Download));
        assert!(!dir.join("hbr").join("earc_route-4.4.ko").exists(), "a file that fails its hash is never written");
        assert_eq!(stage(&dir, |_| None), Err(HbrError::Download));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_command_hands_the_installer_both_hashes() {
        let cmd = install_command(Path::new("/tmp/nj-hbr"));
        assert_eq!(cmd, format!("sh '/tmp/nj-hbr/hbr/nativejelly-hbr.sh' install '/tmp/nj-hbr' {} {}", FILES[0].2, FILES[1].2));
        assert!(INSTALLER.contains("install APPDIR [KO_SHA256 LOADER_SHA256]"), "the compiled-in installer checks the hashes");
    }

    #[test]
    fn the_installer_line_decides() {
        assert_eq!(outcome("hbr: active\n"), Ok(()));
        assert_eq!(outcome("loading…\nhbr: removed\n"), Ok(()));
        assert_eq!(outcome("hbr: REFUSED: kernel 5.4 is not the one this module was built for\n"),
            Err(HbrError::Refused("kernel 5.4 is not the one this module was built for".into())));
        assert_eq!(outcome(""), Err(HbrError::Refused(String::new())));
    }
}
