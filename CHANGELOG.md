# Changelog — Norylum Server

Toutes les modifications importantes de Norylum Server sont
documentées dans ce fichier.

Ce journal suit les principes de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/)
et du versionnement sémantique.

**Projet IA-Propelled — Human-Directed, AI-Built.**

Le développement est réalisé avec l'intelligence artificielle,
sous direction, tests et validation humaine.

Les entrées ci-dessous décrivent le développement sur `dev`.
Elles ne constituent pas des publications de versions stables.

---

## [Unreleased]

### Acquisition V3.1 - Worker reservation engine

- Added atomic acquisition claims with 90-second leases.
- Added token fencing against stale workers.
- Added crash recovery with a three-attempt limit.
- Added an offline-only acquisition worker smoke test.
- No network downloader or automatic media availability enabled.


### Acquisition V2 - Request lifecycle controls

- Added principal-only approval and cancellation endpoints.
- Enforced atomic SQLite state transitions.
- Rejected duplicate or invalid state transitions.
- Prevented unsafe cancellation of in-progress downloads.
- Preserved request history and account isolation.
- Added HTTP authorization and persistence restart tests.
- Approval never starts a downloader without VPN validation.



### Acquisition V1 - Initial security and request foundation

- Added Gluetun network isolation for torrent seeding.
- Removed direct seeder port publishing.
- Added explicit VPN startup and health gating.
- Added offline, networkless torrent maintenance.
- Added SQLite-backed authenticated acquisition requests.
- Added request deduplication and profile isolation.
- Added initial acquisition API security tests.
- Downloads remain disabled until VPN validation.



### Torrent V3 - Isolated lifecycle validation

- Added an explicit, ignored E2E integration test.
- Runs real seeder and cleanup executables.
- Uses synthetic media and a temporary SQLite database.
- Forces loopback networking without DHT or trackers.
- Verifies cleanup refusal while the seeder is active.
- Verifies original preservation and disabled restart.



### Torrent V3 - Network and snapshot hardening

**Added**
- Default localhost-only TCP/UDP port publishing.
- Live peer-count metrics from librqbit.
- Guarded offline snapshot cleanup CLI.
- Explicit user confirmation before deletion.
- Concurrent activation protection via SQLite transaction.
- Regression tests for original-media preservation
  and refusal of unexpected snapshot contents.

**Safety**
- No automatic snapshot deletion.
- No changes to accounts, TMDB or original media.
- Seeding still requires explicit authorization.

---



### Torrent V2 — Persistent multi-torrent management

**Added**
- SQLite torrent_shares and torrent_settings.
- Principal-only authenticated torrent control API.
- Independent media enable/disable.
- Shared global upload rate limit.
- Multi-torrent librqbit coordinator.
- Persistent transfer counters and runtime states.
- Docker administration without manual API tokens.
- SQLite, access-control and multi-share tests.

**Changed**
- Replaced single-media .env configuration.
- Torrent network session starts only with enabled shares.
- Existing V1 shares are not automatically reactivated.

**Limitations**
- Peer count adapter remains pending.
- Live runtime state may be stale after a process crash.
- Snapshot cleanup is intentionally manual.

---



### Torrent V1 — Opt-in librqbit

**Added**
- Module norylum-torrent avec snapshots verifies SHA-256.
- Seeder Rust avec librqbit.
- Service Docker Compose facultatif.
- Commandes torrent-enable, torrent-disable,
  torrent-status et torrent-logs.
- Plafond d'envoi configurable, 64 Kio/s par defaut.
- Tests des snapshots, du verrou exclusif
  et de l'exclusion des copies du catalogue.

**Security**
- Aucun partage par defaut.
- Confirmation explicite avant activation.
- Video originale jamais utilisee comme stockage torrent.
- Refus des chemins non fiables et des snapshots differents.
- Un seul media partage a la fois dans V1.

**Known limitations**
- L'adresse IP publique peut etre exposee par BitTorrent.
- Les snapshots occupent de l'espace disque.
- Pas encore de telechargement ni de gestion multi-torrent.

---



### Docker hardening - process lock and maintenance checks

