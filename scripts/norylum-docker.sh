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
  scripts/norylum-docker.sh vpn-connect
                                    Connecter Gluetun seul
  scripts/norylum-docker.sh vpn-egress-check
                                    Verifier la sortie Internet VPN
  scripts/norylum-docker.sh vpn-dns-check
                                    Verifier le resolver DNS du tunnel
  scripts/norylum-docker.sh vpn-firewall-check
                                    Afficher les regles iptables Gluetun
  scripts/norylum-docker.sh vpn-ipv6-check
                                    Verifier le blocage IPv6 sortant
  scripts/norylum-docker.sh vpn-killswitch-test
                                    Tester le fail-closed dans un env isole
  scripts/norylum-docker.sh vpn-killswitch-active-test
                                    Tester le kill switch sur tunnel actif
  scripts/norylum-docker.sh vpn-start
                                    Demarrer Gluetun puis seeder
  scripts/norylum-docker.sh vpn-status
                                    Afficher la sante du VPN
  scripts/norylum-docker.sh vpn-stop
                                    Arreter le VPN sans arreter Norylum
  scripts/norylum-docker.sh acquisition-requests
                                    Lister les demandes authentifiees
  scripts/norylum-docker.sh acquisition-run
                                    Lancer un cycle d'acquisition sous VPN
  scripts/norylum-docker.sh acquisition-logs
                                    Afficher les journaux du dernier worker
  scripts/norylum-docker.sh acquisition-status
                                    Etat du service acquisition-worker
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
  scripts/norylum-docker.sh web-start
                                    Construire et demarrer l'interface web WASM
  scripts/norylum-docker.sh web-stop
                                    Arreter l'interface web
  scripts/norylum-docker.sh web-logs
                                    Afficher les journaux de l'interface web
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

vpn_check_config() {
  require_config
  python3 "$ROOT/scripts/vpn_config.py"
}

vpn_status() {
  local cid health

  cid="$("${DC[@]}" --profile torrent ps -q gluetun)"

  if [[ -z "$cid" ]]; then
    echo "VPN inactif"
    return 1
  fi

  health="$(docker inspect \
    --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}' \
    "$cid")" || return 1

  echo "Etat Gluetun : $health"
  [[ "$health" == healthy ]]
}

vpn_wait_healthy() {
  local i

  for ((i=0; i<45; i++)); do
    if vpn_status >/dev/null 2>&1; then
      echo "OK : Gluetun healthy"
      return 0
    fi
    sleep 2
  done

  echo "ERREUR : Gluetun non healthy, seeder bloque" >&2
  return 1
}

vpn_stop() {
  # Stop only VPN-dependent services, never the main Norylum API.
  "${DC[@]}" --profile torrent stop seeder gluetun
}

vpn_connect() {
  vpn_check_config || return 1

  # Refuse de recreer le tunnel sous un seeder actif.
  if "${DC[@]}" --profile torrent ps \
      --status running --services 2>/dev/null |
      grep -qx seeder; then
    echo "Arreter le seeder avant de reconfigurer Gluetun" >&2
    return 1
  fi

  "${DC[@]}" --profile torrent up -d gluetun || return 1
  vpn_wait_healthy || return 1
}

vpn_egress_check() {
  if ! vpn_status >/dev/null; then
    echo "VPN non healthy : test refuse" >&2
    return 1
  fi

  local host_ip vpn_ip

  host_ip="$(curl --ipv4 --noproxy '*' \
    --fail --silent --show-error --max-time 15 \
    https://api.ipify.org)" || return 1

  vpn_ip="$("${DC[@]}" --profile torrent \
    --profile vpn-test run --rm --no-deps vpn-probe)" ||
    return 1

  vpn_ip="${vpn_ip//$'\r'/}"
  vpn_ip="${vpn_ip//$'\n'/}"

  if [[ -z "$host_ip" || -z "$vpn_ip" ]]; then
    echo "Test IP incomplet" >&2
    return 1
  fi

  if [[ "$host_ip" == "$vpn_ip" ]]; then
    echo "VERIFICATION NON CONCLUANTE : meme IP de sortie" >&2
    return 1
  fi

  echo "OK : IP de sortie VPN distincte de celle de l'hote"
  echo "Kill switch valide : vpn-killswitch-test, vpn-killswitch-active-test"
}

