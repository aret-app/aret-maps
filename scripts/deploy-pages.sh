#!/usr/bin/env bash
# Publishes the pilot-region tile folder (out/tiles/) to Cloudflare Pages — ADR-0005 — and, when
# built, the route catalogue (out/routes/: tiles + index.json, ADR-0015).
#
# Usage:  scripts/deploy-pages.sh
# Needs:  - out/tiles/ from:  scripts/build-pilot.sh files
#         - optional out/routes/ from:  scripts/build-routes.sh  (a deploy replaces everything,
#           so without it the routes disappear from the site)
#         - Node.js (runs Wrangler through npx at a pinned version)
#         - on your computer: `npx wrangler login` once
#           in CI: CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID environment variables
# Settings: .env (git-ignored), see .env.example

set -euo pipefail
cd "$(dirname "$0")/.."

# Private settings stay in .env, never in this script
if [[ -f .env ]]; then set -a; source .env; set +a; fi
PROJECT="${CF_PAGES_PROJECT:-aret-tiles}"
WRANGLER_VERSION="4.162.0"   # pinned so everyone (and CI) deploys with the same tool
[[ -n "${CLOUDFLARE_ACCOUNT_ID:-}" ]] || unset CLOUDFLARE_ACCOUNT_ID

[[ -d out/tiles ]] || { echo "out/tiles/ not found: run  scripts/build-pilot.sh files  first"; exit 1; }

echo "==> Preparing out/site/"
rm -rf out/site
mkdir -p out/site
cp site/_headers out/site/_headers
cp -R out/tiles out/site/tiles
if [[ -d out/routes/tiles ]]; then
  mkdir -p out/site/routes
  cp -R out/routes/tiles/. out/site/routes/
  cp out/routes/index.json out/site/routes/index.json
  echo "    routes: $(find out/site/routes -name '*.pbf' | wc -l | tr -d ' ') tiles + index.json"
else
  echo "    routes: not built (scripts/build-routes.sh), not published"
fi

# Cloudflare Pages limits (ADR-0005): 20,000 files and 25 MB per file per deployment
count=$(find out/site -type f | wc -l | tr -d ' ')
too_big=$(find out/site -type f -size +25M | wc -l | tr -d ' ')
echo "    files: $count   size: $(du -sh out/site | cut -f1)"
(( count <= 20000 )) || { echo "!!  $count files exceed Cloudflare Pages' 20,000-file limit" >&2; exit 1; }
(( too_big == 0 ))   || { echo "!!  $too_big files are over 25 MB" >&2; exit 1; }

echo "==> Deploying to Cloudflare Pages project '$PROJECT' (production)"
npx --yes "wrangler@${WRANGLER_VERSION}" pages deploy out/site \
  --project-name="$PROJECT" --branch=main --commit-dirty=true

echo
echo "Tiles:    https://${PROJECT}.pages.dev/tiles/{z}/{x}/{y}.pbf"
echo "Metadata: https://${PROJECT}.pages.dev/tiles/metadata.json"
if [[ -d out/site/routes ]]; then
  echo "Routes:   https://${PROJECT}.pages.dev/routes/{z}/{x}/{y}.pbf   (layer \"routes\")"
  echo "Index:    https://${PROJECT}.pages.dev/routes/index.json"
fi