- Exclusive OS-level volume lock shared by the server and offline metadata writer.
- Rejects second Norylum process on the same media directory.
- `doctor`, `backup` and `restart-check` commands.
- `update` refuses a dirty Git tree and verifies service health after rebuild.
- Keeps admin identities, TMDB associations and SQLite on persistent storage.
- External clients and torrent seeders are not yet covered by these locks.



### Library Ingestion V2 + Docker (2026-11-03)

- Worker d'ingestion automatique sur marqueurs `.ready` (polling configurable).
- Indexation incrémentale SQLite sans rescan global ni purge de la bibliothèque.
- Journal persistant `ingestion_jobs` : tentatives, erreurs, reprise partielle.
- API principale `GET /v1/library/ingest/jobs`.
- Image Docker multi-stage, Compose, volumes persistants, santé HTTP.
- Configuration initiale unique du compte administrateur et du jeton TMDB.
- Script d'installation, sauvegarde SQLite et mise à jour Docker en une commande.
- Par défaut exposition du seul port local 127.0.0.1:8120.

---



### Library Ingestion V1 — 2026-11-03

- Protected `POST /v1/library/ingest/ready` for the principal profile.
- Isolated `.norylum-staging` folder excluded from media scanning.
- Explicit complete-file notification, two-point stability check, strict ffprobe.
- No-overwrite hard-link publication, rollback on index failure.
- Serialized scans, durable SQLite catalog updates, stable media IDs.
- Path validation and HTTP/scan regression tests.


### Metadata V2 — 2026-11-03

**Added**
- Outil Rust hors ligne d'insertion de tags.
- Identifiants TMDB dans MKV, MP4 et M4V.
- Provenance generique Norylum Server.
- Numeros de saison et episode pour les series.
- Fusion des tags globaux Matroska existants.
- Copie de securite originale avant remplacement.
- Verification des pistes et des tags apres ecriture.
- Tests de fusion XML et de resolution TMDB.

**Security**
- Mode previsualisation par defaut.
- Refus si Norylum ecoute sur le port 8120.
- Verification de stabilite du fichier source.
- Aucune ecriture dans le conteneur en ligne.
- Aucune identite d'instance ajoutee aux tags.

**Notes**
- Usage hors ligne pour eviter les conflits avec
  le streaming et le futur seeding BitTorrent.
- La detection de tous les consommateurs externes
  du fichier n'est pas encore disponible.

---



### Metadata V1 — 2026-11-03

**Added**
- Client HTTPS de consultation TMDB.
- Recherche des films et series, en francais.
- Suggestions d'association depuis les titres indexes.
- Association manuelle par TMDB ID.
- Cache SQLite des synopsis, titres et affiches.
- Tables distinctes pour les films et les series.
- Provenance generique `Norylum Server`.
- Tests unitaires du client, de la persistance et de l'API.

**Security**
- Acces TMDB configure par jeton cote serveur.
- Recherche et modification reservees au profil principal.
- Lecture des fiches filtree par controle parental.
- Aucun identifiant d'instance dans les metadonnees.
- URLs d'affiches limitees au domaine officiel TMDB.

**Notes**
- Aucun fichier video modifie dans cette version.
- L'association automatique reste volontairement desactivee.
- Les appels TMDB reels necessitent un jeton valide.

---



### Playback Sync V2 — 2026-11-03

**Added**
- Rejeu idempotent des ecritures via operation_id.
- Migration SQLite additive des operations synchronisees.
- Accuse d'operation conserve pendant 7 jours.
- Etat serveur actuel inclus dans les conflits HTTP 409.
- GET /v1/playback/history, limite a 100 medias.
- Tests de rejeu, conflits et reprise apres interruption.

**Security**
- Identifiants d'operation scopes par compte,
  profil et media.
- Aucun ecrasement par un rejeu ancien.
- Historique filtre selon les permissions du profil.

---



### Playback Sync V1 — 2026-11-03

**Added**
- Revisions persistantes des progressions dans SQLite.
- GET /v1/playback/sync.
- PUT /v1/playback/sync/:media_id.
- GET /v1/playback/continue-watching.
- Gestion des conflits de revision avec HTTP 409.
- Tests de reprise multi-appareils et de conflits.

