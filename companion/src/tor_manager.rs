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
///   1. Find a new version at <https://dist.torproject.org/torbrowser/>.
///   2. Fetch the matching `sha256sums-signed-build.txt` from that directory.
///   3. (Recommended) Verify the file's PGP signature against the Tor Project
///      signing key.
///   4. Copy the hashes for the four expert-bundle archives into
///      [`KNOWN_BUNDLES`] below.
///   5. Bump [`BUNDLE_VERSION`] and the companion crate version.
pub const BUNDLE_VERSION: &str = "15.0.15";

/// Minimum Tor binary version this companion will reuse from an
/// externally-running instance (see `tor_detector.rs`). Older versions
/// are rejected.
pub const MIN_REUSABLE_TOR_VERSION: (u32, u32, u32, u32) = (0, 4, 7, 0);

/// One bundle per supported platform. SHA-256 hashes are the official ones
/// published by The Tor Project at
/// <https://dist.torproject.org/torbrowser/15.0.15/sha256sums-signed-build.txt>.
#[derive(Debug, Clone, Copy)]
pub struct Bundle {
    pub platform: &'static str,
    pub url: &'static str,
    /// Hex-encoded lowercase SHA-256.
    pub sha256: &'static str,
    /// Relative path of the Tor executable inside the extracted archive.
    pub binary_subpath: &'static str,
}

const KNOWN_BUNDLES: &[Bundle] = &[
    Bundle {
        platform: "windows-x86_64",
        url: "https://dist.torproject.org/torbrowser/15.0.15/tor-expert-bundle-windows-x86_64-15.0.15.tar.gz",
        sha256: "8d3daf579192f3f128c0f42553dd994c640501b4b98682216d807c88004f7a96",
        binary_subpath: "tor/tor.exe",
    },
    Bundle {
        platform: "linux-x86_64",
        url: "https://dist.torproject.org/torbrowser/15.0.15/tor-expert-bundle-linux-x86_64-15.0.15.tar.gz",
        sha256: "ffc4528394442c3b33a9ccece3536511a3992c78e704756693bed7a2297ef0e7",
        binary_subpath: "tor/tor",
    },
    Bundle {
        platform: "macos-x86_64",
        url: "https://dist.torproject.org/torbrowser/15.0.15/tor-expert-bundle-macos-x86_64-15.0.15.tar.gz",
        sha256: "664ba99389b73bc4264b0ec1dfec247b444e4ce664ea7e19d4b58081bc87cf3c",
        binary_subpath: "tor/tor",
    },
    Bundle {
        platform: "macos-aarch64",
        url: "https://dist.torproject.org/torbrowser/15.0.15/tor-expert-bundle-macos-aarch64-15.0.15.tar.gz",
        sha256: "9afb993d5d505a1cfb62d3119c25cf07674d7e9305a9a87116dcdff36c64e054",
        binary_subpath: "tor/tor",
    },
];

/// Host platform string for diagnostics, e.g. "windows/x86_64".
pub fn host_platform() -> String {
    format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Tor data directory (best-effort), independent of bundle resolution so it
/// can be surfaced in diagnostics even on unsupported platforms.
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("OnionRouter").join("tor").join("data"))
}

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

/// A concrete bundle to install: either the pinned fallback or a newer
/// version resolved (and PGP-verified) at runtime by `tor_update`.
#[derive(Debug, Clone)]
pub struct ResolvedBundle {
    pub version: String,
    pub url: String,
    /// Hex-encoded lowercase SHA-256 of the archive.
    pub sha256: String,
    pub binary_subpath: &'static str,
    /// Platform slug, e.g. "windows-x86_64".
    pub platform: &'static str,
}

impl ResolvedBundle {
    /// Placeholder for an unsupported host. The empty version makes the
    /// install path bail with the usual "unsupported platform" error.
    pub fn unsupported() -> Self {
        Self {
            version: String::new(),
            url: String::new(),
            sha256: String::new(),
            binary_subpath: "",
            platform: "",
        }
    }
}

/// The hard-pinned, known-good bundle for this host. Always available offline
/// and used as the auto-update fallback.
pub fn pinned_bundle() -> Result<ResolvedBundle> {
    let b = bundle_for_host()?;
    Ok(ResolvedBundle {
        version: BUNDLE_VERSION.to_string(),
        url: b.url.to_string(),
        sha256: b.sha256.to_string(),
        binary_subpath: b.binary_subpath,
        platform: b.platform,
    })
}

