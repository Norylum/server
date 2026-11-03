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
  scripts/norylum-docker.sh update   Sauvegarder SQLite + git pull + rebuild + healthcheck
  scripts/norylum-docker.sh backup   Sauvegarder SQLite (WAL inclus)
  scripts/norylum-docker.sh doctor   Diagnostic sans afficher les secrets
  scripts/norylum-docker.sh restart-check
                                    Redemarrer et verifier comptes/profils/TMDB
  scripts/norylum-docker.sh logs     Consulter les journaux
  scripts/norylum-docker.sh status   Etat et URL locale
  scripts/norylum-docker.sh scan     Scan authentifie sans token manuel
  scripts/norylum-docker.sh jobs     Journal d'ingestion authentifie
  scripts/norylum-docker.sh torrent-enable MEDIA_ID [KIO_PAR_SEC]
                                    Activer explicitement un partage BitTorrent
  scripts/norylum-docker.sh torrent-disable
                                    Arreter le partage
  scripts/norylum-docker.sh torrent-status
                                    Afficher l'etat du seeder
  scripts/norylum-docker.sh torrent-logs
                                    Afficher les journaux et le magnet
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

require_clean_dev() {
  [[ "$(git branch --show-current)" == dev ]] || {
    echo 'Mise a jour reservee a la branche dev' >&2; return 1;
  }
  [[ -z "$(git status --porcelain)" ]] || {
    echo 'Arbre Git modifie: commit/stash requis avant update' >&2; return 1;
  }
}

wait_healthy() {
  local port i
  port="$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)"
  port="${port:-8120}"
  for ((i=0; i<60; i++)); do
    if curl --fail --silent --max-time 2        "http://127.0.0.1:${port}/health" >/dev/null; then
      echo "OK: serveur sain sur 127.0.0.1:${port}"
      return 0
    fi
    sleep 2
  done
  "${DC[@]}" logs --tail=80 server >&2 || true
  echo 'Echec du controle de sante. Ne pas supprimer la sauvegarde SQLite.' >&2
  return 1
}

# Stable identity fingerprint, not a database dump.
# IDs and metadata associations must survive restart and image changes.
identity_fingerprint() {
  command -v sqlite3 >/dev/null || {
    echo 'sqlite3 requis pour restart-check' >&2; return 1;
  }
  command -v shasum >/dev/null || {
    echo 'shasum requis pour restart-check' >&2; return 1;
  }
  [[ -f media/norylum.sqlite3 ]] || {
    echo 'Base SQLite absente' >&2; return 1;
  }
  sqlite3 -readonly media/norylum.sqlite3     "SELECT 'account:' || id FROM accounts
     UNION ALL SELECT 'profile:' || id || ':' || account_id FROM profiles
     UNION ALL SELECT 'movie:' || media_id || ':' || tmdb_id FROM media_metadata
     UNION ALL SELECT 'show:' || show_id || ':' || tmdb_id FROM show_metadata
     ORDER BY 1;" | shasum -a 256 | awk '{print $1}'
}