vpn_dns_check() {
  if ! vpn_status >/dev/null; then
    echo "VPN non healthy : test refuse" >&2
    return 1
  fi

  local cid
  cid="$("${DC[@]}" --profile torrent ps -q gluetun)"

  echo "=== DNS depuis Gluetun (/etc/resolv.conf) ==="
  docker exec "$cid" cat /etc/resolv.conf

  echo ""
  echo "=== DNS depuis le conteneur reseau VPN (probe) ==="
  "${DC[@]}" --profile torrent --profile vpn-test \
    run --rm --no-deps vpn-probe \
    cat /etc/resolv.conf

  echo ""
  echo "=== Preuve iptables : port 53 externe bloque ==="
  local cid
  cid="$("${DC[@]}" --profile torrent ps -q gluetun)"
  if docker exec "$cid" iptables -L OUTPUT -n 2>/dev/null | \
      grep -qE "dpt:53|udp dpt:53|tcp dpt:53"; then
    echo "ATTENTION : regle OUTPUT autorisant le port 53 detectee" >&2
  else
    echo "OK : aucune regle OUTPUT n'autorise le port 53 vers des adresses externes"
    echo "     Le resolver 127.0.0.1 envoie les requetes DoT (port 853) via tun0."
  fi

  echo ""
  echo "Note : un nameserver 127.0.0.1 signifie que le DNS transite"
  echo "       par le resolver interne Gluetun (DNS-over-TLS)."
  echo "       Cela ne prouve pas l'absence de fuite DNS : seule"
  echo "       une capture reseau (tcpdump sur tun0) peut l'exclure."
}

vpn_firewall_check() {
  if ! vpn_status >/dev/null; then
    echo "VPN non healthy : test refuse" >&2
    return 1
  fi

  local cid
  cid="$("${DC[@]}" --profile torrent ps -q gluetun)"

  echo "=== iptables OUTPUT (IPv4) ==="
  docker exec "$cid" iptables -L OUTPUT -n -v || {
    echo "iptables indisponible dans le conteneur" >&2
    return 1
  }

  echo ""
  echo "=== ip6tables OUTPUT (IPv6) ==="
  docker exec "$cid" ip6tables -L OUTPUT -n -v 2>/dev/null || \
    echo "(ip6tables non disponible ou IPv6 desactive dans le conteneur)"

  echo ""
  echo "=== iptables INPUT (IPv4) ==="
  docker exec "$cid" iptables -L INPUT -n -v 2>/dev/null || \
    echo "(iptables INPUT indisponible)"

  echo ""
  echo "Note : l'absence de regle INPUT pour tun0 signifie que les pairs"
  echo "       ne peuvent pas initier de connexions entrantes vers le seeder."
  echo "       Seul le seeder peut initier les connexions (outbound-only)."
}

vpn_ipv6_check() {
  if ! vpn_status >/dev/null; then
    echo "VPN non healthy : test refuse" >&2
    return 1
  fi

  local output
  if output="$("${DC[@]}" --profile torrent --profile vpn-test \
      run --rm --no-deps vpn-probe \
      curl --ipv6 --noproxy '*' \
        --fail --silent --show-error \
        --max-time 10 \
        https://ipv6.icanhazip.com 2>&1)"; then
    echo "ATTENTION : adresse IPv6 publique obtenue : $output" >&2
    echo "ATTENTION : sortie IPv6 possible — verifier la configuration Gluetun" >&2
    return 1
  else
    echo "OK : connexion IPv6 sortante bloquee (echec attendu)"
    echo "Note : absence de sortie IPv6 prouvee par echec de connexion,"
    echo "       pas par inspection du trafic reseau."
  fi
}

