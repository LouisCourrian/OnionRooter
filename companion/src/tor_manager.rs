//! Download, verify, extract, and launch the official Tor Expert Bundle.
//!
//! Inspired by the `tornion` Python library: pinned SHA-256 hashes, official
//! Tor Project URLs, no third-party mirrors.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};

/// Pinned Tor Expert Bundle version that this companion knows how to verify.
///
/// **Update protocol** when bumping:
///   1. Download new bundles from <https://www.torproject.org/download/tor/>.
///   2. Verify Tor Project's GPG signature on the `.sha256sum` file.
///   3. Copy the matching SHA-256 hashes into [`KNOWN_BUNDLES`] below.
///   4. Bump [`TOR_VERSION`] and bump the companion's own crate version.
pub const TOR_VERSION: &str = "0.4.8.13";

/// Minimum Tor version this companion will reuse from an externally-running
/// instance (see `tor_detector.rs`). Older versions are rejected.
pub const MIN_REUSABLE_TOR_VERSION: (u32, u32, u32, u32) = (0, 4, 7, 0);

/// One bundle per supported platform. `sha256` MUST be the official hash
/// published by The Tor Project for [`TOR_VERSION`] — placeholders below are
/// flagged and refused at runtime until updated.
#[derive(Debug, Clone, Copy)]
pub struct Bundle {
    pub platform: &'static str,
    pub url: &'static str,
    /// Hex-encoded lowercase SHA-256. Empty string = unset placeholder.
    pub sha256: &'static str,
    /// Relative path of the Tor executable inside the extracted archive.
    pub binary_subpath: &'static str,
}

const KNOWN_BUNDLES: &[Bundle] = &[
    Bundle {
        platform: "windows-x86_64",
        url: "https://archive.torproject.org/tor-package-archive/torbrowser/13.5.7/tor-expert-bundle-windows-x86_64-13.5.7.tar.gz",
        sha256: "", // TODO: paste official SHA-256 from torproject.org/dist/torbrowser/13.5.7/sha256sums-signed-build.txt
        binary_subpath: "tor/tor.exe",
    },
    Bundle {
        platform: "linux-x86_64",
        url: "https://archive.torproject.org/tor-package-archive/torbrowser/13.5.7/tor-expert-bundle-linux-x86_64-13.5.7.tar.gz",
        sha256: "", // TODO: paste official SHA-256
        binary_subpath: "tor/tor",
    },
    Bundle {
        platform: "macos-x86_64",
        url: "https://archive.torproject.org/tor-package-archive/torbrowser/13.5.7/tor-expert-bundle-macos-x86_64-13.5.7.tar.gz",
        sha256: "", // TODO: paste official SHA-256
        binary_subpath: "tor/tor",
    },
    Bundle {
        platform: "macos-aarch64",
        url: "https://archive.torproject.org/tor-package-archive/torbrowser/13.5.7/tor-expert-bundle-macos-aarch64-13.5.7.tar.gz",
        sha256: "", // TODO: paste official SHA-256
        binary_subpath: "tor/tor",
    },
];

/// Resolve the bundle matching the host platform.
fn bundle_for_host() -> Result<&'static Bundle> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("macos", "x86_64") => "macos-x86_64",
        ("macos", "aarch64") => "macos-aarch64",
        (os, arch) => bail!("unsupported platform: {os}/{arch}"),
    };
    KNOWN_BUNDLES
        .iter()
        .find(|b| b.platform == platform)
        .ok_or_else(|| anyhow!("no bundle entry for platform {platform}"))
}

/// Paths used by the companion on disk.
pub struct TorPaths {
    /// Root: `%APPDATA%\OnionRouter\tor\` on Windows, `~/.local/share/onionrouter/tor/` on Linux.
    pub root: PathBuf,
    /// Cached archive download.
    pub archive: PathBuf,
    /// Extracted tree.
    pub extracted: PathBuf,
    /// Full path to the Tor binary.
    pub binary: PathBuf,
    /// Tor's data directory.
    pub data_dir: PathBuf,
    /// Generated torrc.
    pub torrc: PathBuf,
}

impl TorPaths {
    pub fn resolve(bundle: &Bundle) -> Result<Self> {
        let base = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("could not determine local data dir"))?
            .join("OnionRouter")
            .join("tor");
        let root = base.join(TOR_VERSION);
        let extracted = root.join("extracted");
        let archive_name = bundle
            .url
            .rsplit('/')
            .next()
            .unwrap_or("tor-expert-bundle.tar.gz");
        Ok(Self {
            archive: root.join(archive_name),
            binary: extracted.join(bundle.binary_subpath),
            data_dir: base.join("data"),
            torrc: base.join("torrc"),
            extracted,
            root,
        })
    }
}

