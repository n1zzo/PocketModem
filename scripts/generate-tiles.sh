#!/bin/bash
# generate-tiles.sh - Generate MBTiles for offline map
#
# Usage:
#   ./generate-tiles.sh <region> [max_zoom]
#
# Examples:
#   ./generate-tiles.sh germany 14
#   ./generate-tiles.sh italy 12
#   ./generate-tiles.sh north-america 10
#
# Requires: tippecanoe (https://github.com/mapbox/tippecanoe)
#
# Install tippecanoe:
#   macOS: brew install tippecanoe
#   Debian/Ubuntu: apt install tippecanoe
#   Alpine: apk add tippecanoe

set -e

REGION=${1:-germany}
MAX_ZOOM=${2:-14}
OUTPUT_FILE="${REGION}.mbtiles"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo_step() {
    echo -e "${GREEN}[1/4]${NC} $1"
}

echo_info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

echo_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check for tippecanoe
if ! command -v tippecanoe &> /dev/null; then
    echo_error "tippecanoe not found!"
    echo "Please install it first:"
    echo "  macOS: brew install tippecanoe"
    echo "  Debian/Ubuntu: apt install tippecanoe"
    exit 1
fi

# Check for wget
if ! command -v wget &> /dev/null; then
    echo_error "wget not found! Please install it."
    exit 1
fi

OSM_FILE="${REGION}.osm.pbf"

echo_step "Generating offline tiles for region: ${REGION}"
echo_info "Max zoom: ${MAX_ZOOM}"
echo_info "Output: ${OUTPUT_FILE}"
echo ""

# Download OSM extract if not present
if [ ! -f "$OSM_FILE" ]; then
    echo_step "Downloading OSM extract..."
    echo_info "URL: https://download.geofabrik.de/${REGION}-latest.osm.pbf"
    
    wget -q --show-progress \
        "https://download.geofabrik.de/${REGION}-latest.osm.pbf" \
        -O "$OSM_FILE"
    
    echo_info "Download complete: $(ls -lh $OSM_FILE | awk '{print $5}')"
else
    echo_info "Using existing file: $OSM_FILE"
fi

echo ""

# Generate MBTiles with vector tiles
echo_step "Generating MBTiles (zoom 0-${MAX_ZOOM})..."

tippecanoe \
    --output="${OUTPUT_FILE}" \
    --layer=default \
    --maximum-zoom=${MAX_ZOOM} \
    --minimum-zoom=0 \
    --extend-zooms-if-still-dropping \
    --drop-densest-as-needed \
    --detect-shared-borders \
    --force \
    "$OSM_FILE"

# Show result
echo ""
echo_step "Done!"
echo_info "Generated: ${OUTPUT_FILE}"
echo_info "Size: $(ls -lh ${OUTPUT_FILE} | awk '{print $5}')"
echo ""
echo "Copy to device and select in Settings > Map > Offline Tiles"
echo ""
echo "Available regions:"
echo "  europe:      $(ls -lh germany.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"
echo "  italy:       $(ls -lh italy.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"
echo "  france:      $(ls -lh france.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"
echo "  spain:       $(ls -lh spain.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"
echo "  europe:      $(ls -lh europe.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"
echo "  north-america: $(ls -lh north-america.mbtiles 2>/dev/null | awk '{print $5}' || echo 'not generated')"