# Cahier des charges — OnionRouter

**Extension Firefox + Compagnon Rust avec gestion automatique de Tor**

---

## 1. Présentation du projet

### Contexte

L'objectif est de créer un outil permettant à n'importe quel utilisateur de Firefox de visiter des sites en `.onion` (réseau Tor) sans avoir à installer manuellement le Navigateur Tor ni configurer quoi que ce soit.

### Inspiration

Le projet s'appuie sur les principes de **tornion** (bibliothèque Python) : télécharger automatiquement le binaire officiel de Tor, vérifier son intégrité, et le gérer de façon transparente — mais cette fois au service d'un navigateur grand public.

### Résumé en une phrase

> Une extension Firefox qui détecte les adresses `.onion` et les route automatiquement via Tor, grâce à un petit programme compagnon écrit en Rust qui gère tout en arrière-plan.

---

## 2. Composants du projet

Le projet se décompose en **trois parties** distinctes :

```
┌─────────────────────────┐      Native Messaging      ┌──────────────────────────┐
│   Extension Firefox     │ ◄─────────────────────── ► │  Compagnon Rust (.exe)   │
│  (détection .onion,     │                             │  (gestion Tor, proxy,    │
│   gestion du proxy)     │                             │   téléchargement auto)   │
└─────────────────────────┘                             └──────────────────────────┘
                                                                    │
                                                                    ▼
                                                         ┌──────────────────────┐
                                                         │   Binaire Tor        │
                                                         │  (téléchargé auto    │
                                                         │   depuis torproject) │
                                                         └──────────────────────┘
```

### 2.1 Le compagnon Rust

Programme léger (~3 Mo) qui tourne en tâche de fond sur l'ordinateur de l'utilisateur.

### 2.2 L'extension Firefox

S'installe comme n'importe quelle extension depuis addons.mozilla.org.

### 2.3 L'installeur

Un seul fichier à télécharger qui installe et configure les deux composants ci-dessus.

---

## 3. Fonctionnalités

### 3.1 Fonctionnalités principales (indispensables)

| #   | Fonctionnalité                     | Description                                                                             |
| --- | ---------------------------------- | --------------------------------------------------------------------------------------- |
| F1  | Détection automatique des `.onion` | L'extension repère toute URL se terminant par `.onion`                                  |
| F2  | Routage automatique via Tor        | Le trafic est routé via le proxy Tor local (SOCKS5) selon le mode actif                 |
| F3  | Téléchargement automatique de Tor  | Le compagnon télécharge le binaire officiel "Tor Expert Bundle" si absent               |
| F4  | Vérification d'intégrité           | Le hash SHA-256 du binaire est vérifié avant toute exécution                            |
| F5  | Lancement automatique de Tor       | Tor démarre en arrière-plan dès qu'une `.onion` est visitée                             |
| F6  | Réutilisation d'un Tor existant    | Si Tor tourne déjà (ex: Tor Browser ouvert), le compagnon le **vérifie** puis l'utilise |
| F7  | Icône dans la barre Firefox        | Indique l'état du proxy (actif / inactif / en cours de démarrage)                       |

### 3.2 Fonctionnalités secondaires (souhaitables)

| #   | Fonctionnalité                      | Description                                                                                                                                                 |
| --- | ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F8  | Arrêt automatique de Tor            | Tor s'arrête quand Firefox est fermé ou après X minutes d'inactivité                                                                                        |
| F9  | Bouton marche/arrêt manuel          | L'utilisateur peut forcer l'activation/désactivation depuis l'icône                                                                                         |
| F10 | Notification au premier lancement   | Informe l'utilisateur que Tor est en cours de téléchargement                                                                                                |
| F11 | Mise à jour automatique de Tor      | Vérifie périodiquement si une nouvelle version est disponible                                                                                               |
| F12 | Page de diagnostic                  | Affiche l'état de Tor, le chemin du binaire, le port utilisé                                                                                                |
| F13 | Mode "Tout via Tor"                 | Tout le trafic Firefox passe par Tor, pas seulement les `.onion`                                                                                            |
| F14 | Mode "Whitelist"                    | Seuls les domaines ajoutés par l'utilisateur passent par Tor                                                                                                |
| F15 | Gestion de la whitelist             | Interface pour ajouter, modifier et supprimer des domaines depuis le popup                                                                                  |
| F16 | Bouton "Ajouter ce site"            | En un clic depuis le popup, le domaine du site actuellement visité est ajouté à la whitelist — sans avoir à taper quoi que ce soit                          |
| F17 | Désactivation automatique de WebRTC | En mode "Tout via Tor", WebRTC est désactivé automatiquement dans Firefox — réactivé dès que l'utilisateur quitte ce mode                                   |
| F18 | Option WebRTC pour les autres modes | Dans les modes "Onion uniquement" et "Whitelist", une case à cocher dans le popup permet à l'utilisateur de désactiver WebRTC manuellement s'il le souhaite |