/// Ensure the Tor binary is installed and integrity-verified.
/// Returns the absolute path to the executable.
pub async fn ensure_binary() -> Result<PathBuf> {
    let bundle = bundle_for_host()?;
    let paths = TorPaths::resolve(bundle)?;

    if paths.binary.exists() {
        debug!("Tor binary already present at {}", paths.binary.display());
        return Ok(paths.binary);
    }

    info!(
        "Tor binary missing — downloading expert bundle for {}",
        bundle.platform
    );

    fs::create_dir_all(&paths.root)
        .await
        .with_context(|| format!("creating {}", paths.root.display()))?;

    if !paths.archive.exists() {
        download(bundle.url, &paths.archive).await?;
    }

    verify_sha256(&paths.archive, bundle.sha256).await?;
    extract_tar_gz(&paths.archive, &paths.extracted).await?;

    if !paths.binary.exists() {
        bail!(
            "extraction completed but binary not found at {}",
            paths.binary.display()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&paths.binary)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&paths.binary, perms)?;
    }

    info!("Tor binary ready at {}", paths.binary.display());
    Ok(paths.binary)
}

async fn download(url: &str, dest: &Path) -> Result<()> {
    info!("downloading {url}");
    let tmp = dest.with_extension("part");
    let resp = reqwest::get(url)
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?;
    let bytes = resp.bytes().await.context("reading response body")?;
    fs::write(&tmp, &bytes)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, dest)
        .await
        .with_context(|| format!("renaming to {}", dest.display()))?;
    Ok(())
}

async fn verify_sha256(path: &Path, expected_hex: &str) -> Result<()> {
    if expected_hex.is_empty() {
        bail!(
            "no pinned SHA-256 for this platform — refusing to execute unverified binary. \
             Update KNOWN_BUNDLES in tor_manager.rs with the official hash before shipping."
        );
    }

    let bytes = fs::read(path).await?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hex::encode(hasher.finalize());

    if !actual.eq_ignore_ascii_case(expected_hex) {
        // The downloaded archive is suspect — wipe it so a retry redownloads.
        let _ = fs::remove_file(path).await;
        bail!("SHA-256 mismatch: expected {expected_hex}, got {actual}");
    }
    debug!("SHA-256 verified ({})", &actual[..16]);
    Ok(())
}

async fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let archive = archive.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        if dest.exists() {
            std::fs::remove_dir_all(&dest).ok();
        }
        std::fs::create_dir_all(&dest)?;
        let file = std::fs::File::open(&archive)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        tar.unpack(&dest)?;
        Ok(())
    })
    .await
    .context("extraction task panicked")??;
    Ok(())
}

/// Configuration captured at launch time.
pub struct LaunchedTor {
    pub child: Child,
    pub socks_port: u16,
    pub control_port: u16,
}

/// Generate a torrc and spawn Tor. Waits up to `bootstrap_timeout` for the
/// "Bootstrapped 100%" log line before returning.
pub async fn launch(
    binary: &Path,
    socks_port: u16,
    control_port: u16,
    bootstrap_timeout: Duration,
) -> Result<LaunchedTor> {
    let bundle = bundle_for_host()?;
    let paths = TorPaths::resolve(bundle)?;
    fs::create_dir_all(&paths.data_dir).await?;

    let torrc = format!(
        "SocksPort {socks_port}\n\
         ControlPort {control_port}\n\
         CookieAuthentication 0\n\
         DataDirectory {data_dir}\n\
         AvoidDiskWrites 1\n\
         Log notice stdout\n",
        data_dir = paths.data_dir.display(),
    );
    fs::write(&paths.torrc, torrc).await?;

    info!(
        "spawning Tor: SocksPort={socks_port} ControlPort={control_port}"
    );

    let mut cmd = Command::new(binary);
    cmd.arg("-f")
        .arg(&paths.torrc)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW — keep the console invisible on Windows.
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Tor stdout not captured"))?;

    let bootstrap = wait_for_bootstrap(stdout);
    match timeout(bootstrap_timeout, bootstrap).await {
        Ok(Ok(())) => {
            info!("Tor bootstrapped successfully");
            Ok(LaunchedTor {
                child,
                socks_port,
                control_port,
            })
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            Err(e)
        }
        Err(_) => {
            let _ = child.kill().await;
            bail!("Tor failed to bootstrap within {:?}", bootstrap_timeout)
        }
    }
}

async fn wait_for_bootstrap<R: tokio::io::AsyncRead + Unpin>(stdout: R) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        debug!("[tor] {line}");
        if line.contains("Bootstrapped 100%") {
            return Ok(());
        }
        if line.contains("[err]") {
            warn!("tor reported an error: {line}");
        }
    }
    bail!("Tor stdout closed before bootstrap completed");
}

/// Best-effort graceful shutdown.
pub async fn shutdown(launched: &mut LaunchedTor) {
    if let Err(e) = launched.child.kill().await {
        error!("failed to kill Tor process: {e}");
    }
    let _ = launched.child.wait().await;
}
