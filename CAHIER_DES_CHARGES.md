# Cahier des charges - OnionRouter

Extension Firefox + companion Rust pour utiliser Tor depuis Firefox sans
installer Tor Browser.

Version de travail: `0.3.0`.

## 1. Objectif

OnionRouter permet a un utilisateur Firefox de visiter des sites `.onion` en
routeant automatiquement le trafic concerne via Tor.

Le projet ne cherche pas a remplacer Tor Browser. Il fournit un chemin simple
pour les usages ou l'utilisateur veut rester dans Firefox tout en evitant la
configuration manuelle de Tor.

## 2. Composants

```text
Firefox extension
  - detection des URLs et domaines a router
  - configuration proxy Firefox
  - popup utilisateur
  - protection WebRTC

Rust companion
  - Native Messaging
  - telechargement et verification du Tor Expert Bundle
  - lancement de Tor
  - reutilisation d'un Tor existant si verifie

Installers / packages
  - Windows NSIS installer
  - Debian/Ubuntu .deb companion package
  - scripts de developpement Windows/Linux
```

## 3. Fonctionnalites principales

| ID | Fonctionnalite | Statut | Notes |
| --- | --- | --- | --- |
| F1 | Detection automatique des `.onion` | Fait | Geree dans `background.js`. |
| F2 | Routage SOCKS5 via Tor | Fait | `browser.proxy.onRequest` + `proxyDNS: true`. |
| F3 | Telechargement automatique de Tor | Fait | Tor Expert Bundle officiel. |
| F4 | Verification SHA-256 | Fait | Hashes pinnees dans `tor_manager.rs`. |
| F5 | Lancement automatique de Tor | Fait | Demarrage a la demande via companion. |
| F6 | Reutilisation d'un Tor existant | Fait | Verification Control Port + version minimale. |
| F7 | Icone d'etat Firefox | Fait | Inactif, demarrage, actif, erreur. |
| F8 | Arret propre de Tor | Fait partiel | Native Messaging arrete son Tor; tray Windows le garde volontairement vivant. |
| F9 | Bouton marche/arret manuel | Fait | Popup start/stop. |
| F10 | Notification premier lancement | Non fait | Pas prioritaire pour `0.2.2`. |
| F11 | Mise a jour automatique de Tor | Fait | Derniere version, sommes verifiees PGP, fallback pinne. |
| F12 | Page de diagnostic | Fait | Page extension + action `diagnostic` du companion. |
| F13 | Mode "Tout via Tor" | Fait | Tout le trafic Firefox passe par Tor. |
| F14 | Mode "Whitelist" | Fait | Domaines choisis + `.onion`. |
| F15 | Gestion whitelist | Fait | Ajout/suppression dans le popup. |
| F16 | Ajouter le site courant | Fait | Via `tabs.query`. |
| F17 | WebRTC coupe en mode all | Fait | Via `browser.privacy.network`. |
| F18 | Option WebRTC hors mode all | Fait | Preference utilisateur persistante. |

## 4. Perimetre explicitement exclu

- Pas de VPN.
- Pas de chiffrement du trafic normal hors routage Tor.
- Pas de gestion multi-circuits.
- Pas d'interface pour publier un service `.onion`.
- Pas de gestion specifique cookies/historique.
- Pas de promesse d'equivalence avec le niveau d'isolation de Tor Browser.

## 5. Specification technique

### 5.1 Extension Firefox

Choix techniques:

- Manifest V3.
- JavaScript sans bundler.
- `browser.proxy.onRequest` pour le routage.
- `browser.runtime.connectNative()` pour parler au companion.
- `browser.storage.local` pour mode, whitelist et WebRTC.

Modes:

```text
onion:
  .onion -> Tor
  reste  -> direct

all:
  tout -> Tor

whitelist:
  .onion                 -> Tor
  domaine dans whitelist -> Tor
  reste                  -> direct
```

Regle de securite: les `.onion` ne sortent jamais en direct. En cas d'erreur
Tor, l'extension renvoie un proxy local impossible pour casser la requete.

### 5.2 Companion Rust

Choix techniques:

- Rust edition 2021.
- Tokio pour l'asynchrone.
- Serde/JSON pour Native Messaging.
- Reqwest + rustls pour le telechargement.
- SHA-256 pinne pour verifier les archives Tor.

Le companion accepte deux modes:

- Native Messaging: lance par Firefox, communique en stdin/stdout.
- Windows tray: lance au login, maintient Tor vivant et publie le port.

### 5.3 Protocole Native Messaging

Messages Firefox vers companion:

```json
{ "action": "start" }
{ "action": "stop" }
{ "action": "status" }
{ "action": "ping" }
{ "action": "diagnostic" }
```

Messages companion vers Firefox:

```json
{ "status": "ready", "port": 9050 }
{ "status": "stopped" }
{ "status": "error", "message": "..." }
{ "status": "pong" }
{
  "status": "diagnostic",
  "running": true,
  "source": "owned",
  "socks_port": 9050,
  "control_port": 9051,
  "tor_version": null,
  "bundle_version": "15.0.15",
  "companion_version": "0.3.0",
  "platform": "windows/x86_64",
  "data_dir": "..."
}
```

Chaque message est prefixe par 4 octets little-endian indiquant la taille du
payload JSON, selon le standard Mozilla Native Messaging.

### 5.4 Tor Expert Bundle

La version Tor connue est pinnee dans `companion/src/tor_manager.rs`.

Le companion connait les bundles:

- Windows x86_64.
- Linux x86_64.
- macOS x86_64.
- macOS aarch64.