### 3.3 Hors périmètre (explicitement exclus)

- Pas de VPN, pas de chiffrement du trafic normal (hors `.onion`)
- Pas de gestion de plusieurs circuits Tor simultanés
- Pas d'interface pour publier un service `.onion`
- Pas de gestion des cookies ou de l'historique spécifique à Tor

---

## 4. Spécifications techniques

### 4.1 Compagnon Rust

| Élément               | Choix                                |
| --------------------- | ------------------------------------ |
| Langage               | Rust (édition 2021)                  |
| Runtime async         | `tokio`                              |
| Sérialisation JSON    | `serde` + `serde_json`               |
| Téléchargement        | `reqwest`                            |
| Vérification hash     | `sha2`                               |
| Communication Firefox | Native Messaging (stdin/stdout JSON) |
| Gestion processus Tor | `std::process::Command`              |

**Protocole Native Messaging** : chaque message échangé avec Firefox est un JSON précédé de 4 octets indiquant sa taille (standard Mozilla).

Messages supportés :

```json
// Firefox → Compagnon
{ "action": "start" }
{ "action": "stop" }
{ "action": "status" }

// Compagnon → Firefox
{ "status": "starting" }
{ "status": "ready", "port": 9050 }
{ "status": "stopped" }
{ "status": "error", "message": "..." }
```

### 4.2 Extension Firefox

| Élément                 | Choix                                    |
| ----------------------- | ---------------------------------------- |
| Langage                 | JavaScript (Manifest V3)                 |
| API proxy               | `browser.proxy.onRequest`                |
| Communication compagnon | `browser.runtime.connectNative()`        |
| Permissions requises    | `proxy`, `nativeMessaging`, `webRequest` |

**Les 3 modes de routage :**

| Mode                    | Icône  | Comportement                                                               |
| ----------------------- | ------ | -------------------------------------------------------------------------- |
| 🧅 **Onion uniquement** | Violet | Seules les URLs en `.onion` passent par Tor — tout le reste va directement |
| 🌍 **Tout via Tor**     | Bleu   | L'intégralité du trafic Firefox est routée via Tor                         |
| 📋 **Whitelist**        | Orange | Seuls les domaines ajoutés par l'utilisateur passent par Tor               |

**Algorithme de routage (appliqué à chaque requête) :**

```
Mode "Onion uniquement" :
  Si URL se termine par ".onion" → Tor
  Sinon                          → Direct

Mode "Tout via Tor" :
  Toujours                       → Tor

Mode "Whitelist" :
  Si domaine est dans la liste   → Tor
  Si URL se termine par ".onion" → Tor  (toujours, même en whitelist)
  Sinon                          → Direct
```

> **Note :** les `.onion` passent toujours par Tor quel que soit le mode — c'est non négociable, car ils sont inaccessibles sans Tor.

**Persistance :** le mode actif et la liste whitelist sont sauvegardés dans le stockage local de Firefox (`browser.storage.local`), côté extension uniquement — le compagnon Rust n'a pas besoin de les connaître.

**Interface whitelist (dans le popup) :**

- Champ texte pour saisir un domaine (ex: `monsite.com`)
- Bouton "Ajouter le site actuel" pour ajouter d'un clic le domaine visité
- Liste des domaines ajoutés avec bouton de suppression par entrée

### 4.3 Détection et vérification d'un Tor existant

Avant de lancer son propre processus Tor, le compagnon Rust **vérifie** ce qui tourne éventuellement sur les ports connus.

**Ports sondés dans l'ordre :**

| Port proxy | Port de contrôle | Utilisé par                     |
| ---------- | ---------------- | ------------------------------- |
| 9050       | 9051             | Tor installé en service système |
| 9150       | 9151             | Tor Browser                     |