vpn_killswitch_test() {
  # Isolated kill switch test: start a Gluetun container with deliberately
  # invalid VPN credentials. Gluetun applies its firewall DROP rules at
  # startup, before the VPN connection is established. This proves
  # fail-closed behavior: no traffic escapes when the VPN is not up.
  #
  # This test never touches the production Gluetun or any media data.

  if "${DC[@]}" --profile torrent ps --status running --services 2>/dev/null |
      grep -qx seeder; then
    echo "ERREUR : arreter le seeder avant le test kill switch" >&2
    return 1
  fi

  local tmpdir gluetun_name result=0

  tmpdir="$(mktemp -d)"
  gluetun_name="norylum-gluetun-ks-$$"

  echo "=== Test kill switch VPN (environnement isole) ==="
  echo "Gluetun demarre avec des identifiants invalides."
  echo "Le VPN ne s'etablira pas — le pare-feu doit rester en DROP."
  echo ""

  # Fake credentials — authentication will fail (intentional).
  printf '%s' "norylum-ks-invalid-user"     > "$tmpdir/openvpn_user"
  printf '%s' "norylum-ks-invalid-password" > "$tmpdir/openvpn_password"
  chmod 600 "$tmpdir/openvpn_user" "$tmpdir/openvpn_password"

  # Reuse cached server list so Gluetun does not try to download it
  # through its own firewall DROP rules.
  mkdir -p "$tmpdir/state"
  if [[ -f "$ROOT/vpn-state/servers.json" ]]; then
    cp "$ROOT/vpn-state/servers.json" "$tmpdir/state/servers.json"
  fi

  if ! docker run -d \
      --name "$gluetun_name" \
      --cap-add NET_ADMIN \
      --device /dev/net/tun \
      --security-opt no-new-privileges:true \
      -e VPN_SERVICE_PROVIDER=nordvpn \
      -e VPN_TYPE=openvpn \
      -e OPENVPN_PROTOCOL=udp \
      -e SERVER_COUNTRIES=Netherlands \
      -e VPN_PORT_FORWARDING=off \
      -v "$tmpdir:/run/secrets:ro" \
      -v "$tmpdir/state:/gluetun" \
      qmcgaw/gluetun:v3.41.3 >/dev/null; then
    echo "ERREUR : impossible de demarrer le conteneur Gluetun de test" >&2
    rm -rf "$tmpdir"
    return 1
  fi

  echo "Attente de l'initialisation Gluetun (15s)..."
  sleep 15

  echo ""
  echo "--- Journaux Gluetun de test ---"
  docker logs --tail=15 "$gluetun_name" 2>&1 || true

  echo ""
  echo "--- Regles iptables OUTPUT dans le conteneur de test ---"
  docker exec "$gluetun_name" iptables -L OUTPUT -n -v 2>/dev/null || \
    echo "(iptables non disponible dans le conteneur)"

  echo ""
  echo "--- Test connectivite IPv4 (attendu : echec) ---"
  if docker run --rm \
      --network "container:$gluetun_name" \
      --security-opt no-new-privileges:true \
      norylum-server:local \
      curl --ipv4 --noproxy '*' --fail --silent --show-error \
        --max-time 10 https://api.ipify.org 2>/dev/null; then
    echo "ECHEC : connexion IPv4 etablie — kill switch inoperant" >&2
    result=1
  else
    echo "OK : connexion IPv4 bloquee (kill switch actif)"
  fi

  echo ""
  echo "--- Test connectivite IPv6 (attendu : echec) ---"
  if docker run --rm \
      --network "container:$gluetun_name" \
      --security-opt no-new-privileges:true \
      norylum-server:local \
      curl --ipv6 --noproxy '*' --fail --silent --show-error \
        --max-time 10 https://ipv6.icanhazip.com 2>/dev/null; then
    echo "ECHEC : connexion IPv6 etablie — fuite potentielle" >&2
    result=1
  else
    echo "OK : connexion IPv6 bloquee"
  fi

  echo ""
  echo "--- Nettoyage ---"
  docker rm -f "$gluetun_name" >/dev/null 2>&1 || true
  rm -rf "$tmpdir"
  echo "Conteneur de test supprime."

  echo ""
  if (( result == 0 )); then
    echo "=== RESULTAT : fail-closed confirme ==="
    echo "Le pare-feu Gluetun bloque tout le trafic quand le VPN n'est pas etabli."
    echo ""
    echo "Limitation : ce test verifie l'absence de connexion au demarrage."
    echo "La coupure d'un tunnel actif (drop en cours de session) n'est pas couverte."
  else
    echo "=== RESULTAT : ECHEC — verifier la configuration Gluetun ===" >&2
  fi

  return "$result"
}

