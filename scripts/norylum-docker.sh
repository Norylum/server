#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DC=(docker compose --project-directory "$ROOT" -f "$ROOT/compose.yaml")

usage() {
  cat <<'USAGE'
Norylum Docker
  scripts/norylum-docker.sh setup    Configuration initiale (une seule fois)
  scripts/norylum-docker.sh start    Construire et demarrer
  scripts/norylum-docker.sh stop     Arreter sans supprimer les donnees
  scripts/norylum-docker.sh update   Sauvegarder SQLite + git pull + rebuild
  scripts/norylum-docker.sh logs     Consulter les journaux
  scripts/norylum-docker.sh status   Etat et URL locale
  scripts/norylum-docker.sh scan     Scan authentifie sans token manuel
  scripts/norylum-docker.sh jobs     Journal d'ingestion authentifie
  scripts/norylum-docker.sh ingest FICHIER
                                    Copier un fichier dans le staging et signaler .ready
USAGE
}

require_config() {
  if [[ ! -f .env || ! -f config/admin_password ]]; then
    echo "Configuration absente. Lancer: scripts/norylum-docker.sh setup" >&2
    exit 1
  fi
}

setup() {
  if [[ -e .env || -e config/admin_password || -e config/tmdb_token ]]; then
    echo "Configuration deja presente : aucune cle ni mot de passe modifies."
    echo "Pour demarrer: scripts/norylum-docker.sh start"
    return
  fi
  mkdir -p config media/.norylum-staging
  chmod 700 config
  local username password tmdb
  printf 'Nom administrateur (compte existant si la base contient deja des comptes) [admin] : '
  IFS= read -r username
  username="${username:-admin}"
  if [[ ! "$username" =~ ^[a-zA-Z0-9_-]{3,64}$ ]]; then
    echo 'Nom admin invalide (3-64 caracteres ASCII)' >&2; exit 1
  fi
  printf 'Mot de passe admin existant ou initial (12 caracteres minimum, saisie masquee) : '
  IFS= read -rs password
  printf '\n'
  if (( ${#password} < 12 )); then echo 'Mot de passe trop court' >&2; exit 1; fi
  printf 'Jeton TMDB Read Access (saisie masquee, laisser vide pour plus tard) : '
  IFS= read -rs tmdb
  printf '\n'
  (umask 077; printf '%s' "$password" > config/admin_password)
  if [[ -n "$tmdb" ]]; then
    (umask 077; printf '%s' "$tmdb" > config/tmdb_token)
  fi
  unset password tmdb
  (umask 077; cat > .env <<ENV
NORYLUM_ADMIN_USERNAME=${username}
NORYLUM_UID=$(id -u)
NORYLUM_GID=$(id -g)
NORYLUM_PORT=8120
NORYLUM_INGEST_POLL_SECONDS=15
NORYLUM_RUST_LOG=info,norylum=debug
ENV
  )
  echo 'Configuration initiale creee dans .env et config/ (non versionnes).'
  echo 'Le compte admin est cree uniquement si SQLite ne contient aucun compte.'
}

backup_db() {
  if [[ ! -f media/norylum.sqlite3 ]]; then
    echo 'Pas de base SQLite existante a sauvegarder.'
    return
  fi
  mkdir -p media/.norylum-backups
  local filename
  filename="norylum-$(date +%Y%m%d-%H%M%S).sqlite3"
  # Online SQLite backup correctly includes WAL data; never just cp *.sqlite3.
  if "${DC[@]}" ps --status running --services 2>/dev/null | grep -qx server; then
    "${DC[@]}" exec -T server sqlite3 /data/media/norylum.sqlite3 \
      ".backup '/data/media/.norylum-backups/$filename'"
  else
    if command -v sqlite3 >/dev/null 2>&1; then
      sqlite3 media/norylum.sqlite3 ".backup 'media/.norylum-backups/$filename'"
    else
      echo 'sqlite3 absent et conteneur arrete : impossible de sauvegarder la base' >&2
      return 1
    fi
  fi
  chmod 600 "media/.norylum-backups/$filename" 2>/dev/null || true
  echo "Sauvegarde : media/.norylum-backups/$filename"
}

api_call() {
  local path="$1" token username port
  require_config
  port="$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)"
  port="${port:-8120}"
  username="$(sed -n 's/^NORYLUM_ADMIN_USERNAME=//p' .env | head -1)"
  # password transmitted via stdin; not printed nor added to shell history.
  local payload
  payload="$(python3 -c 'import sys,json; print(json.dumps({"username":sys.argv[1],"password":sys.stdin.read()}))' "$username" < config/admin_password)"
  local login_response
  login_response="$(printf '%s' "$payload" | curl --fail --silent --show-error \
    -H 'Content-Type: application/json' --data-binary @- \
    http://127.0.0.1:${port}/v1/auth/login)"
  token="$(printf '%s' "$login_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["access_token"])')"
  curl --fail --silent --show-error -H "Authorization: Bearer $token" \
    "http://127.0.0.1:${port}$path"
  printf '\n'
}

scan() {
  local token username payload response port
  require_config
  port="$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)"
  port="${port:-8120}"
  username="$(sed -n 's/^NORYLUM_ADMIN_USERNAME=//p' .env | head -1)"
  payload="$(python3 -c 'import sys,json; print(json.dumps({"username":sys.argv[1],"password":sys.stdin.read()}))' "$username" < config/admin_password)"
  response="$(printf '%s' "$payload" | curl --fail --silent --show-error \
    -H 'Content-Type: application/json' --data-binary @- \
    http://127.0.0.1:${port}/v1/auth/login)"
  token="$(printf '%s' "$response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["access_token"])')"
  curl --fail --silent --show-error -X POST \
    -H "Authorization: Bearer $token" \
    "http://127.0.0.1:${port}/v1/library/scan"
  printf '\n'
}

ingest() {
  local file="${1:-}"
  if [[ ! -f "$file" || -L "$file" ]]; then
    echo 'Fichier a importer absent ou lien symbolique refuse' >&2; exit 1
  fi
  local filename ext staging tmp
  filename="$(basename "$file")"
  ext="${filename##*.}"
  ext="$(printf '%s' "$ext" | tr '[:upper:]' '[:lower:]')"
  case "$ext" in mkv|mp4|m4v|mov|avi|webm|ts|wmv) ;; *) echo 'Extension non supportee' >&2; exit 1;; esac
  if [[ "$filename" == .* ]]; then echo 'Nom de fichier cache interdit' >&2; exit 1; fi
  staging=media/.norylum-staging
  mkdir -p "$staging"
  if [[ -e "$staging/$filename" || -e "$staging/$filename.ready" ]]; then
    echo 'Un fichier ou marqueur existe deja, operation refusee' >&2; exit 1
  fi
  tmp="$staging/.${filename}.partial.$$"
  trap 'rm -f "$tmp"' EXIT
  cp "$file" "$tmp"
  ln "$tmp" "$staging/$filename"
  rm -f "$tmp"
  touch "$staging/$filename.ready"
  trap - EXIT
  echo "Transfert termine, marqueur cree. Ingestion automatique sous ~15s apres stabilite >=30s."
  echo "Suivre: scripts/norylum-docker.sh jobs"
}

case "${1:-}" in
  setup) setup ;;
  start)
    if [[ ! -f .env ]]; then echo 'Executer setup avant start' >&2; exit 1; fi
    "${DC[@]}" up -d --build
    "${DC[@]}" ps
    ;;
  stop) "${DC[@]}" down ;;
  update)
    if [[ ! -f .env ]]; then echo 'Executer setup avant update' >&2; exit 1; fi
    backup_db
    git pull --ff-only origin dev
    "${DC[@]}" build --pull
    "${DC[@]}" up -d --no-deps --force-recreate server
    "${DC[@]}" ps
    ;;
  logs) "${DC[@]}" logs -f --tail=100 server ;;
  status) "${DC[@]}" ps; echo "URL locale : http://127.0.0.1:$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)" ;;
  scan) scan ;;
  jobs) api_call /v1/library/ingest/jobs ;;
  ingest) ingest "${2:-}" ;;
  *) usage; exit 2 ;;
esac
