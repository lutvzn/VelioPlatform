#!/bin/sh
# Runs Nuvio's own smoke tests (ported from its self-host repository,
# Apache-2.0) against a running Velio server, to check that a Nuvio client
# can use it instead of Nuvio's.
#
#   SUPABASE_PUBLIC_URL  the server, such as http://127.0.0.1:8787
#   DATABASE_URL         its database, for the tests' checks and clean-up
#   DEFAULT_CATALOG_URL  optional; Nuvio's catalog add-on by default
#
# The server must confirm sign-ups at once (ENABLE_EMAIL_AUTOCONFIRM, on by
# default), as the tests sign in straight after signing up.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
: "${SUPABASE_PUBLIC_URL:?Set SUPABASE_PUBLIC_URL to the server}"
: "${DATABASE_URL:?Set DATABASE_URL to the server's database}"
SUPABASE_PUBLIC_URL=${SUPABASE_PUBLIC_URL%/}
ANON_KEY=${ANON_KEY:-$(curl --fail -sS "$SUPABASE_PUBLIC_URL/.well-known/nuvio" | jq -er .publishable_key)}
DEFAULT_CATALOG_URL=${DEFAULT_CATALOG_URL:-https://catalog.nuvio.tv/manifest.json}
export SUPABASE_PUBLIC_URL ANON_KEY DEFAULT_CATALOG_URL DATABASE_URL

for test in discovery signup security workflows; do
    sh "$here/smoke-$test.sh"
done