/// Path to the generated torrc (version-independent).
pub fn torrc_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("OnionRouter").join("tor").join("torrc"))
}

/// Highest Tor bundle version actually extracted on disk, if any. Reflects
/// what auto-update installed, so diagnostics can report the real version in
/// use rather than the pinned baseline.
pub fn installed_version() -> Option<String> {
    let base = dirs::data_local_dir()?.join("OnionRouter").join("tor");
    let mut best: Option<((u32, u32, u32), String)> = None;
    for entry in std::fs::read_dir(&base).ok()?.flatten() {
        if !entry.path().join("extracted").is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(v) = parse_semver(&name) {
            if best.as_ref().map_or(true, |(b, _)| v > *b) {
                best = Some((v, name));
            }
        }
    }
    best.map(|(_, s)| s)
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().transpose().ok()?.unwrap_or(0);
    let c = it.next().transpose().ok()?.unwrap_or(0);
    Some((a, b, c))
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
}

impl TorPaths {
    /// Paths for a specific bundle version. The version keys the install root
    /// so multiple Tor versions can coexist during an upgrade.
    pub fn resolve(version: &str, archive_name: &str, binary_subpath: &str) -> Result<Self> {
        let base = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("could not determine local data dir"))?
            .join("OnionRouter")
            .join("tor");
        let root = base.join(version);
        let extracted = root.join("extracted");
        Ok(Self {
            archive: root.join(archive_name),
            binary: extracted.join(binary_subpath),
            extracted,
            root,
        })
    }
}

/// Ensure the Tor binary is installed and integrity-verified.
/// Returns the absolute path to the executable.
///
/// Resolves the bundle via `tor_update` (latest PGP-verified version when
/// reachable, pinned otherwise). If installing a newer version fails for any
/// reason, falls back to the pinned bundle so the companion never breaks.
pub async fn ensure_binary() -> Result<PathBuf> {
    let resolved = crate::tor_update::resolve_bundle().await;
    match install_from(&resolved).await {
        Ok(path) => Ok(path),
        Err(e) => {
            let pinned = pinned_bundle()?;
            if resolved.version != pinned.version {
                warn!(
                    "installing Tor {} failed ({e:#}); falling back to pinned {}",
                    resolved.version, pinned.version
                );
                install_from(&pinned).await
            } else {
                Err(e)
            }
        }
    }
}

/// Download, verify and extract a specific resolved bundle. Idempotent: if the
/// binary is already present it returns immediately.
async fn install_from(b: &ResolvedBundle) -> Result<PathBuf> {
    if b.version.is_empty() {
        bail!(
            "unsupported platform: {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    let archive_name = b
        .url
        .rsplit('/')
        .next()
        .unwrap_or("tor-expert-bundle.tar.gz");
    let paths = TorPaths::resolve(&b.version, archive_name, b.binary_subpath)?;

    if paths.binary.exists() {
        debug!("Tor binary already present at {}", paths.binary.display());
        return Ok(paths.binary);
    }

    info!("Tor binary missing — installing expert bundle {} ({})", b.version, b.platform);

    fs::create_dir_all(&paths.root)
        .await
        .with_context(|| format!("creating {}", paths.root.display()))?;

    if !paths.archive.exists() {
        download(&b.url, &paths.archive).await?;
    }

    verify_sha256(&paths.archive, &b.sha256).await?;
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
    let data_dir = data_dir().ok_or_else(|| anyhow!("could not determine local data dir"))?;
    let torrc_file = torrc_path().ok_or_else(|| anyhow!("could not determine local data dir"))?;
    fs::create_dir_all(&data_dir).await?;

    let torrc = format!(
        "SocksPort {socks_port}\n\
         ControlPort {control_port}\n\
         CookieAuthentication 0\n\
         DataDirectory {data_dir}\n\
         AvoidDiskWrites 1\n\
         Log notice stdout\n",
        data_dir = data_dir.display(),
    );
    fs::write(&torrc_file, torrc).await?;

    info!(
        "spawning Tor: SocksPort={socks_port} ControlPort={control_port}"
    );

    let mut cmd = Command::new(binary);
    cmd.arg("-f")
        .arg(&torrc_file)
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
