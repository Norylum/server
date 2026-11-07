# Norylum Admin Web — Design Spec

**Date :** 2026-11-06  
**Axe :** 2 — Interface d'administration Web (WASM)  
**Statut :** Approuvé, en attente d'implémentation

---

## Objectif

Fournir une interface Web d'administration pour Norylum Server. L'interface ne lit pas de vidéos. Elle permet de gérer la médiathèque, les acquisitions, le seeder BitTorrent et les profils. Elle est un module **optionnel** : le serveur headless fonctionne sans elle.

---

## Architecture

### Nouveau workspace member

```
apps/norylum-web/
  src/
    main.rs         ← Leptos app entry point (CSR)
    app.rs          ← composant racine, router
    pages/
      login.rs
      dashboard.rs
      media.rs
      acquisitions.rs
      seeder.rs
    components/
      nav.rs
      status_badge.rs
    api.rs          ← fonctions de fetch vers l'API Norylum
  Cargo.toml
  index.html        ← shell HTML pour Trunk
  Trunk.toml
  Dockerfile
```

### Docker

Nouveau service `norylum-web` sous profil `web` dans `compose.yaml` :
- Build multi-stage : `rust:1-slim` + Trunk → `nginx:alpine`
- nginx sert le bundle WASM sur port 3000 (exposé `127.0.0.1:3000:80`)
- nginx proxifie `/api/` → `http://server:8120/` sur le réseau Docker interne
- Le service `server` n'est pas modifié

```yaml
norylum-web:
  profiles: [web]
  build:
    context: apps/norylum-web
    dockerfile: Dockerfile
  ports:
    - "127.0.0.1:3000:80"
  depends_on:
    server:
      condition: service_healthy
  restart: unless-stopped
```

### nginx config (interne au conteneur)

```nginx
server {
    listen 80;
    root /usr/share/nginx/html;
    index index.html;

    location /api/ {
        proxy_pass http://server:8120/;
        proxy_set_header Host $host;
    }

    location / {
        try_files $uri $uri/ /index.html;
    }
}
```

---

## Nouvelle route API

### `POST /v1/acquisition/requests/:id/source`

**Auth :** Bearer token, profil principal uniquement.

**Corps :**
```json
{
  "source_url": "https://cdn.example.org/film.mp4",
  "expected_sha256": "abc...64chars",
  "allowed_hosts": ["cdn.example.org"]
}
```

**Réponses :**
- `201 Created` — source enregistrée
- `400 Bad Request` — validation URL/SHA-256/hosts échouée
- `404 Not Found` — demande inconnue
- `409 Conflict` — source déjà enregistrée ou statut incompatible
- `401 / 403` — non authentifié / profil non principal

**Implémentation :** `crates/api/src/acquisition_routes.rs` (ou fichier équivalent existant), appelle `db.register_acquisition_source()`.

---

## UI V1 — Modules

### Login

- Formulaire username/password → `POST /api/v1/auth/login`
- Token Bearer stocké dans `sessionStorage` (pas de cookie)
- Redirection vers Dashboard si déjà connecté

### Dashboard

- Compteurs : médias, profils, jobs d'ingestion en cours
- Statut VPN : `GET /api/v1/vpn/status` (proxifié)
- Statut seeder : `GET /api/v1/torrent/shares`

### Médiathèque

- Liste paginée : `GET /api/v1/library`
- Badge TMDB par média
- Lien vers association TMDB : `PUT /api/v1/metadata/media/:id`

### Acquisitions

- Liste des demandes : `GET /api/v1/acquisition/requests`
- Approve : `POST /api/v1/acquisition/requests/:id/approve`
- Cancel : `POST /api/v1/acquisition/requests/:id/cancel`
- Enregistrer source : `POST /api/v1/acquisition/requests/:id/source` ← nouvelle route

### Seeder

- État des partages : `GET /api/v1/torrent/shares`
- Enable/disable par média
- Paramètre débit global

---

## Tech

| Choix | Version | Raison |
|---|---|---|
| Leptos | 0.7 | CSR WASM, stable, pas de Node.js |
| Trunk | latest | Bundler WASM Rust natif |
| Tailwind CSS | CDN Play | Pas de build step, pas de Node.js |
| wasm-bindgen | workspace | Généré par Leptos |

**Pas de dépendance Node.js dans le build Docker.**

---

## Contraintes

- Pas de lecteur vidéo, pas de streaming.
- Token stocké en `sessionStorage` (effacé à la fermeture de l'onglet).
- nginx ne distribue pas les secrets VPN.
- L'interface ne démarre pas le VPN — elle affiche son état.
- `norylum-web` est optionnel : `docker compose --profile web up` uniquement si voulu.

---

## Hors scope V1

- Authentification multi-comptes dans l'UI
- Gestion des codes PIN
- Codes PIN dans l'UI
- Transcoding controls
- Vue historique des téléchargements
