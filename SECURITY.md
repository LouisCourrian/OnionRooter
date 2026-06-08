# Security Policy

## Reporting a vulnerability

Please report security issues **privately**, not in public issues:

- Preferred: GitHub → **Security** tab → **Report a vulnerability** (private
  advisory).
- Include steps to reproduce, affected component (extension or companion), and
  version.

You'll get an acknowledgement as soon as possible. Please give a reasonable
window to fix before any public disclosure.

## Scope

In scope: the Firefox extension, the Rust companion, the release/signing
pipeline. Out of scope: Tor itself (report upstream), and the explicitly
excluded items in [`CAHIER_DES_CHARGES.md`](CAHIER_DES_CHARGES.md) §4 (OnionRouter
does not claim Tor Browser's isolation guarantees).

## Security model (summary)

- **Tor binaries** are refused unless their **SHA-256 matches** a known value.
  Auto-update additionally **verifies the Tor Project's PGP signature** on the
  checksums (key pinned in the companion).
- **Client-auth private keys** live only in the companion, never in the Firefox
  profile — encrypted with the **OS keystore** (Windows DPAPI) or a
  **passphrase** (Argon2id + XChaCha20-Poly1305). The on-disk index stores only
  a **SHA-256** of each protected `.onion` address.
- The companion's **update check is routed through Tor** so it cannot leak your
  IP.
- **Release artifacts are GPG-signed**; the public key is
  [`docs/onionrouter-signing-key.asc`](docs/onionrouter-signing-key.asc). The
  extension is signed by Mozilla (AMO).

## Known limitations

- A single hard-pinned Tor version eventually 404s when the Tor Project prunes
  old releases; auto-update mitigates this, with the pin as a fallback.
- The Windows installer and Debian package are **not** code-signed
  (Authenticode / dpkg-sig) yet — verify them with the published GPG signature.
- Same-user malware can ask the OS keystore to decrypt OS-tier keys (inherent to
  any auto-unlock scheme); the passphrase tier mitigates the at-rest case.
