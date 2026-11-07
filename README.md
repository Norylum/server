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
│   └── norylum-server/        (server, seeder, metadata-tags, torrent-cleanup)
├── crates/
│   ├── acquisition-transport/ (worker queue, source registry)
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
│   ├── streaming/
│   └── torrent/               (librqbit integration, snapshot, seeding lifecycle)
└── shared/
    ├── api-types/
    └── test-support/
```

Les modules sont organisés par responsabilité afin de faciliter la maintenance, les tests et les évolutions futures.

## ✅ Fonctionnalités implémentées

- Scan et indexation des bibliothèques multimédias (films et séries).
- Organisation des saisons et épisodes ; analyse des noms de fichiers.
- Extraction des métadonnées techniques ; identifiants de médias stables.
- Stockage SQLite avec mode WAL.
- Authentification Argon2id, sessions Bearer, comptes et profils multi-utilisateurs.
- Codes PIN par profil, appareils et sessions avec cycle de vie complet.
- Streaming Direct Play, HTTP 200 et 206 Partial Content.
- Synchronisation de la progression de lecture entre appareils (avec idempotence).
- Métadonnées enrichies via TMDB (synopsis, affiches, associations manuelles).
- Écriture des tags MKV/MP4 (`norylum-metadata-tags`).
- Ingestion automatique et manuelle avec validation ffprobe.
- Déploiement Docker avec sauvegarde SQLite, rotation des logs.
- Moteur BitTorrent opt-in avec librqbit (`norylum-seeder`), snapshots SHA-256, nettoyage explicite.
- VPN Gluetun (NordVPN/ProtonVPN/Mullvad…), isolation réseau, kill switch validé.
- Diagnostics VPN : DNS, pare-feu, IPv6, kill switch fail-closed et tunnel actif.
- Worker d'acquisition : réservations atomiques, sources de confiance, téléchargement HTTPS via VPN avec vérification SHA-256.
- Cache local des affiches TMDB : téléchargement à l'association, serving via `GET /v1/images/posters/:filename`.
- Limitation de débit par IP sur les routes d'authentification (10 req/min, `governor`).
- Interface d'administration Web WASM (Leptos 0.7, Trunk, nginx) — module optionnel (`--profile web`).
- Tests unitaires et d'intégration ; test E2E cycle de vie torrent (isolé).

## 🚧 Développement en cours

- Transcodage et streaming adaptatif (GPU : QSV, NVENC, VAAPI, VideoToolbox).

Les modules présents dans le workspace ne sont pas tous entièrement implémentés. Ne pas exposer le serveur publiquement sans reverse proxy HTTPS.

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


## Devices V1 — Appareils et sessions

Un appareil est une instance de connexion creee par le serveur.
Ce n'est pas une empreinte materielle certifiee.

Lors de `POST /v1/auth/login`, le client peut envoyer :

```json
{
  "username": "mon-compte",
  "password": "mon-mot-de-passe",
  "device_name": "Mac"
}
```

La reponse inclut `device_id` et `session_id`.
Le changement de profil conserve le meme `device_id`
mais genere un nouveau `session_id`.

Routes accessibles au profil principal :

- `GET /v1/devices` : appareils du compte.
- `GET /v1/devices/sessions` : sessions non expirees.
- `DELETE /v1/devices/:device_id` : revoquer un appareil.
- `DELETE /v1/devices/sessions/:session_id` : revoquer une session.

Les reponses n'exposent jamais les jetons Bearer ou leurs hashes.
Une revocation est prise en compte a la requete suivante.

La migration SQLite conserve les sessions historiques et
leur attribue des identifiants serveur. Les appareils sans
session active restent visibles dans l'historique.

Une nouvelle connexion avec le bon mot de passe cree une
nouvelle entree appareil : la revocation ne constitue pas
une interdiction permanente de reconnexion.


## Devices V2 — Cycle de vie

Le profil principal peut gerer le nom des appareils et
nettoyer l'historique de ses connexions.

Routes :

- `PATCH /v1/devices/:device_id` : renommer un appareil.
- `POST /v1/devices/maintenance` : nettoyer le compte.
- `DELETE /v1/devices/:device_id/history` : oublier
  un appareil sans session.

Exemple pour renommer :

```json
{
  "name": "Television du salon"
}
```

La maintenance est explicite : elle n'est pas lancee
periodiquement par le serveur.

Elle supprime les sessions expirees du compte courant,
puis les appareils sans aucune session et dont la
derniere activite remonte a plus de 30 jours.

Un appareil ayant encore des sessions ne peut pas etre
oublie. Il faut d'abord revoquer ces sessions ou
executer la maintenance si elles sont expirees.

Le nettoyage ne modifie pas les autres comptes,
les profils ou la progression de lecture.


## Playback Sync V1 — Reprise entre appareils

La progression appartient a un compte, un profil et un media.
Tous les appareils connectes au meme profil consultent
la meme progression dans SQLite.

Routes :

- `GET /v1/playback/sync` : progressions du profil.
- `PUT /v1/playback/sync/:media_id` : ecriture conditionnelle.
- `GET /v1/playback/continue-watching` : lectures en cours.
- `GET /v1/playback/:media_id` : lecture unitaire existante.
- `POST /v1/playback/progress` : ecriture historique.

Exemple de mise a jour initiale :

```json
{
  "position_seconds": 120.0,
  "duration_seconds": 1200.0
}
```

Si aucune progression n'existe, sa revision devient 1.
Pour modifier une progression existante :

```json
{
  "position_seconds": 300.0,
  "duration_seconds": 1200.0,
  "expected_revision": 1
}
```

Le serveur renvoie une nouvelle revision, ici 2.
En cas de revision obsolete, il retourne HTTP 409.
Le client doit relire la progression avant de reessayer.

L'ancien POST /v1/playback/progress reste compatible :
il incremente la revision mais utilise le principe
last-write-wins. Les nouveaux clients doivent utiliser
l'ecriture conditionnelle pour eviter les conflits silencieux.

Les medias non autorises au profil connecte sont exclus
des listes et refuses en consultation ou en ecriture.
Les lectures terminees (90 pour cent ou plus) ne figurent
pas dans continue-watching, limite a 50 entrees.

Les revisions sont migrees sans effacer les progressions
existantes. La synchronisation concerne les appareils
connectes au meme serveur Norylum, pas plusieurs serveurs
independants.


## Playback Sync V2 — Reprise resiliente

Les clients peuvent transmettre un `operation_id` stable
sur PUT /v1/playback/sync/:media_id.

Exemple :

```json
{
  "position_seconds": 300.0,
  "duration_seconds": 1200.0,
  "expected_revision": 1,
  "operation_id": "operation-550e8400-e29b"
}
```

Le client doit generer un identifiant aleatoire unique
pour chaque nouvelle ecriture et le reutiliser en cas
de repetition apres une perte de reponse HTTP.

Une meme operation avec le meme contenu et le meme
identifiant est rejouee sans augmenter la revision.
Un identifiant reutilise avec un contenu different
est refuse avec HTTP 400.

Les accuses d'operations sont conserves pendant 7 jours.
Au-dela, le rejeu n'est plus garanti ; une ancienne
revision provoque normalement HTTP 409.

La reponse rejouee est le resultat historique de
l'operation. Elle peut etre plus ancienne que l'etat
actuel du serveur : il faut synchroniser avant de
reprendre la lecture.

En cas de conflit HTTP 409, la reponse contient
`current`, une progression du serveur. Le client
peut la conserver, ou presenter a l'utilisateur
un choix explicite avant une nouvelle mise a jour.

GET /v1/playback/history retourne les 100 derniers
medias consultes par le profil, tries selon updated_at.
Il s'agit d'un historique recent avec un etat par
media, et non d'un journal de chaque lecture.

L'ancien POST /v1/playback/progress reste last-write-wins.
Les clients compatibles V2 doivent utiliser
l'ecriture avec operation_id et expected_revision.


## Metadata V1 — TMDB, synopsis et affiches

Norylum peut rechercher des films et series sur TMDB,
puis associer manuellement leur identifiant aux
enregistrements locaux.

Configuration facultative au demarrage :

- `NORYLUM_TMDB_READ_TOKEN` : API Read Access Token TMDB.

Sans ce jeton, la lecture des fiches deja presentes
dans SQLite fonctionne, mais les recherches et
nouvelles associations TMDB sont indisponibles.

Routes reservees au profil principal pour les
recherches et les modifications :

- `GET /v1/metadata/search?kind=movie&query=Matrix`
- `GET /v1/metadata/search?kind=tv&query=Breaking%20Bad`
- `GET /v1/metadata/media/:media_id/suggestions`
- `GET /v1/metadata/shows/:show_id/suggestions`
- `PUT /v1/metadata/media/:media_id`
- `PUT /v1/metadata/shows/:show_id`

Routes de consultation pour les profils autorises :

- `GET /v1/metadata/media/:media_id`
- `GET /v1/metadata/shows/:show_id`

Exemple d'association manuelle :

```json id="lt3atb"
{
  "tmdb_id": 603
}
```

L'association recupere le detail officiel depuis TMDB.
Elle stocke dans SQLite :

- Le TMDB ID et le type : `movie/603` ou `tv/1396`.
- Le titre et le synopsis en francais, si disponibles.
- Le chemin de l'affiche et sa provenance TMDB.
- La date de sortie ou de premiere diffusion.
- La provenance generique `Norylum Server`.

Aucun identifiant de l'instance Norylum, domaine, IP
ou nom de serveur n'est stocke dans ces fiches.

Les associations sont explicites : Norylum ne choisit
pas automatiquement parmi des resultats ambigus.

Les associations existantes sont conservees lors des
rescans des memes medias. Une association manuelle
peut etre corrigee avec un nouveau PUT.

Metadata V1 ne modifie aucun fichier MKV, MP4 ou autre.
L'ecriture des tags dans les conteneurs sera realisee
dans Metadata V2, apres verification de l'integrite
des fichiers et avant une eventuelle mise en torrent.

Les images sont referencees par URL TMDB ; elles ne sont
pas encore telechargees dans un cache d'images local.

Attribution TMDB obligatoire pour les interfaces :

"This product uses the TMDB API but is not endorsed or
certified by TMDB."

Les interfaces qui affichent ces donnees devront aussi
respecter les exigences de logo et d'attribution TMDB.

La provenance generique dans les fichiers ne garantit
pas l'anonymat d'un serveur sur le reseau BitTorrent.


## Metadata V2 — Tags internes MKV / MP4

Norylum fournit un outil Rust pour ecrire les tags
des films et episodes deja associes a TMDB.

Outils requis :

- `ffprobe` et `ffmpeg`.
- `mkvextract` et `mkvpropedit` pour les MKV.

Les tags ecrits sont :

- `TMDB=movie/603` pour un film.
- `TMDB=tv/1396` pour une serie.
- `TV_SEASON` et `TV_EPISODE` pour un episode.
- `NORYLUM_PROVENANCE=Norylum Server`.

Aucun nom d'instance, adresse IP, compte ou domaine
Norylum n'est ajoute par cet outil.

La commande ne necessite pas le jeton TMDB :
elle reutilise les associations de Metadata V1.

### Utilisation hors ligne

1. Indexer les medias avec Norylum.
2. Associer les fiches TMDB via Metadata V1.
3. Arreter le serveur et tout logiciel qui utilise,
   telecharge ou partage le fichier.
4. Previsualiser les tags :

```bash id="82mpwh"
cargo run -p norylum-server --bin norylum-metadata-tags -- \
    --media-id med_ID