**Algorithme de vérification :**

```
Pour chaque paire (port_proxy, port_contrôle) :

  1. Tenter une connexion TCP sur port_contrôle
     → Échec : rien ne tourne ici, on passe à la paire suivante

  2. Envoyer : AUTHENTICATE ""
     → Réponse attendue : 250 OK
     → Autre réponse : ce n'est PAS Tor → passer à la paire suivante

  3. Envoyer : GETINFO version
     → Réponse attendue : 250-version=0.4.x.x  (format Tor)
     → Autre réponse : ce n'est PAS Tor → passer à la paire suivante

  4. Vérifier que la version ≥ version minimale acceptée (ex: 0.4.7)
     → Trop ancienne : ignorer, lancer notre propre Tor

  5. ✅ C'est bien Tor, version acceptable → réutiliser ce proxy

Si aucune paire ne passe la vérification :
  → Lancer notre propre Tor sur le premier port libre trouvé
```

**Ce que ça protège :**

- Un proxy SOCKS5 quelconque qui tournerait par hasard sur le port 9050 ne sera jamais confondu avec Tor
- Un vieux Tor trop ancien (et potentiellement vulnérable) ne sera pas réutilisé
- Le port exact utilisé est toujours communiqué à l'extension (`"port": 9050` ou autre)

**Module Rust dédié :** `tor_detector.rs`

---

### 4.4 Compatibilité OS

| OS                    | Support       | Notes                         |
| --------------------- | ------------- | ----------------------------- |
| Windows 10/11         | ✅ Prioritaire | Installeur `.exe`             |
| macOS 12+             | ✅ Secondaire  | Installeur `.pkg`             |
| Linux (Ubuntu/Debian) | ✅ Secondaire  | Paquet `.deb` ou script shell |

### 4.5 Tor Expert Bundle