vpn_killswitch_active_test() {
  # Tests that the kill switch fires when an active VPN tunnel drops.
  # Uses real credentials in a fully isolated container; never touches
  # the production Gluetun, the seeder, or any media data.

  if [[ ! -f "$ROOT/vpn-secrets/openvpn_user" || \
        ! -f "$ROOT/vpn-secrets/openvpn_password" ]]; then
    echo "vpn-secrets manquants : test impossible" >&2
    return 1
  fi

  if "${DC[@]}" --profile torrent ps --status running --services 2>/dev/null |
      grep -qx seeder; then
    echo "ERREUR : arreter le seeder avant le test" >&2
    return 1
  fi

  local tmpdir gluetun_name result=0

  tmpdir="$(mktemp -d)"
  gluetun_name="norylum-gluetun-ks-active-$$"

  echo "=== Test kill switch sur tunnel VPN actif (env isole) ==="
  echo "Phase 1 : connexion NordVPN reelle, verification connectivite."
  echo "Phase 2 : coupure OpenVPN, verification blocage immediat."
  echo ""

  mkdir -p "$tmpdir/state"
  if [[ -f "$ROOT/vpn-state/servers.json" ]]; then
    cp "$ROOT/vpn-state/servers.json" "$tmpdir/state/servers.json"
  fi

  if ! docker run -d \
      --name "$gluetun_name" \
      --cap-add NET_ADMIN \
      --device /dev/net/tun \
      --security-opt no-new-privileges:true \
      -e VPN_SERVICE_PROVIDER=nordvpn \
      -e VPN_TYPE=openvpn \
      -e OPENVPN_PROTOCOL=udp \
      -e SERVER_COUNTRIES=Netherlands \
      -e VPN_PORT_FORWARDING=off \
      -v "$ROOT/vpn-secrets:/run/secrets:ro" \
      -v "$tmpdir/state:/gluetun" \
      qmcgaw/gluetun:v3.41.3 >/dev/null; then
    echo "ERREUR : impossible de demarrer le Gluetun de test" >&2
    rm -rf "$tmpdir"
    return 1
  fi

  echo "Attente connexion VPN (90s max)..."
  local i h healthy=0
  for ((i=0; i<45; i++)); do
    h="$(docker inspect \
      --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}' \
      "$gluetun_name" 2>/dev/null)" || break
    if [[ "$h" == "healthy" ]]; then
      healthy=1
      break
    fi
    sleep 2
  done

  if (( healthy == 0 )); then
    echo "ERREUR : Gluetun de test non healthy apres 90s" >&2
    docker logs --tail=20 "$gluetun_name" 2>&1 || true
    docker rm -f "$gluetun_name" >/dev/null 2>&1 || true
    rm -rf "$tmpdir"
    return 1
  fi

  echo "OK : Gluetun de test healthy (tunnel VPN etabli)"

  echo ""
  echo "--- Phase 1 : connectivite avec VPN actif ---"
  local vpn_ip
  if vpn_ip="$(docker run --rm \
      --network "container:$gluetun_name" \
      --security-opt no-new-privileges:true \
      norylum-server:local \
      curl --ipv4 --noproxy '*' --fail --silent --show-error \
        --max-time 15 https://api.ipify.org 2>/dev/null)"; then
    echo "OK : connectivite IPv4 via VPN (IP : $vpn_ip)"
  else
    echo "ECHEC : probe sans connectivite malgre Gluetun healthy" >&2
    docker rm -f "$gluetun_name" >/dev/null 2>&1 || true
    rm -rf "$tmpdir"
    return 1
  fi

  echo ""
  echo "--- Phase 2 : coupure OpenVPN ---"
  local ovpn_pid
  ovpn_pid="$(docker exec "$gluetun_name" \
    sh -c 'pgrep -x openvpn | head -1' 2>/dev/null)" || ovpn_pid=""

  if [[ -z "$ovpn_pid" ]]; then
    ovpn_pid="$(docker exec "$gluetun_name" \
      sh -c 'ps aux 2>/dev/null | awk "/[o]penvpn/{print \$1}" | head -1')" \
      || ovpn_pid=""
  fi

  if [[ -z "$ovpn_pid" ]]; then
    echo "ERREUR : PID OpenVPN introuvable dans le conteneur de test" >&2
    docker rm -f "$gluetun_name" >/dev/null 2>&1 || true
    rm -rf "$tmpdir"
    return 1
  fi

  echo "PID OpenVPN : $ovpn_pid — envoi SIGKILL..."
  docker exec "$gluetun_name" kill -9 "$ovpn_pid" 2>/dev/null || true

  echo "Test de connectivite immediat apres coupure..."
  if docker run --rm \
      --network "container:$gluetun_name" \
      --security-opt no-new-privileges:true \
      norylum-server:local \
      curl --ipv4 --noproxy '*' --fail --silent --show-error \
        --max-time 10 https://api.ipify.org 2>/dev/null; then
    echo "ECHEC : connexion IPv4 etablie apres coupure — kill switch inoperant" >&2
    result=1
  else
    echo "OK : connexion IPv4 bloquee apres coupure OpenVPN"
  fi

  echo ""
  echo "--- Nettoyage ---"
  docker rm -f "$gluetun_name" >/dev/null 2>&1 || true
  rm -rf "$tmpdir"
  echo "Conteneur de test supprime."

  echo ""
  if (( result == 0 )); then
    echo "=== RESULTAT : kill switch operationnel sur tunnel actif ==="
    echo "Le trafic est bloque immediatement apres la coupure OpenVPN."
  else
    echo "=== RESULTAT : ECHEC ===" >&2
  fi

  return "$result"
}