La verification SHA-256 est obligatoire. Une archive inconnue ou modifiee est
refusee.

Mise a jour automatique (F11): au demarrage, le companion decouvre la derniere
version stable sur `dist.torproject.org`, telecharge `sha256sums-signed-build.txt`
et sa signature `.asc`, **verifie la signature PGP** contre la cle de build Tor
embarquee dans le binaire (`companion/assets/tor-signing-key.asc`), puis utilise
le hash de la plateforme. Toute erreur (hors ligne, signature invalide) bascule
sur la version pinnee connue-bonne: la maj auto ne peut jamais casser le
companion. Voir `companion/src/tor_update.rs`.

### 5.5 Detection de Tor existant

Ports sondes:

| SOCKS | Control | Usage probable |
| --- | --- | --- |
| 9050 | 9051 | Tor systeme |
| 9150 | 9151 | Tor Browser |

Verification:

1. Connexion TCP au Control Port.
2. `PROTOCOLINFO 1`.
3. Authentification `NULL` ou `COOKIE`.
4. `GETINFO version`.
5. Version minimale `0.4.7.0`.

`SAFECOOKIE` et `HASHEDPASSWORD` ne sont pas encore supportes pour la
reutilisation. Le companion lance alors son propre Tor.

## 6. Distribution

### 6.1 Windows

Statut: fait.

L'installeur NSIS:

- installe le companion sous `%LOCALAPPDATA%\OnionRouter`;
- genere le manifest Native Messaging;
- inscrit la cle registre Firefox sous HKCU;
- ajoute une entree de desinstallation;
- lance et enregistre le tray au login;
- place une XPI a cote de l'installation.

Le build Windows est fait par GitHub Actions.

### 6.2 Extension Firefox signee

Statut: disponible hors depot pour le moment.

La distribution finale doit utiliser une XPI signee par AMO, listee ou non
listee. L'automatisation `web-ext sign` reste possible quand les secrets AMO
seront disponibles.

### 6.3 Debian/Ubuntu

Statut: fait pour `0.2.2`.

Le script `installer/linux/build-deb.sh` produit:

```text
dist/onionrouter-companion_<version>_amd64.deb
```

Le paquet installe:

- `/usr/lib/onionrouter/onionrouter-companion`
- `/usr/lib/mozilla/native-messaging-hosts/com.onionrouter.companion.json`
- `/usr/share/doc/onionrouter-companion/`

Le paquet Debian n'installe pas l'extension Firefox. Elle doit etre installee
separement via AMO ou une XPI signee.

### 6.4 Signature des artefacts

Statut: infrastructure prete, cle a fournir.

Les artefacts de release (installeur Windows via `SHA256SUMS.txt`, paquet
Debian) sont signes avec une cle GPG du projet quand les secrets
`GPG_SIGNING_KEY` et `GPG_SIGNING_PASSPHRASE` sont configures. En leur absence,
la CI ignore la signature sans echouer.

La signature couvre la provenance du companion et de l'installeur, que le sceau
AMO de la XPI ne couvre pas. Voir `docs/TECHNICAL.md` pour la generation de la
cle et la verification.

### 6.5 macOS

Statut: non fait.

Le companion contient deja une entree Tor Expert Bundle macOS, mais le
packaging `.pkg` et l'enregistrement systeme ne sont pas encore implementes.

## 7. Etat des phases

### Phase 1 - MVP

- [x] Companion Rust Native Messaging.
- [x] Telechargement et verification SHA-256 du binaire Tor.
- [x] Lancement Tor et attente bootstrap.
- [x] Detection `.onion`.
- [x] Routage SOCKS5.
- [x] Icone d'etat.

### Phase 2 - Robustesse

- [x] Detection Tor existant via Control Port.
- [x] Verification que l'instance est bien Tor.
- [x] Verification version minimale.
- [x] Fallback lancement Tor interne.
- [x] Messages d'erreur lisibles.
- [x] Arret propre du Tor possede par la session Native Messaging.
- [x] Support Windows.
- [x] Support Linux companion.

### Phase 3 - Modes avances

- [x] Mode "Tout via Tor".
- [x] Mode "Whitelist".
- [x] Interface ajout/suppression whitelist.
- [x] Bouton "Ajouter ce site".
- [x] Persistance mode, whitelist et WebRTC.
- [x] Icones selon le mode.
- [x] Gestion WebRTC.

### Phase 4 - Distribution

- [x] Installeur Windows.
- [x] Workflow release Windows.
- [x] Paquet Debian/Ubuntu companion.
- [x] Workflow release `.deb`.
- [x] Documentation technique.
- [x] Page de diagnostic.
- [ ] Publication AMO integree au depot.
- [ ] Packaging macOS.

## 8. Criteres de succes

- [x] Une adresse `.onion` s'ouvre dans Firefox sans configuration proxy
      manuelle.
- [x] Le trafic normal reste direct en mode `onion`.
- [x] Le mode `all` route tout via Tor.
- [x] La whitelist route uniquement les domaines choisis.
- [x] Les DNS des requetes routees passent par Tor.
- [x] Le companion refuse les archives Tor non verifiees.
- [x] Les tests unitaires Rust passent.
- [x] Le manifest Firefox est valide.
- [x] Les scripts JS passent le check syntaxique.
- [ ] La release publique finale pointe vers une XPI signee disponible.

## 9. Prochaines priorites

1. Verifier le `.deb` sur une Debian/Ubuntu propre.
2. Ajouter l'automatisation AMO si les secrets sont disponibles.
3. Etudier le packaging macOS.
4. Ajouter `SAFECOOKIE` pour reutiliser davantage d'instances Tor externes.
