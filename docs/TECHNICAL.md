# Documentation technique - OnionRouter

## Vue d'ensemble

OnionRouter permet a une extension Firefox de router le trafic `.onion` via
Tor sans demander a l'utilisateur d'installer Tor Browser.

Le projet est compose de trois blocs:

- `extension/`: extension Firefox Manifest V3.
- `companion/`: companion Rust expose en Native Messaging.
- `installer/`: packaging Windows, Linux et scripts de developpement.

L'extension ne lance pas Tor directement. Elle demande au companion Rust de
demarrer, detecter ou reutiliser un backend Tor, puis configure le proxy
Firefox requete par requete via `browser.proxy.onRequest`.

## Flux principal

1. Firefox charge l'extension.
2. L'utilisateur visite une URL ou active un mode qui necessite Tor.
3. `background.js` ouvre une connexion Native Messaging vers
   `com.onionrouter.companion`.
4. Le companion cherche un Tor utilisable:
   - runtime publie par le tray Windows;
   - instance externe sur `9050/9051` ou `9150/9151`;
   - lancement d'un Tor embarque telecharge et verifie.
5. Le companion repond avec le port SOCKS.
6. L'extension route le trafic via `127.0.0.1:<port>` avec `proxyDNS: true`.

## Extension Firefox

Fichiers principaux:

- `extension/manifest.json`: permissions, ID Gecko, popup, background.
- `extension/background.js`: routage, Native Messaging, WebRTC, stockage.
- `extension/popup.html`, `popup.css`, `popup.js`: interface utilisateur.
- `extension/diagnostics.html`, `diagnostics.css`, `diagnostics.js`: page de
  diagnostic, ouverte depuis le popup dans un onglet.

Permissions utilisees:

- `proxy`: routage par requete.
- `nativeMessaging`: communication avec le companion.
- `storage`: persistance du mode, whitelist et preference WebRTC.
- `privacy`: activation/desactivation de WebRTC.
- `tabs`: ajout du site courant a la whitelist + ouverture de la page diagnostic.
- `notifications`: notification d'accueil au premier lancement (F10).
- `webRequest` et `<all_urls>`: surface de routage Firefox.

Modes de routage:

- `onion`: seules les URLs `.onion` passent par Tor.
- `all`: toutes les requetes passent par Tor.
- `whitelist`: `.onion` plus les domaines autorises passent par Tor.

Les domaines `.onion` passent toujours par Tor, quel que soit le mode. En cas
d'erreur de demarrage Tor pour une URL qui doit passer par Tor, l'extension
renvoie un proxy local impossible (`127.0.0.1:1`) afin d'eviter une fuite DNS
ou un fallback direct.

## Companion Rust

Fichiers principaux:

- `main.rs`: entree, mode Native Messaging et dispatch Windows tray.
- `messaging.rs`: framing Native Messaging Mozilla.
- `tor_manager.rs`: telechargement, verification, extraction, lancement Tor.
- `tor_update.rs`: maj auto -- decouverte derniere version + verif PGP des sommes.
- `tor_detector.rs`: verification d'une instance Tor externe.
- `proxy.rs`: allocation de ports SOCKS/Control.
- `runtime.rs`: fichier d'etat partage entre le tray et Native Messaging.
- `tray.rs`: daemon tray Windows.

Le binaire a deux modes:

- sans argument: Native Messaging, lance par Firefox.
- `--tray`: daemon Windows long-lived, lance au login par l'installeur.

## Protocole Native Messaging

Chaque message suit le framing Mozilla:

```text
[4 octets little-endian: taille JSON][payload JSON UTF-8]
```

Messages extension vers companion:

```json
{ "action": "start" }
{ "action": "stop" }
{ "action": "status" }
{ "action": "ping" }
{ "action": "diagnostic" }
```

Messages companion vers extension:

```json
{ "status": "ready", "port": 9050 }
{ "status": "stopped" }
{ "status": "error", "message": "..." }
{ "status": "pong" }
{ "status": "diagnostic", "running": true, "source": "owned",
  "socks_port": 9050, "control_port": 9051, "tor_version": null,
  "bundle_version": "15.0.15", "companion_version": "0.3.0",
  "platform": "windows/x86_64", "data_dir": "..." }
```

