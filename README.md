# Norylum Server

**IA-Propelled — Propulsé par l'Intelligence Artificielle**

Serveur multimédia auto-hébergé, développé en Rust selon une architecture monolithe modulaire.

## 🤖 Un projet développé par l'IA

**Norylum est un projet IA-Propelled : son développement est réalisé avec l'intelligence artificielle, sous direction et supervision humaine.**

L'intelligence artificielle participe directement à :

- La conception de l'architecture logicielle.
- L'écriture et l'évolution du code source.
- L'implémentation des fonctionnalités.
- La création des tests automatisés.
- L'identification et la correction des problèmes.
- La documentation et l'amélioration continue.

Le mainteneur humain définit les objectifs, pilote les décisions techniques, exécute les validations et contrôle les commits et les publications.

Ce modèle de développement repose sur une collaboration entre intelligence artificielle et supervision humaine.

**AI-Propelled Development — Human-Directed, AI-Built.**

## 🎯 Objectif

Construire un serveur multimédia moderne, performant, modulaire et auto-hébergé, permettant de gérer, organiser et diffuser une bibliothèque personnelle de films et séries.

## 🦀 Technologies

- Rust — Langage principal
- Tokio — Exécution asynchrone
- Axum — API HTTP
- SQLite — Persistance locale
- FFprobe — Inspection des médias
- Cargo — Workspace, compilation et tests

## 🏗️ Architecture

Norylum Server utilise un monolithe modulaire Rust.

```text
server/
├── apps/
│   └── norylum-server/
├── crates/
│   ├── api/
│   ├── auth/
│   ├── authorization/
│   ├── catalog/
│   ├── devices/
│   ├── foundation/
│   ├── identity/
│   ├── library/
│   ├── metadata/
│   ├── persistence/
│   ├── playback/
│   ├── profiles/
│   ├── runtime/
│   └── streaming/
└── shared/
    ├── api-types/
    └── test-support/
```

Les modules sont organisés par responsabilité afin de faciliter la maintenance, les tests et les évolutions futures.

## ✅ Fonctionnalités implémentées

- Scan des bibliothèques multimédias.
- Indexation des films et séries.
- Organisation des saisons et épisodes.
- Analyse des noms de fichiers.
- Extraction des métadonnées techniques.
- Stockage SQLite avec mode WAL.
- Identifiants de médias stables.
- Streaming Direct Play.
- Support HTTP 200 et 206 Partial Content.
- Sauvegarde de la progression de lecture.
- Contrôles d'autorisation initiaux.
- Tests unitaires et d'intégration.

## 🚧 Développement en cours

Les prochaines étapes concernent notamment :

- Authentification réelle des utilisateurs.
- Comptes et profils.
- Gestion des appareils.
- Métadonnées enrichies et affiches.
- Reprise de lecture entre appareils.
- Transcodage et streaming adaptatif.
- Sécurisation et déploiement en production.

Les modules présents dans le workspace ne sont pas tous entièrement implémentés.

**Attention :** les en-têtes d'identité utilisés actuellement ne constituent pas une authentification sécurisée. Le serveur ne doit pas être exposé publiquement dans cet état.

## 🚀 Développement local

Compiler le workspace :

```bash
cargo build --workspace
```

Vérifier le code :

```bash
cargo check --workspace
```

Exécuter les tests :

```bash
cargo test --workspace
```

Vérifier le formatage :

```bash
cargo fmt --all -- --check
```

Analyser le code avec Clippy :

```bash
cargo clippy --workspace --all-targets
```

Démarrer le serveur :

```bash
cargo run -p norylum-server
```

Par défaut, le serveur écoute sur :

`http://127.0.0.1:8120`

Le répertoire multimédia peut être configuré avec la variable `NORYLUM_MEDIA_DIR`.

## 🌿 Stratégie de développement

- `dev` : branche active de développement.
- `main` : branche de référence pour les versions stabilisées.