vpn_start() {
  # Arreter l'ancien seeder avant toute operation Gluetun.
  "${DC[@]}" --profile torrent stop seeder \
    >/dev/null 2>&1 || true

  vpn_connect || return 1

  # Bloquer le seeder si la sortie VPN n'est pas verifiee.
  if ! vpn_egress_check; then
    echo "ERREUR : verification VPN echouee, seeder bloque" >&2
    return 1
  fi

  "${DC[@]}" --profile torrent up -d seeder
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

  vpn_start || return 1

  torrent_api POST     "/v1/torrent/media/$id/enable"     '{"confirm_distribution_rights":true}'
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

  if "${DC[@]}" --profile maintenance run \
      --rm --no-deps \
      --entrypoint /usr/local/bin/norylum-torrent-cleanup \
      maintenance --media-id "$id"; then

    printf "Supprimer uniquement ce snapshot (ecrire SUPPRIMER) : "
    local answer
    IFS= read -r answer

    if [[ "$answer" == "SUPPRIMER" ]]; then
      if ! "${DC[@]}" --profile maintenance run \
          --rm --no-deps \
          --entrypoint /usr/local/bin/norylum-torrent-cleanup \
          maintenance --media-id "$id" --apply; then
        result=1
      fi
    else
      echo "Nettoyage annule."
    fi
  else
    result=1
  fi

  if (( running == 1 )); then
    vpn_start || result=1
  fi

  return "$result"
}

torrent_logs() {
  "${DC[@]}" --profile torrent logs --tail=100 seeder
}

acquisition_run() {
  require_config
  if ! [[ -d vpn-secrets ]]; then
    echo "ERREUR : vpn-secrets absent — VPN non configure" >&2
    exit 1
  fi
  local gluetun_cid
  gluetun_cid="$("${DC[@]}" --profile acquisition ps -q gluetun 2>/dev/null)" || gluetun_cid=""
  if [[ -z "$gluetun_cid" ]]; then
    echo "ERREUR : Gluetun non demarre. Lancer vpn-connect ou vpn-start d'abord." >&2
    exit 1
  fi
  local health
  health="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}' "$gluetun_cid" 2>/dev/null)" || health="unknown"
  if [[ "$health" != "healthy" ]]; then
    echo "ERREUR : Gluetun $health — attendre healthy avant de lancer une acquisition." >&2
    exit 1
  fi
  echo "Lancement d'un cycle d'acquisition sous VPN..."
  "${DC[@]}" --profile acquisition run --rm acquisition-worker
}