```

5. Ecrire les tags :

```bash id="b3n5em"
cargo run -p norylum-server --bin norylum-metadata-tags -- \
    --media-id med_ID --apply
```

Le repertoire media est celui de NORYLUM_MEDIA_DIR,
ou ./media en l'absence de cette variable.

Le programme :
- Refuse de fonctionner si le serveur Norylum
  repond sur le port local 8120.
- Refuse les fichiers modifies depuis moins de
  30 secondes.
- Cree une copie temporaire sur le meme volume.
- Preserve les tags globaux MKV sans rapport avec
  Norylum.
- Utilise -c copy pour les MP4, sans reencodage.
- Verifie les signatures techniques des pistes,
  le nombre de chapitres et les nouveaux tags.
- Revalide la taille et la date de modification
  du fichier d'origine avant remplacement.
- Conserve le fichier original en .bak.
- Met a jour la taille enregistree dans SQLite.

Cette verification ne constitue pas une preuve
bit-a-bit de chaque piste. Une verification de
lecture manuelle reste necessaire.

Le fichier original n'est pas modifie directement.
En cas de refus ou d'echec pendant la preparation,
il reste en place.

Le remplacement final s'effectue serveur arrete,
par renommage sur le meme volume. En cas d'echec
du second renommage, une restauration est tentee.
La sauvegarde .bak reste disponible pour
restauration manuelle.

Les sauvegardes .bak sont volumineuses : verifier
l'espace libre avant chaque operation.

Attention : le controle du port 8120 ne detecte
ni un autre lecteur externe, ni un client torrent
tiers, ni une autre instance sur un port different.
L'operateur doit s'assurer que le fichier n'est
pas utilise. Le verrou partage avec le futur moteur
torrent est prevu pour une prochaine etape.

Les MP4 peuvent utiliser des champs mdta que
certains lecteurs ne montrent pas.

Ne jamais modifier un fichier en cours de seeding :
modifier ses octets invaliderait les pieces du
torrent. Le partage sera prepare apres l'ecriture
finale des metadonnees.


## Library Ingestion V1: explicit notify_file_ready

Trusted local operators copy completed media into `media/.norylum-staging/`.
This directory is excluded from the library scanner and not exposed to viewers.
For example, place a complete `Example (2020).mkv` there (do not test on an
existing film). After closing the transfer writer and letting the file remain
unchanged for at least 30 seconds, a principal Norylum profile may call:

```bash
curl -i -X POST http://127.0.0.1:8120/v1/library/ingest/ready \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  --data '{"source":"Example (2020).mkv","destination":"Example (2020).mkv"}'
```

The server waits two seconds to confirm size and modification time stability,
requires a valid video stream reported by `ffprobe`, rejects traversal, hidden
paths and symlinks, and creates the final directory entry without replacing
an existing file. It reindexes the full catalog inside a SQLite transaction;
if scanning or indexing fails, the final hard link is removed and staging
remains available for retry. Successful results return HTTP 201 with the
indexed `media` and `staging_removed` flag.

This V1 endpoint is deliberately a *ready notification*, not a multipart
upload or filesystem watcher. The external transfer must be complete before
notification. It does not identify or tag media automatically and never
starts torrent sharing. TMDB association and file tagging remain separate.
For cross-filesystem mounts, hard-link publication is unsupported. A shared
file lock with external downloaders and the future torrent engine will be a
separate version; do not ingest a file while another process is writing it.


## Installation et mises a jour avec Docker (Norylum)

Prerequis : Docker Desktop / Docker Engine avec Compose V2, Git.

Sur une machine neuve, cloner ce depot puis lancer :

```bash
scripts/norylum-docker.sh setup
scripts/norylum-docker.sh start
```

`setup` demande UNE SEULE FOIS le nom et mot de passe du premier administrateur,
ainsi que le jeton TMDB optionnel. Il ecrit `.env` et les fichiers dans
`config/` avec permissions restrictives ; ces fichiers sont ignores par Git.
Le jeton est relu a chaque demarrage du conteneur. L'administrateur est
cree uniquement si la base ne contient aucun compte. Une simple mise a jour
ne recree donc ni son compte, ni ses profils, ni les sessions conservees dans
SQLite. Ne pas supprimer `media/norylum.sqlite3` ni `config/`.

Le repertoire local `media/` est monte dans le conteneur (`/data/media`) :
films, series et SQLite persistent lors des reconstructions des images.
Si la base existait deja avant Docker, saisir le **compte admin actuel** dans
`setup` (et non un nouveau) : le bootstrap est idempotent et n'ecrase pas
les identifiants existants. Au premier demarrage dans Docker, Norylum detecte
les anciens chemins absolus macOS, effectue un scan de rebasage et conserve
les media IDs par chemin relatif.

```bash
scripts/norylum-docker.sh status
scripts/norylum-docker.sh logs
scripts/norylum-docker.sh jobs
scripts/norylum-docker.sh scan
scripts/norylum-docker.sh update
scripts/norylum-docker.sh stop
```

`update` effectue une sauvegarde transactionnelle SQLite via `.backup`,
un `git pull --ff-only origin dev`, reconstruit l'image Docker et remplace
le conteneur. L'application n'est pas exposee directement a Internet :
`127.0.0.1:8120` est publie sur la machine hote. Pour un usage distant,
configurer plus tard un reverse proxy HTTPS avec authentification et controle
d'acces. **Ne jamais lancer l'ancien serveur natif et le conteneur Docker
contre la meme base simultanement.** Les mises a jour restent **manuelles**,
avec une commande unique.

Le mot de passe initial reste dans `config/admin_password` pour les commandes
locales `scan` et `jobs` ; le stocker de maniere sure, et proteger les acces a
l'hote ainsi qu'aux sauvegardes. Les secrets ne sont pas ecrits dans
`compose.yaml` ou l'image Docker.

### Ingestion V2 automatique (marqueur explicite)

Les nouvelles videos doivent etre copiees completement dans
`media/.norylum-staging/` ; creer ensuite un fichier vide portant le nom
`NOM_DE_VIDEO.mkv.ready`. Le serveur surveille ces marqueurs (par defaut
toutes les 15 secondes), puis attend au moins 30 secondes sans modification,
execute ffprobe, publie par lien dur et indexe le fichier de maniere
**incrementale** sans purger les autres medias. Le marqueur est efface en cas
de succes et l'operation est inscrite dans `ingestion_jobs` SQLite.
La route HTTP `POST /v1/library/ingest/ready` reste disponible et partage
le verrou d'indexation avec l'ingestion automatique et le scan manuel.

Pour copier facilement un film present sur la machine hote :

```bash
scripts/norylum-docker.sh ingest "/chemin/Film (2026).mkv"
```

La copie se termine avant la creation du marqueur `.ready`. Ne jamais
utiliser cette commande sur une video dont on ne dispose pas des droits
necessaires. La recuperation apres crash est prise en charge pour la fenetre
lien dur + indexation, mais une panne ou une corruption externe peut toujours
necessiter une intervention. Les erreurs restent visibles via
`/v1/library/ingest/jobs`. Une simple detection de taille stable ne prouve
pas qu'un programme tiers a ferme le fichier : seul le producteur doit
produire `.ready` a la fin du transfert.

L'association TMDB et l'ecriture MKV/MP4 sont pour l'instant des operations
separees. Aucun moteur torrent ni seeding automatique n'est active.


## Docker reliability V1 - single-instance media volume lock

The server and offline `norylum-metadata-tags --apply` both acquire
`media/.norylum-instance.lock` through an operating-system advisory lock.
The lock is automatically released when the process exits or crashes.
The lock file itself stays in place and must not be manually deleted.
This prevents two cooperating Norylum server instances or the offline
metadata writer from accessing the same media volume concurrently.

**Limitations:** this does not lock external video players, download
clients or BitTorrent seeders; they do not participate in the protocol.
The current server can still stream while its ingestion worker publishes
other files. A per-file reader/writer lease and torrent lifecycle controls
are required before enabling torrent seeding. Advisory lock behavior across
Docker Desktop bind mounts should be checked on each target platform.

Docker maintenance commands:

```bash
scripts/norylum-docker.sh doctor
scripts/norylum-docker.sh backup
scripts/norylum-docker.sh restart-check
scripts/norylum-docker.sh update
```

`restart-check` verifies that account/profile IDs, TMDB associations,
and admin authentication survive a restart. It does not print passwords
or access tokens. `update` requires a clean `dev` branch, creates an
online SQLite backup before pulling changes, rebuilds, and verifies the
HTTP health endpoint. A failed healthcheck does not mean the database
should be rolled back automatically: inspect the logs and backup first.

Do not run the native server and Docker server on the same media volume.
For a new system, provide matching existing admin credentials during setup
if importing a populated SQLite database; Norylum never resets the existing
admin password when starting a container.

## Torrent V1 — Partage volontaire avec librqbit

Norylum dispose d'un moteur BitTorrent separe,
desactive par defaut via un profil Docker Compose.

Le partage est limite a un media a la fois dans V1.

Le processus de seeding :
- exige un MediaId present dans SQLite ;
- exige une association TMDB existante ;
- refuse les fichiers instables ou trop recents ;
- refuse les liens symboliques ;
- cree un snapshot physique du fichier ;
- verifie le contenu avec SHA-256 ;
- cree le torrent a partir du snapshot ;
- n'ecrit jamais dans la video originale.

Les copies se trouvent dans :

    media/.norylum-torrent-snapshots/

Elles ne sont pas indexees dans la bibliotheque.

Le snapshot occupe approximativement la taille
du fichier original. Il est conserve apres l'arret
du seeding ; aucune suppression automatique.

### Activer

Choisir uniquement un fichier dont la distribution
par BitTorrent est autorisee.

    scripts/norylum-docker.sh torrent-enable med_ID 64

La commande exige une confirmation explicite.

Le nombre 64 est le plafond d'envoi applicatif
en Kio/s. Valeurs acceptees : 8 a 512.

Le conteneur seeder ecoute sur le port TCP/UDP 6881.
Le profil Docker torrent n'est demarre que si
un partage a ete volontairement active.

### Etat et arret

    scripts/norylum-docker.sh torrent-status
    scripts/norylum-docker.sh torrent-logs
    scripts/norylum-docker.sh torrent-disable

La desactivation stoppe le conteneur et efface
l'option active dans le fichier .env local.

Le fichier .torrent est ecrit dans le repertoire
de snapshots. Le lien magnet figure dans les logs.

### Limites et confidentialite

BitTorrent peut reveler l'adresse IP publique
aux pairs et aux services de decouverte.
La mention generique Norylum Server dans la video
ne protege pas l'adresse IP des communications.

Le moteur utilise DHT, sans ajout automatique de
tracker et sans UPnP. Le port doit etre ouvert
pour certaines connexions entrantes.

Le plafond d'envoi porte sur le trafic gere
par librqbit et non necessairement sur tous
les octets transmis par la machine.

Norylum ne supprime pas les metadonnees
personnelles deja presentes dans le fichier.

Ne partager que des contenus dont la distribution
est autorisee. Les telechargements BitTorrent
ne sont pas implementes dans cette version.

## Torrent V2 - Multi-torrent persistant

Le controle du seeding est desormais stocke dans SQLite.

Les anciennes variables NORYLUM_TORRENT_MEDIA_ID et
NORYLUM_TORRENT_UPLOAD_KIBPS ne sont plus utilisees.

La migration ne reactive aucun partage V1 automatiquement.
Les anciennes copies torrent sont conservees sans modification.

### Architecture

- Un processus norylum-server : HTTP, auth, catalogue.
- Un processus norylum-seeder : coordination BitTorrent.
- Une seule session librqbit pour plusieurs medias.
- Un plafond global d'envoi, partage entre tous les torrents.
- Une liste SQLite d'autorisations par MediaId.
- Un snapshot independant par media.

Le service seeder Docker peut fonctionner en attente.
Lorsque la liste des medias autorises est vide, il ne
demarre aucune session BitTorrent et ne contacte aucun pair.

### Administration

    scripts/norylum-docker.sh torrent-status
    scripts/norylum-docker.sh torrent-limit 64
    scripts/norylum-docker.sh torrent-enable med_ID
    scripts/norylum-docker.sh torrent-disable med_ID
    scripts/norylum-docker.sh torrent-logs

Aucun nouveau mot de passe ou jeton TMDB n'est necessaire.
Les commandes se connectent avec l'administrateur existant.

La commande torrent-enable exige la confirmation OUI,
puis utilise l'API authentifiee du profil principal.

### API

    GET  /v1/torrent/shares
    GET  /v1/torrent/settings
    PUT  /v1/torrent/settings
    POST /v1/torrent/media/:media_id/enable
    POST /v1/torrent/media/:media_id/disable

Pour activer un partage, le corps JSON doit contenir :

    {"confirm_distribution_rights":true}

L'API enregistre la demande, puis le seeder la prend en
compte au cycle suivant (environ cinq secondes).

Le status stopping indique une demande d'arret.
Le status disabled indique que le seeder a traite l'arret.

### Statistiques

uploaded_bytes est un compteur cumule persistant.
upload_bps est une estimation issue des relevés du seeder.
peer_count reste null tant que l'adaptateur de pairs
n'est pas implemente et verifie.

En cas d'arret brutal du seeder, les derniers etats
peuvent rester affiches jusqu'au redemarrage.
Les donnees historiques restent dans SQLite.

### Securite et limites

- Aucun partage n'est active par defaut.
- Les fichiers originaux ne sont pas utilises comme source
  directe du moteur BitTorrent.
- Les snapshots doivent etre conserves tant que les
  torrents correspondants sont actifs.
- Les limites de debit sont applicatives et ne couvrent
  pas l'ensemble du trafic IP du systeme.
- BitTorrent peut exposer l'adresse IP publique des pairs.
- La verification des droits de distribution reste
  la responsabilite de l'utilisateur.

Ne pas supprimer manuellement les snapshots pendant
le seeding. Aucun nettoyage automatique des copies n'est
effectue dans cette version.

## Torrent V3 - Network hardening and guarded cleanup

### Network

The seeder's published TCP and UDP ports are bound to
127.0.0.1 by default:

    NORYLUM_TORRENT_BIND_IP=127.0.0.1

An externally reachable address requires an explicit,
separate network configuration decision.

Loopback port publishing does not anonymize outgoing
BitTorrent, DHT or peer communications.

No BitTorrent session starts without an enabled share.

### Peer metrics

peer_count reports live connected peers when librqbit
provides an active statistics snapshot. Otherwise null
indicates that the measurement is unavailable.

### Snapshot cleanup

A snapshot can be deleted only through an explicit
administrative operation:

    scripts/norylum-docker.sh torrent-disable med_ID
    scripts/norylum-docker.sh torrent-status
    scripts/norylum-docker.sh torrent-cleanup med_ID

Before deletion:
- Every torrent must be disabled.
- The target must have status disabled in SQLite.
- The seeder must release the exclusive lock.
- The snapshot folder must contain exactly one
  regular MKV, MP4 or M4V file.
- Symlinks, subdirectories and unknown contents
  cause the operation to fail.
- SQLite holds a write transaction during deletion.
- The terminal requires the word SUPPRIMER.

The operation removes the isolated snapshot and
its corresponding metainfo file. It never removes
the original library video.

The process never deletes a snapshot automatically.
Historic transfer counters remain in SQLite.

Do not remove the media/.norylum-torrent-snapshots/.seeder.lock file manually.
Do not use filesystem cleanup tools directly on an
active or partially prepared torrent.

## Torrent V3 - Isolated lifecycle integration test

The test runs the real seeder and cleanup executables
against a temporary SQLite database and synthetic media.

Run explicitly:

    cargo test -p norylum-server \
      --test torrent_v3_lifecycle \
      -- --ignored --nocapture

Safety characteristics:
- Debug builds only for the isolated seeder mode
- Temporary directory with a required E2E marker
- TCP listener bound to 127.0.0.1 on an ephemeral port
- DHT disabled
- Tracker communication disabled
- Local service discovery disabled
- No real media, credentials or production SQLite access
- No externally distributed torrent

The test covers idle startup, opt-in, torrent creation,
cleanup refusal while active, disable, guarded deletion,
original-file preservation and restart persistence.

The test is ignored by default to prevent accidental
process or network startup during ordinary CI runs.

This is not a two-peer data transfer benchmark.

## Acquisition V1 - VPN and authenticated requests

Norylum separates the API from the network workers.

Network security:
- Gluetun is mandatory for the torrent seeder.
- Seeder uses network_mode: service:gluetun.
- No torrent ports are published by default.
- Only Gluetun receives NET_ADMIN.
- Local cleanup runs with network_mode: none.
- VPN credentials are read from vpn-secrets/.
- Missing credentials prevent vpn-start.
- Norylum server remains accessible independently.

Supported providers:
- protonvpn (recommended)
- mullvad (recommended)
- airvpn
- nordvpn
- private internet access (OpenVPN initially)

Local configuration keys in .env:

    NORYLUM_VPN_PROVIDER=protonvpn
    NORYLUM_VPN_TYPE=wireguard

Secret file names in vpn-secrets/:

    wireguard_private_key
    wireguard_addresses
    wireguard_preshared_key
    openvpn_user
    openvpn_password

Only create files required by the selected provider.
Do not store credential values in .env or Git.

Start the VPN and seeder explicitly:

    scripts/norylum-docker.sh vpn-start
    scripts/norylum-docker.sh vpn-status

The VPN tunnel and kill switch must be tested before
authorizing any real network acquisition or seeding.

Authenticated acquisition requests:

    POST /v1/acquisition/requests
    GET /v1/acquisition/requests
    GET /v1/acquisition/requests/:id

Example POST body:

    {
      "kind": "movie",
      "tmdb_id": 603,
      "quality_profile": "1080p",
      "language": "fr"
    }

The client must provide a valid Bearer access token.
Acquisition V1 currently requires the principal profile.

The POST request returns HTTP 202.
Requests are persisted in SQLite with status
awaiting_approval.

No download starts when a request is registered.
Source selection, approval and the acquisition worker
are future stages.

TMDB identifies content but does not provide the
downloadable video file.

## Acquisition V2 - Request lifecycle

Authenticated principal-profile routes:

    POST /v1/acquisition/requests/:id/approve
    POST /v1/acquisition/requests/:id/cancel

Allowed transitions:

    awaiting_approval -> approved
    awaiting_approval -> cancelled
    approved -> cancelled

Invalid transitions return HTTP 409.
Unknown request identifiers return HTTP 404.
Unauthenticated requests return HTTP 401.

Cancellation of searching, downloading or processing
is deliberately refused until worker cancellation
and acknowledgement are implemented.

Approved requests remain pending in SQLite.
No source search or download starts automatically.
VPN and leak protection remain mandatory
before enabling acquisition network workers.

## Acquisition V3.1 - Offline worker foundation

The worker queue uses SQLite immediate transactions.
Only approved requests can be claimed.

A claim changes the request status to searching.
Leases expire after 90 seconds; up to three attempts are
allowed. Stale worker tokens cannot renew or finish a claim.

The worker currently supports only --offline-smoke.
It operates against in-memory SQLite and has no network client.
No download, seeding, or automatic ingestion is activated.

Do not activate real acquisition until Gluetun leak tests
and authorized-source validation have succeeded.

## Acquisition V3.2 - Authorized sources

An approved acquisition is not eligible for a worker claim
until a trusted source is explicitly registered.

Sources are immutable, bound to a specific account request,
require a valid HTTPS URL, an exact hostname allowlist match,
and a lowercase SHA-256 digest.

Only a live worker lease can retrieve the registered source.
Source URLs are not exposed by the public acquisition API.

The registration method is internal. No source administration
endpoint and no network download are enabled yet.

Before fetching, the future network worker must also enforce:
DNS/IP protections against SSRF and rebinding, no redirects,
strict TLS verification, size/time limits, digest verification
and a validated Gluetun kill switch.


## Architecture modulaire et VPN

- Serveur Rust headless fonctionnel sans frontend.
- Frontend Web facultatif, conteneur separe (prevu).
- Direct Play prioritaire, transcodage GPU a la demande
  avec detection des capacites (prevu).
- Recherche de sources par fournisseurs interchangeables
  (prevue).
- Seeder BitTorrent opt-in et Gluetun optionnel.
- Aucun magnet ou telechargement torrent automatique
  pour le moment.

Guide de configuration et validation :
[docs/NORDVPN_TORRENT.md](docs/NORDVPN_TORRENT.md).