Les changements sont validés par compilation, tests et revue avant publication.

## 📜 Licence

GPL-3.0-or-later.

---

**Norylum — IA-Propelled Project**

*Designed with AI. Built with AI. Directed by humans.*

## Authentification locale

Norylum utilise Argon2id pour les mots de passe et des
sessions Bearer opaques stockees sous forme de hash SHA-256
dans SQLite.

Au premier demarrage, configurer :

- `NORYLUM_ADMIN_USERNAME`
- `NORYLUM_ADMIN_PASSWORD` (12 caracteres minimum)

Le bootstrap est unique et ne remplace pas un compte existant.

### Routes

- `POST /v1/auth/login` : connexion
- `GET /v1/auth/me` : session courante
- `POST /v1/auth/logout` : deconnexion
- `POST /v1/auth/switch-profile` : changement de profil

Toutes les routes de bibliotheque, streaming et progression
necessitent un jeton Bearer valide.

Les en-tetes d'identite fournis par les clients ne constituent
pas une authentification. Le middleware les remplace par les
valeurs provenant de SQLite.

Les jetons expirent apres sept jours.

### Limites de cette premiere version

Ne pas exposer directement le serveur sur Internet.
Avant tout deploiement public, ajouter HTTPS, limitation des
tentatives de connexion, gestion complete des profils et
durcissement de la configuration.


## Gestion des profils V1

Les profils sont persistants dans SQLite et isoles par compte.

- `GET /v1/profiles` : lister les profils visibles.
- `POST /v1/profiles` : creer un profil adulte ou enfant.
- `POST /v1/auth/switch-profile` : obtenir une session pour un profil.

La creation est reservee au profil principal du compte.
Un profil enfant ne peut voir que son propre profil dans
la liste et ne peut pas en creer un autre.

Exemple de creation de profil enfant :

```json
{
  "name": "Junior",
  "is_child": true,
  "age_rating_max": 10
}
```

Profiles V2 ajoute PUT /v1/profiles/:profile_id et
DELETE /v1/profiles/:profile_id. Seul le profil principal
peut modifier ou supprimer les profils secondaires.

La modification revoque toutes les sessions du profil.
La suppression revoque ses sessions et efface sa progression.
Le profil principal ne peut pas etre supprime ni devenir enfant.

Pour passer sur un profil adulte, l'endpoint
POST /v1/auth/switch-profile exige maintenant le champ
`account_password` en plus de `profile_id`.

Les codes PIN individuels restent a implementer.
Ne pas exposer le serveur publiquement.

## 📝 Historique des changements

Le développement de Norylum est documenté dans le
[CHANGELOG.md](CHANGELOG.md).

Chaque étape importante y est enregistrée avec sa date,
ses fonctionnalités et les commits correspondants.


## Profiles V3 — Codes PIN

Les profils adultes peuvent recevoir un PIN de 6 a 8 chiffres.

- `PUT /v1/profiles/:profile_id/pin` : definir ou remplacer un PIN.
- `DELETE /v1/profiles/:profile_id/pin` : supprimer un PIN.
- `POST /v1/auth/switch-profile` : utiliser `profile_pin`
  pour acceder a un profil adulte protege.

Les operations de gestion d'un PIN sont reservees au profil
principal et exigent `account_password`.

Un PIN est hache avec Argon2id. Sa valeur n'est jamais
retournee par l'API.

Apres cinq tentatives PIN incorrectes, le PIN est verrouille
pendant quinze minutes. L'etat est persiste dans SQLite.

Le mot de passe du compte reste la methode de recuperation :
il permet au proprietaire du compte de debloquer un profil.

Les modifications de PIN revoquent les sessions du profil.
Un profil enfant ne peut pas obtenir de PIN ni changer
de profil depuis sa propre session.

Le PIN est une protection d'usage familial et ne remplace
pas une authentification forte ou une exposition HTTPS
securisee. L'API reste destinee au developpement local.