acquisition_logs() {
  "${DC[@]}" --profile acquisition logs --tail=200 acquisition-worker 2>/dev/null || \
    echo "INFO : aucun journal acquisition-worker disponible"
}

acquisition_status() {
  echo "=== Demandes d'acquisition ==="
  torrent_api GET /v1/acquisition/requests
  echo ""
  echo "=== Conteneur acquisition-worker ==="
  "${DC[@]}" --profile acquisition ps acquisition-worker 2>/dev/null || \
    echo "INFO : service acquisition-worker non demarre"
}

web_start() {
  require_config
  echo "Construction et demarrage de l'interface web..."
  "${DC[@]}" --profile web up -d --build norylum-web
  echo "Interface disponible sur http://127.0.0.1:3000"
}

web_stop() {
  "${DC[@]}" --profile web stop norylum-web
}

web_logs() {
  "${DC[@]}" --profile web logs -f --tail=100 norylum-web
}

case "${1:-}" in
  setup) setup ;;
  start)
    if [[ ! -f .env ]]; then echo 'Executer setup avant start' >&2; exit 1; fi
    "${DC[@]}" --profile torrent stop seeder >/dev/null 2>&1 || true
    "${DC[@]}" up -d --build server
    echo "Seeder arrete : utiliser vpn-start apres configuration."
    "${DC[@]}" ps
    ;;
  stop) "${DC[@]}" --profile torrent down ;;
  update)
    require_config
    require_clean_dev
    backup_db
    "${DC[@]}" --profile torrent stop seeder >/dev/null 2>&1 || true
    git pull --ff-only origin dev
    "${DC[@]}" build --pull
    "${DC[@]}" up -d --no-deps --force-recreate server
    wait_healthy
    echo "Seeder arrete par securite apres mise a jour."
    echo "Utiliser vpn-start pour le relancer via Gluetun."
    "${DC[@]}" ps
    ;;
  backup) require_config; backup_db ;;
  doctor) scripts/norylum-doctor.sh ;;
  restart-check) restart_check ;;
  logs) "${DC[@]}" logs -f --tail=100 server ;;
  status) "${DC[@]}" ps; echo "URL locale : http://127.0.0.1:$(sed -n 's/^NORYLUM_PORT=//p' .env | head -1)" ;;
  scan) scan ;;
  jobs) api_call /v1/library/ingest/jobs ;;
  vpn-connect) vpn_connect ;;
  vpn-egress-check) vpn_egress_check ;;
  vpn-dns-check) vpn_dns_check ;;
  vpn-firewall-check) vpn_firewall_check ;;
  vpn-ipv6-check) vpn_ipv6_check ;;
  vpn-killswitch-test) vpn_killswitch_test ;;
  vpn-killswitch-active-test) vpn_killswitch_active_test ;;
  vpn-start) vpn_start ;;
  vpn-status) vpn_status ;;
  vpn-stop) vpn_stop ;;
  acquisition-requests) torrent_api GET /v1/acquisition/requests ;;
  acquisition-run) acquisition_run ;;
  acquisition-logs) acquisition_logs ;;
  acquisition-status) acquisition_status ;;
  torrent-enable) torrent_enable "${2:-}" ;;
  torrent-limit) torrent_limit "${2:-}" ;;
  torrent-disable) torrent_disable "${2:-}" ;;
  torrent-status) torrent_status ;;
  torrent-cleanup) torrent_cleanup "${2:-}" ;;
  torrent-logs) torrent_logs ;;
  ingest) ingest "${2:-}" ;;
  web-start) web_start ;;
  web-stop) web_stop ;;
  web-logs) web_logs ;;
  *) usage; exit 2 ;;
esac