**Changed**
- L'ancienne ecriture de progression incremente
  maintenant la revision.
- Migration additive des progressions existantes.

**Security**
- Filtrage des progressions selon le profil authentifie.
- Controle parental avant lecture ou ecriture.
- Validation des positions et durees non finies.

---



### Devices V2 — 2026-11-03

**Added**
- Renommage des appareils par API.
- Maintenance explicite des sessions expirees.
- Retention de 30 jours pour les appareils inactifs.
- Suppression volontaire de l'historique d'un appareil.
- Tests du nettoyage et de l'isolation entre comptes.

**Security**
- Gestion reservee au profil principal.
- Refus de supprimer un appareil ayant encore des sessions.
- Nettoyage transactionnel et strictement limite au compte.
- Conservation des appareils ayant des sessions actives.

---

### Devices V1 — 2026-11-03

**Added**
- Identifiants d'appareils generes par le serveur.
- Identifiants publics de sessions, distincts des jetons.
- `GET /v1/devices` et `GET /v1/devices/sessions`.
- Revocation d'un appareil ou d'une session par API.
- Champ facultatif `device_name` a la connexion.
- Migration additive des sessions historiques.
- Tests de revocation et d'isolation entre comptes.

**Security**
- Gestion des appareils reservee au profil principal.
- Controle du compte directement dans les requetes SQLite.
- Aucune exposition des jetons dans les listes de sessions.
- Identite de l'appareil provenant des sessions verifiees,
  et non des en-tetes transmis par le client.
- Changement de profil lie a l'appareil d'origine.

---

### Profiles V3 — 2026-11-03

**Added**
- PIN de 6 a 8 chiffres pour les profils adultes.
- Configuration et suppression des PIN via l'API.
- Hachage Argon2id avec sel aleatoire.
- Migration SQLite additive et idempotente.
- Tests unitaires des PIN et de leur verrouillage.
- Tests HTTP des nouvelles routes.

**Security**
- Gestion du PIN reservee au profil principal avec
  verification du mot de passe du compte.
- Verrouillage du PIN pendant 15 minutes apres
  cinq tentatives incorrectes.
- Revocation des sessions lors d'un changement de PIN.
- Suppression du PIN lors du passage en profil enfant.
- Mot de passe du compte conserve pour la recuperation.

---

Les changements en cours de développement seront documentés
ici avant leur intégration dans une version publiée.

### Added
- Historique de développement maintenu dans `CHANGELOG.md`.
- Profiles V2 : `PUT /v1/profiles/:profile_id`.
- Profiles V2 : `DELETE /v1/profiles/:profile_id`.
- Tests unitaires et HTTP des permissions de profils.

### Changed
- Le changement vers un profil adulte exige le mot de passe
  du compte via `account_password`.

### Security
- Modification des profils reservee au profil principal.
- Suppression du profil principal interdite.
- Transformation du profil principal en enfant interdite.
- Revocation atomique des sessions des profils modifies.
- Suppression des sessions et progressions des profils supprimes.
- Verification atomique de la session source et des droits
  lors du changement de profil.


---

## Historique de développement

### 2026-11-03 — Profiles V1

**Added**
- Implémentation du module `norylum-profiles`.
- Endpoint `GET /v1/profiles`.
- Endpoint `POST /v1/profiles`.
- Gestion des profils adultes et enfants.
- Persistance des profils dans SQLite.
- Isolation des profils par compte.
- Restrictions de visibilité pour les profils enfants.
- Création des profils réservée au profil principal.
- Validation des noms et des limites d'âge.
- Tests des permissions et des doublons.

