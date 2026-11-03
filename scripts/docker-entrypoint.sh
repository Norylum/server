#!/bin/sh
set -eu

# Read secrets only in the container; Compose and docker inspect expose file
# paths rather than the actual credential values.
if [ -f /run/norylum-config/tmdb_token ]; then
  NORYLUM_TMDB_READ_TOKEN="$(cat /run/norylum-config/tmdb_token)"
  export NORYLUM_TMDB_READ_TOKEN
fi
if [ -f /run/norylum-config/admin_password ]; then
  : "${NORYLUM_ADMIN_USERNAME:?Username missing from .env}"
  NORYLUM_ADMIN_PASSWORD="$(cat /run/norylum-config/admin_password)"
  export NORYLUM_ADMIN_PASSWORD
else
  # Existing database can start without the original bootstrap secret.
  unset NORYLUM_ADMIN_USERNAME NORYLUM_ADMIN_PASSWORD
fi

exec "$@"