`diagnostic` renvoie un instantane best-effort. `source` vaut `owned`
(Tor lance par le companion), `tray` (daemon tray) ou `external` (Tor
reutilise). `tor_version` n'est connu que pour un Tor externe reutilise.
Les champs statiques (`bundle_version`, `companion_version`, `platform`,
`data_dir`) sont toujours renvoyes, meme si Tor n'est pas demarre.

`starting` existe dans le type Rust, mais le flux actuel ne l'emet pas encore
depuis le companion. L'extension gere deja cet etat cote UI.

## Page de diagnostic

Ouverte via le lien "Diagnostics…" du popup, dans un onglet
(`browser.tabs.create` + `runtime.getURL("diagnostics.html")`). Elle agrege
trois sources:

- l'etat de l'extension (`get-state`): statut, mode, WebRTC, whitelist;
- un instantane du companion (`get-diagnostic`): version companion, plateforme,
  source Tor, ports SOCKS/Control, version Tor, version bundle, dossier data;
- un test de connectivite (`ping-companion`): aller-retour `ping`/`pong` chronometre.

Cote `background.js`:

- `ensureCompanionPort()` ouvre le port Native Messaging **sans** demarrer Tor,
  pour que la page puisse interroger le companion meme a l'arret.
- `requestDiagnostic()` / `pingCompanion()` envoient la requete et resolvent une
  promesse one-shot quand la reponse arrive (avec timeout et rejet sur
  deconnexion du companion).

Le bouton "Copy report" copie un resume texte (clipboard API, repli sur
`execCommand`) pour les rapports de bug. La page n'introduit aucune nouvelle
permission: l'ouverture d'onglet utilise `tabs`, deja requise.

## Gestion de Tor

La version du Tor Expert Bundle est pinnee dans `tor_manager.rs`:

- `BUNDLE_VERSION`
- URL officielle Tor Project par plateforme.
- SHA-256 officiel par archive.
- chemin relatif du binaire Tor dans l'archive.

Plateformes actuellement connues:

- Windows x86_64.
- Linux x86_64.
- macOS x86_64.
- macOS aarch64.

La cible Linux `.deb` est limitee a `amd64`, car le bundle Linux connu est
`linux-x86_64`.

Stockage Tor:

