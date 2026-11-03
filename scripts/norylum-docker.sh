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
  scripts/norylum-docker.sh torrent-enable MEDIA_ID
                                    Activer explicitement un partage BitTorrent
  scripts/norylum-docker.sh torrent-disable MEDIA_ID
                                    Arreter le partage
  scripts/norylum-docker.sh torrent-limit KIO_PAR_SEC
                                    Definir le debit global
  scripts/norylum-docker.sh torrent-status
                                    Afficher l'etat du seeder
  scripts/norylum-docker.sh torrent-cleanup MEDIA_ID
                                    Nettoyer un snapshot desactive
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


# Reuse stored local admin credentials; never print tokens.
torrent_api() {
  require_config

  local method="$1"
  local endpoint="$2"
  local data="${3:-}"
  local port username payload response token

  port="$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)"
  port="${port:-8120}"

  username="$(sed -n 's/^NORYLUM_ADMIN_USERNAME=//p' .env | head -1)"

  payload="$(python3 -c '
import json,sys
print(json.dumps({
  "username":sys.argv[1],
  "password":sys.stdin.read()
}))
' "$username" < config/admin_password)"

  response="$(printf '%s' "$payload" |
    curl --fail --silent --show-error       --max-time 15       -H "Content-Type: application/json"       --data-binary @-       "http://127.0.0.1:${port}/v1/auth/login")"

  token="$(printf '%s' "$response" |
    python3 -c '
import json,sys
print(json.load(sys.stdin)["access_token"])
')"

  if [[ -n "$data" ]]; then
    curl --fail --silent --show-error       --max-time 30       -X "$method"       -H "Authorization: Bearer $token"       -H "Content-Type: application/json"       --data "$data"       "http://127.0.0.1:${port}${endpoint}"
  else
    curl --fail --silent --show-error       --max-time 30       -X "$method"       -H "Authorization: Bearer $token"       "http://127.0.0.1:${port}${endpoint}"
  fi

  printf '\n'
}

torrent_enable() {
  local id="${1:-}"

  if [[ ! "$id" =~ ^med_[[:xdigit:]]{16,32}$ ]]; then
    echo "MediaId invalide : med_... requis" >&2
    return 1
  fi

  echo "Media : $id"
  echo "BitTorrent peut exposer ton adresse IP publique."
  echo "Le partage exige les droits de distribution du media."
  printf "Confirmer explicitement (ecrire OUI) : "

  local answer
  IFS= read -r answer

  if [[ "$answer" != "OUI" ]]; then
    echo "Activation annulee"
    return 0
  fi

  torrent_api POST     "/v1/torrent/media/$id/enable"     '{"confirm_distribution_rights":true}'

  # Launches the coordinator; it reads the SQLite queue.
  "${DC[@]}" --profile torrent up     -d --no-deps seeder
}

torrent_disable() {
  local id="${1:-}"

  if [[ ! "$id" =~ ^med_[[:xdigit:]]{16,32}$ ]]; then
    echo "Indiquer le MediaId a desactiver" >&2
    return 1
  fi

  torrent_api POST     "/v1/torrent/media/$id/disable"
}

torrent_limit() {
  local limit="${1:-}"

  if [[ ! "$limit" =~ ^[0-9]{1,3}$ ]]; then
    echo "Limite numerique requise : 8 a 512" >&2
    return 1
  fi

  local number=$((10#$limit))
  if (( number < 8 || number > 512 )); then
    echo "Limite autorisee : 8 a 512 Kio/s" >&2
    return 1
  fi

  local payload
  printf -v payload '{"upload_kibps":%d}' "$number"

  torrent_api PUT /v1/torrent/settings "$payload"
}

torrent_status() {
  torrent_api GET /v1/torrent/shares
}

torrent_cleanup() {
  require_config

  local id="${1:-}"

  if [[ ! "$id" =~ ^med_[[:xdigit:]]{16,32}$ ]]; then
    echo "MediaId invalide" >&2
    return 1
  fi

  command -v sqlite3 >/dev/null || {
    echo "sqlite3 requis" >&2
    return 1
  }

  local enabled
  enabled="$(sqlite3 -readonly media/norylum.sqlite3 \
    "SELECT COUNT(*) FROM torrent_shares WHERE enabled=1;")"

  if [[ "$enabled" != "0" ]]; then
    echo "Desactiver TOUS les torrents avant nettoyage" >&2
    return 1
  fi

  local running=0
  if "${DC[@]}" --profile torrent ps \
      --status running --services 2>/dev/null |
      grep -qx seeder; then
    running=1
    "${DC[@]}" --profile torrent stop seeder
  fi

  local result=0

  echo "Verification du snapshot : $id"

  if "${DC[@]}" --profile torrent run \
      --rm --no-deps \
      --entrypoint /usr/local/bin/norylum-torrent-cleanup \
      seeder --media-id "$id"; then

    printf "Supprimer uniquement ce snapshot (ecrire SUPPRIMER) : "
    local answer
    IFS= read -r answer

    if [[ "$answer" == "SUPPRIMER" ]]; then
      if ! "${DC[@]}" --profile torrent run \
          --rm --no-deps \
          --entrypoint /usr/local/bin/norylum-torrent-cleanup \
          seeder --media-id "$id" --apply; then
        result=1
      fi
    else
      echo "Nettoyage annule."
    fi
  else
    result=1
  fi

  if (( running == 1 )); then
    "${DC[@]}" --profile torrent up -d --no-deps seeder ||
      result=1
  fi

  return "$result"
}

torrent_logs() {
  "${DC[@]}" --profile torrent logs --tail=100 seeder
}

case "${1:-}" in
  setup) setup ;;
  start)
    if [[ ! -f .env ]]; then echo 'Executer setup avant start' >&2; exit 1; fi
    "${DC[@]}" up -d --build server
    # Idle coordinator does not start librqbit until SQLite authorizes a seed.
    "${DC[@]}" --profile torrent up -d --no-deps seeder
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
    "${DC[@]}" --profile torrent up       -d --no-deps --force-recreate seeder
    "${DC[@]}" --profile torrent ps
    ;;
  backup) require_config; backup_db ;;
  doctor) scripts/norylum-doctor.sh ;;
  restart-check) restart_check ;;
  logs) "${DC[@]}" logs -f --tail=100 server ;;
  status) "${DC[@]}" ps; echo "URL locale : http://127.0.0.1:$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)" ;;
  scan) scan ;;
  jobs) api_call /v1/library/ingest/jobs ;;
  torrent-enable) torrent_enable "${2:-}" ;;
  torrent-limit) torrent_limit "${2:-}" ;;
  torrent-disable) torrent_disable "${2:-}" ;;
  torrent-status) torrent_status ;;
  torrent-cleanup) torrent_cleanup "${2:-}" ;;
  torrent-logs) torrent_logs ;;
  ingest) ingest "${2:-}" ;;
  *) usage; exit 2 ;;
esac