**Commit**
- [`10deec6`](https://github.com/Norylum/server/commit/10deec6) — feat(profiles): add account-scoped profile management

### 2026-11-03 — Authentication V1

**Added**
- Authentification locale des comptes.
- Création initiale d'un administrateur.
- Hachage des mots de passe avec Argon2id.
- Sessions Bearer avec expiration.
- Stockage des empreintes de sessions dans SQLite.
- Endpoint `POST /v1/auth/login`.
- Endpoint `GET /v1/auth/me`.
- Endpoint `POST /v1/auth/logout`.
- Endpoint `POST /v1/auth/switch-profile`.
- Migration SQLite pour comptes, profils et sessions.
- Tests du cycle de vie des sessions.

**Security**
- Validation des sessions par un middleware Axum.
- Suppression de la confiance directe dans les en-têtes
  d'identité fournis par les clients.
- Vérification des informations de compte et de profil
  depuis la base SQLite.
- Révocation des sessions à la déconnexion.

**Commit**
- [`f6d671d`](https://github.com/Norylum/server/commit/f6d671d) — feat(auth): implement secure authentication

### 2026-11-02 — Identité du projet

**Documentation**
- Officialisation de Norylum comme projet IA-Propelled.
- Documentation de la collaboration IA / supervision humaine.
- Documentation initiale de l'architecture Rust.
- Présentation des fonctionnalités et des objectifs.
- Documentation des commandes Cargo.

**Commit**
- [`e7cbdbb`](https://github.com/Norylum/server/commit/e7cbdbb) — docs: declare Norylum as an AI-Propelled project

### 2026-10-30 — Catalogue et séries

**Added**
- Identifiants déterministes pour séries, saisons et épisodes.
- Analyse des noms de fichiers multimédias.
- Classification des films, épisodes et médias inconnus.
- Modèle SQLite des séries, saisons et épisodes.
- Migrations additives et idempotentes.
- Catalogue des films et séries.
- Endpoints de consultation des séries et saisons.
- Tests d'intégration du scan, du streaming HTTP 206,
  de la progression de lecture et de la persistance.

**Changed**
- Extension des dépendances du workspace.
- Mise à jour du `.gitignore` pour les fichiers SQLite
  et plusieurs formats vidéo.

**Commits**
- [`b1fd646`](https://github.com/Norylum/server/commit/b1fd646) — Tests d'intégration étendus.
- [`93383a1`](https://github.com/Norylum/server/commit/93383a1) — Routes API films et séries.
- [`1afd7a5`](https://github.com/Norylum/server/commit/1afd7a5) — Services de catalogue.
- [`756e3bc`](https://github.com/Norylum/server/commit/756e3bc) — Modèle relationnel SQLite.
- [`86a7f7a`](https://github.com/Norylum/server/commit/86a7f7a) — Classification des médias.
- [`2385481`](https://github.com/Norylum/server/commit/2385481) — Parseur de noms de fichiers.
- [`101fc28`](https://github.com/Norylum/server/commit/101fc28) — Identifiants déterministes.
- [`d96174c`](https://github.com/Norylum/server/commit/d96174c) — Dépendances du workspace.
- [`d36f6e2`](https://github.com/Norylum/server/commit/d36f6e2) — Extensions du gitignore.

### 2026-10-29 — Fondation du serveur

**Added**
- Structure initiale du serveur Rust.
- Workspace Cargo modulaire.
- API HTTP avec Axum.
- Streaming Direct Play avec HTTP 206 Partial Content.
- Intégration de SQLite en mode WAL.
- Persistance locale du catalogue et de la progression.

**Commits**
- [`1ff0d73`](https://github.com/Norylum/server/commit/1ff0d73) — Persistance SQLite.
- [`1de9915`](https://github.com/Norylum/server/commit/1de9915) — Workspace Rust et streaming.
- [`f77e334`](https://github.com/Norylum/server/commit/f77e334) — Initial commit.

---

## Politique de maintenance du changelog

Pour chaque nouveau lot de développement :

1. Ajouter les changements dans `[Unreleased]`.
2. Utiliser les catégories Added, Changed, Fixed,
   Security, Deprecated ou Removed selon le besoin.
3. Décrire les modifications effectivement implémentées.
4. Ne documenter comme validés que les tests exécutés.
5. Lors d'une publication, créer une section de version
   datée et déplacer les changements correspondants.
6. Conserver les références aux commits importants.

## Sécurité

Norylum Server est actuellement en développement.

L'authentification et les contrôles d'accès nécessitent
encore des améliorations avant une exposition publique,
notamment HTTPS, limitation des tentatives de connexion
et protections supplémentaires des profils.

---

**Norylum — IA-Propelled Project**

*Human-Directed. AI-Built. Continuously Improved.*
