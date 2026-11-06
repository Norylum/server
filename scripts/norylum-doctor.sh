#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DC=(docker compose --project-directory "$ROOT" -f "$ROOT/compose.yaml")

die() { echo "ERREUR: $*" >&2; exit 1; }

command -v docker >/dev/null || die 'Docker absent'
docker info >/dev/null 2>&1 || die 'Docker Desktop non demarre'
[[ -f .env ]] || die 'Configuration absente: scripts/norylum-docker.sh setup'
[[ -s config/admin_password ]] || die 'config/admin_password absent ou vide'
[[ -d media ]] || die 'Dossier media absent'
"${DC[@]}" config --quiet || die 'Configuration Compose invalide'
echo 'OK: Docker, configuration Compose, media et secret admin presents'

if [[ -f config/tmdb_token ]]; then
  [[ -s config/tmdb_token ]] || die 'config/tmdb_token vide'
  echo 'OK: fichier jeton TMDB present (valeur non affichee)'
else
  echo 'INFO: jeton TMDB absent (la consultation du cache reste possible)'
fi

if [[ -f media/norylum.sqlite3 ]]; then
  if command -v sqlite3 >/dev/null; then
    result="$(sqlite3 -readonly media/norylum.sqlite3 'PRAGMA quick_check;' 2>&1)" || die "SQLite: $result"
    [[ "$result" == 'ok' ]] || die "SQLite: $result"
    echo 'OK: SQLite PRAGMA quick_check = ok'
    sqlite3 -readonly -header -column media/norylum.sqlite3 \
      "SELECT (SELECT COUNT(*) FROM accounts) AS accounts,
              (SELECT COUNT(*) FROM profiles) AS profiles,
              (SELECT COUNT(*) FROM media_items) AS media,
              (SELECT COUNT(*) FROM media_metadata) AS movies_tmdb,
              (SELECT COUNT(*) FROM ingestion_jobs) AS ingestion_jobs;"
  else
    echo 'INFO: sqlite3 non installe sur hote: diagnostic SQLite saute'
  fi
else
  echo 'INFO: aucune base SQLite existante (premiere installation)'
fi

port="$(sed -n 's/^NORYLUM_PORT=//p' .env | head -n 1)"
port="${port:-8120}"
if curl -fsS --max-time 3 "http://127.0.0.1:${port}/health" >/dev/null; then
  echo "OK: API de sante disponible sur 127.0.0.1:${port}"
else
  echo "INFO: serveur non joignable sur 127.0.0.1:${port}"
fi

if [[ -d vpn-secrets ]]; then
  if python3 scripts/vpn_config.py 2>/dev/null; then
    echo 'OK: configuration VPN locale valide'
  else
    echo 'ATTENTION: configuration VPN locale invalide ou incomplete'
  fi

  gluetun_cid="$("${DC[@]}" --profile torrent ps -q gluetun 2>/dev/null)" || gluetun_cid=""
  if [[ -n "$gluetun_cid" ]]; then
    gluetun_health="$(docker inspect \
      --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}' \
      "$gluetun_cid" 2>/dev/null)" || gluetun_health="unknown"
    if [[ "$gluetun_health" == "healthy" ]]; then
      echo "OK: Gluetun $gluetun_health"
    else
      echo "ATTENTION: Gluetun $gluetun_health"
    fi
  else
    echo 'INFO: Gluetun non demarre (profil torrent inactif)'
  fi

  if "${DC[@]}" --profile torrent ps --status running --services 2>/dev/null |
      grep -qx seeder; then
    echo 'INFO: seeder actif — verifier torrent-status'
  else
    echo 'INFO: seeder arrete'
  fi
else
  echo 'INFO: vpn-secrets absent — VPN non configure'
fi

"${DC[@]}" ps