- Windows: `%LOCALAPPDATA%\OnionRouter\tor\`
- Linux: `~/.local/share/OnionRouter/tor/` ou equivalent `dirs`
- macOS: repertoire local data retourne par `dirs`

Le companion refuse d'executer une archive si le SHA-256 ne correspond pas.

### Mise a jour automatique (F11)

`tor_update.rs` evite que la version pinnee se perime (les vieilles versions
sont purgees du miroir Tor, d'ou des 404):

1. decouvre la derniere version stable (parse du listing `dist.torproject.org`);
2. si > version pinnee: telecharge `sha256sums-signed-build.txt` + `.asc`;
3. **verifie la signature PGP** contre la cle de build Tor embarquee
   (`assets/tor-signing-key.asc`, sous-cle `CAAE408A…78A65729`), via le crate
   `pgp` (rPGP, pur Rust) -- essai cle primaire puis sous-cles;
4. extrait le hash de la plateforme et installe le bundle (verif SHA-256).

Toute erreur (hors ligne, parse, signature invalide, download) bascule sur la
version pinnee: la maj auto ne peut pas casser le companion. Les versions
coexistent sous `tor/<version>/` le temps d'un upgrade.

## Detection d'un Tor existant

Avant de lancer son propre Tor, le companion sonde:

- `9050/9051`: Tor systeme.
- `9150/9151`: Tor Browser.

La detection ne se limite pas a un port ouvert. Elle verifie le Control Port:

1. `PROTOCOLINFO 1`.
2. Authentification `NULL` ou `COOKIE`.
3. `GETINFO version`.
4. Version minimale `0.4.7.0`.

`SAFECOOKIE` est supporte (challenge/reponse HMAC-SHA256, control-spec §3.24),
ce qui permet de reutiliser le Tor d'un Tor Browser ouvert. `HASHEDPASSWORD`
n'est pas supporte: le companion lance alors son propre Tor.

## Fichier runtime du tray

Le tray Windows peut lancer Tor sur un port libre non standard. Pour que les
instances Native Messaging retrouvent ce Tor, `runtime.rs` publie:

```json
{
  "socks_port": 12345,
  "control_port": 12346,
  "tray_pid": 9999,
  "bundle_version": "15.0.15"
}
```

Emplacement: repertoire local data `OnionRouter/runtime/state.json`.

## Packaging

### Windows

`installer/build.ps1` construit:

- le companion Rust;
- le XPI depuis `extension/`;
- l'installeur NSIS.

Le workflow GitHub `release.yml` installe NSIS sur `windows-latest`, construit
l'installeur, calcule les SHA-256 et publie les assets sur une release GitHub.

### Debian/Ubuntu

`installer/linux/build-deb.sh` construit:

- `dist/onionrouter-companion_<version>_amd64.deb`

Le paquet installe:

- `/usr/lib/onionrouter/onionrouter-companion`
- `/usr/lib/mozilla/native-messaging-hosts/com.onionrouter.companion.json`
- documentation sous `/usr/share/doc/onionrouter-companion/`

Il n'installe pas l'extension Firefox. L'extension doit rester distribuee via
AMO ou via une XPI signee.

## Release

La version est synchronisee manuellement dans:

- `extension/manifest.json`
- `companion/Cargo.toml`
- `companion/Cargo.lock`

Pour publier:

```bash
git tag v0.3.0
git push origin v0.3.0
```

Le workflow refuse un tag qui ne correspond pas a la version du manifest, sauf
suffixe de prerelease (`v0.3.0-rc1`).

## Signature des releases

Les artefacts de release peuvent etre signes avec une cle GPG. La signature
prouve la provenance et l'integrite des binaires; le sceau AMO ne couvre que la
XPI, pas le companion ni l'installeur.

La signature est optionnelle dans la CI: si les secrets ne sont pas configures,
les etapes de signature sont ignorees et le build reste vert.

Secrets GitHub attendus:

- `GPG_SIGNING_KEY`: cle privee GPG au format ASCII-armored.
- `GPG_SIGNING_PASSPHRASE`: passphrase de la cle (vide si la cle n'en a pas).

Artefacts signes:

- Windows: `SHA256SUMS.txt.asc` (signature detachee des sommes XPI + EXE).
- Debian: `onionrouter-companion_<version>_amd64.deb.asc` (signature detachee
  du paquet).

Generer la cle de signature (une seule fois):

```bash
gpg --full-generate-key            # cle RSA 4096, sans expiration ou a renouveler
gpg --list-secret-keys --keyid-format=long
# Exporter la cle privee pour le secret GitHub:
gpg --armor --export-secret-keys <KEYID> > onionrouter-signing-private.asc
# Exporter la cle publique a publier dans le depot:
gpg --armor --export <KEYID> > docs/onionrouter-signing-key.asc
```

Ajouter ensuite `onionrouter-signing-private.asc` comme secret
`GPG_SIGNING_KEY` (et la passphrase comme `GPG_SIGNING_PASSPHRASE`), puis
committer uniquement la cle publique `docs/onionrouter-signing-key.asc`.

Verifier une release (cote utilisateur):

```bash
# Importer la cle publique du projet une fois:
gpg --import docs/onionrouter-signing-key.asc

# Windows:
gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt        # ou Get-FileHash sous Windows

# Debian:
gpg --verify onionrouter-companion_0.3.0_amd64.deb.asc \
            onionrouter-companion_0.3.0_amd64.deb
```

## Validation locale

Rust:

```powershell
cargo test --manifest-path companion\Cargo.toml
```

Extension:

```powershell
python -m json.tool extension\manifest.json
node --check extension\background.js
node --check extension\popup.js
```

Debian package, sur Linux:

```bash
python3 --version
bash installer/linux/build-deb.sh
dpkg-deb --info dist/onionrouter-companion_0.3.0_amd64.deb
dpkg-deb --contents dist/onionrouter-companion_0.3.0_amd64.deb
```

## Limites connues

- Le `.deb` ne couvre que `amd64`.
- Le tray est Windows uniquement.
- La page diagnostic n'est pas encore implementee.
- La mise a jour automatique du bundle Tor n'est pas encore implementee.
- Les methodes Control Port `SAFECOOKIE` et `HASHEDPASSWORD` ne sont pas encore
  prises en charge pour la reutilisation d'un Tor externe.
- Le packaging macOS n'est pas encore implemente.