restart_check() {
  require_config
  command -v sqlite3 >/dev/null || {
    echo 'Installer sqlite3 pour ce controle' >&2; return 1;
  }
  local before after
  before="$(identity_fingerprint)"
  echo 'Redemarrage du conteneur Norylum...'
  "${DC[@]}" restart server
  wait_healthy
  after="$(identity_fingerprint)"
  if [[ "$before" != "$after" ]]; then
    echo 'ERREUR: comptes/profils/associations TMDB modifies apres restart' >&2
    return 1
  fi
  # This calls the actual login endpoint without exposing the returned token.
  api_call /v1/auth/me >/dev/null
  echo 'OK: comptes, profils, associations TMDB et login admin conserves'
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


torrent_enable() {
  require_config

  local media_id="${1:-}"
  local limit="${2:-64}"

  if [[ ! "$media_id" =~ ^med_[[:xdigit:]]{16,32}$ ]]; then
    echo "MediaId Norylum invalide" >&2
    return 1
  fi

  if [[ ! "$limit" =~ ^[0-9]+$ ]] ||
     (( 10#$limit < 8 || 10#$limit > 512 )); then
    echo "Limite autorisee : 8 a 512 Kio/s" >&2
    return 1
  fi

  command -v sqlite3 >/dev/null || {
    echo "sqlite3 requis pour verifier le MediaId" >&2
    return 1
  }

  local count
  count="$(sqlite3 -readonly media/norylum.sqlite3     "SELECT COUNT(*) FROM media_items WHERE id='$media_id';")"

  if [[ "$count" != "1" ]]; then
    echo "Media non indexe : $media_id" >&2
    return 1
  fi

  echo "Activation demandee pour : $media_id"
  echo "Limite d'envoi : $limit Kio/s"
  echo "BitTorrent peut exposer ton adresse IP publique."
  echo "Une copie du media sera conservee pour le partage."
  printf 'Confirmer le partage autorise (ecrire OUI) : '

  local answer
  IFS= read -r answer
  if [[ "$answer" != "OUI" ]]; then
    echo "Activation annulee."
    return 0
  fi

  python3 - "$media_id" "$limit" <<'PYCONFIG'
from pathlib import Path
import sys

path = Path(".env")
data = [
    line for line in path.read_text().splitlines()
    if not line.startswith((
        "NORYLUM_TORRENT_MEDIA_ID=",
        "NORYLUM_TORRENT_UPLOAD_KIBPS=",
    ))
]

data.extend([
    "NORYLUM_TORRENT_MEDIA_ID=" + sys.argv[1],
    "NORYLUM_TORRENT_UPLOAD_KIBPS=" + sys.argv[2],
])

path.write_text("\n".join(data) + "\n")
path.chmod(0o600)
PYCONFIG

  "${DC[@]}" --profile torrent up -d --no-deps --force-recreate seeder
  "${DC[@]}" --profile torrent ps

  echo "Verifier : scripts/norylum-docker.sh torrent-logs"
}

torrent_disable() {
  require_config
  "${DC[@]}" --profile torrent stop seeder || true

  python3 <<'PYCONFIG'
from pathlib import Path

path = Path(".env")
data = [
    line for line in path.read_text().splitlines()
    if not line.startswith("NORYLUM_TORRENT_MEDIA_ID=")
]
data.append("NORYLUM_TORRENT_MEDIA_ID=")
path.write_text("\n".join(data) + "\n")
path.chmod(0o600)
PYCONFIG

  echo "Partage desactive."
  echo "Les snapshots existants ne sont pas supprimes."
}

torrent_status() {
  "${DC[@]}" --profile torrent ps seeder
}

torrent_logs() {
  "${DC[@]}" --profile torrent logs --tail=100 seeder
}

case "${1:-}" in
  setup) setup ;;
  start)
    if [[ ! -f .env ]]; then echo 'Executer setup avant start' >&2; exit 1; fi
    "${DC[@]}" up -d --build server
    if grep -q '^NORYLUM_TORRENT_MEDIA_ID=med_' .env; then
      "${DC[@]}" --profile torrent up -d seeder
    fi
    "${DC[@]}" --profile torrent ps
    ;;
  stop) "${DC[@]}" --profile torrent down ;;
  update)
    require_config
    require_clean_dev
    backup_db
    git pull --ff-only origin dev
    "${DC[@]}" build --pull
    "${DC[@]}" up -d --no-deps --force-recreate server
    wait_healthy
    if grep -q '^NORYLUM_TORRENT_MEDIA_ID=med_' .env; then
      "${DC[@]}" --profile torrent up -d --no-deps --force-recreate seeder
    fi
    "${DC[@]}" --profile torrent ps
    ;;
  backup) require_config; backup_db ;;
  doctor) scripts/norylum-doctor.sh ;;
  restart-check) restart_check ;;
  logs) "${DC[@]}" logs -f --tail=100 server ;;
  status) "${DC[@]}" ps; echo "URL locale : http://127.0.0.1:$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)" ;;
  scan) scan ;;
  jobs) api_call /v1/library/ingest/jobs ;;
  torrent-enable) torrent_enable "${2:-}" "${3:-64}" ;;
  torrent-disable) torrent_disable ;;
  torrent-status) torrent_status ;;
  torrent-logs) torrent_logs ;;
  ingest) ingest "${2:-}" ;;
  *) usage; exit 2 ;;
esac
