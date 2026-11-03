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