- Source officielle : `https://www.torproject.org/download/tor/`
- Hashes SHA-256 codés en dur dans le compagnon (comme dans tornion)
- Stockage local : `%APPDATA%\OnionRouter\tor\` (Windows) / `~/.local/share/onionrouter/tor/` (Linux/Mac)

---

## 5. Expérience utilisateur

### 5.1 Installation (une seule fois)

```
1. L'utilisateur télécharge l'installeur depuis le site du projet
2. Il l'exécute → le compagnon Rust est installé + enregistré auprès de Firefox
3. Il installe l'extension depuis addons.mozilla.org (lien fourni par l'installeur)
4. C'est tout.
```

### 5.2 Usage quotidien

```
1. L'utilisateur tape une adresse .onion dans Firefox
2. L'icône de l'extension passe en "chargement" (⏳)
3. Le compagnon lance Tor en arrière-plan (5-15 secondes au premier lancement)
4. L'icône passe au vert (✅) — le site s'affiche
5. Les prochaines visites .onion sont instantanées (Tor reste actif)
```

### 5.3 États de l'icône

| Icône            | Signification                        |
| ---------------- | ------------------------------------ |
| ⚫ Gris           | Compagnon non connecté / Tor inactif |
| 🟡 Jaune         | Tor en cours de démarrage            |
| 🟢 Vert (violet) | Tor actif — Mode `.onion` uniquement |
| 🟢 Vert (bleu)   | Tor actif — Mode "Tout via Tor"      |
| 🟢 Vert (orange) | Tor actif — Mode "Whitelist"         |
| 🔴 Rouge         | Erreur (voir diagnostic)             |

### 5.4 Changer de mode

Depuis le popup de l'icône, l'utilisateur voit :

```
┌─────────────────────────────────┐
│  🧅 OnionRouter          [●]    │
│  ───────────────────────────    │
│  Mode actif :                   │
│  ○ Onion uniquement             │
│  ○ Tout via Tor                 │  ← WebRTC désactivé automatiquement
│  ○ Whitelist                    │
│  ───────────────────────────    │
│  ☐ Désactiver WebRTC            │  ← visible en mode Onion / Whitelist
│  ───────────────────────────    │
│  [+ Ajouter ce site]            │  ← visible en mode Whitelist
│  monsite.com              [✕]   │
│  autresite.org            [✕]   │
└─────────────────────────────────┘
```

Règles d'affichage du popup :

- La case "Désactiver WebRTC" n'apparaît **pas** en mode "Tout via Tor" (WebRTC est déjà coupé automatiquement)
- La section whitelist n'apparaît que si le mode "Whitelist" est sélectionné
- Un petit cadenas 🔒 s'affiche à côté de "Tout via Tor" pour signaler la protection renforcée

---

## 6. Sécurité

| Risque                                 | Mesure                                                                           |
| -------------------------------------- | -------------------------------------------------------------------------------- |
| Binaire Tor corrompu ou falsifié       | Vérification SHA-256 obligatoire avant exécution                                 |
| Fuite DNS                              | Firefox forcé à résoudre les DNS via Tor (`socks_remote_dns = true`)             |
| Fuite WebRTC en mode "Tout via Tor"    | WebRTC désactivé automatiquement à l'activation du mode                          |
| Fuite WebRTC en mode Onion / Whitelist | Case à cocher dans le popup — désactivation manuelle au choix de l'utilisateur   |
| Exécution non autorisée du compagnon   | Le compagnon ne répond qu'à Firefox (vérification de l'origine Native Messaging) |
| Mise à jour malveillante               | Hashes connus codés en dur ; versions inconnues refusées                         |
| Proxy imposteur sur le port 9050       | Vérification obligatoire via le Tor Control Port avant toute réutilisation       |
| Tor trop ancien réutilisé              | Version minimale vérifiée via `GETINFO version` ; refusé si inférieure au seuil  |

---

## 7. Structure des dépôts GitHub

```
onionrouter/                  ← Repo principal (monorepo)
├── companion/                ← Code Rust du compagnon
│   ├── src/
│   │   ├── main.rs
│   │   ├── tor_manager.rs    ← Téléchargement, vérification, lancement de Tor
│   │   ├── tor_detector.rs   ← Vérification d'un Tor existant via Control Port
│   │   ├── messaging.rs      ← Protocole Native Messaging Firefox
│   │   └── proxy.rs          ← Gestion SOCKS5
│   └── Cargo.toml
├── extension/                ← Code de l'extension Firefox
│   ├── manifest.json
│   ├── background.js         ← Logique proxy + communication compagnon
│   ├── popup.html            ← Interface de l'icône
│   └── icons/
├── installer/                ← Scripts d'installation (Windows/Mac/Linux)
└── README.md
```

---

## 8. Phases de développement

### Phase 1 — Fondations (MVP)

- [ ] Compagnon Rust : Native Messaging fonctionnel avec Firefox
- [ ] Compagnon Rust : Téléchargement + vérification SHA-256 du binaire Tor
- [ ] Compagnon Rust : Lancement de Tor et détection du port prêt
- [ ] Extension : Détection des URLs `.onion`
- [ ] Extension : Routage via le proxy SOCKS5
- [ ] Extension : Icône avec état basique (actif/inactif)

### Phase 2 — Robustesse

- [ ] Détection d'un Tor existant via sondage TCP des ports 9050/9150
- [ ] Vérification que c'est bien Tor via le Control Port (AUTHENTICATE + GETINFO version)
- [ ] Vérification de la version minimale de Tor
- [ ] Fallback : lancement de notre Tor si aucun Tor valide détecté
- [ ] Gestion des erreurs et messages clairs pour l'utilisateur
- [ ] Arrêt propre de Tor à la fermeture de Firefox
- [ ] Support Windows + Linux

### Phase 3 — Modes avancés

- [ ] Mode "Tout via Tor" (routage global)
- [ ] Mode "Whitelist" avec interface d'ajout/suppression de domaines
- [ ] Bouton "Ajouter le site actuel" dans le popup
- [ ] Persistance du mode et de la whitelist via `browser.storage.local`
- [ ] Couleur de l'icône selon le mode actif

### Phase 4 — Distribution

- [ ] Installeur Windows (`.exe`)
- [ ] Installeur Linux (`.deb` / script)
- [ ] Publication sur addons.mozilla.org
- [ ] Page de diagnostic dans l'extension
- [ ] Support macOS

---

## 9. Critères de succès

- ✅ Un utilisateur non-technique peut installer et utiliser l'outil en moins de 5 minutes
- ✅ Une adresse `.onion` s'ouvre dans Firefox sans configuration manuelle
- ✅ Le trafic normal (hors `.onion`) n'est jamais modifié
- ✅ L'exécutable compagnon pèse moins de 10 Mo
- ✅ Aucune dépendance externe requise côté utilisateur
