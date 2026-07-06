#!/bin/sh
# Pipeline compartido: pcap -> CSV (+ XLSX).
# Se usa como entrypoint del contenedor Docker y también en ejecución nativa.
# Resuelve las herramientas por variables de entorno, con fallbacks razonables.
set -eu

# binario network_inventory
if [ -n "${NI_BIN:-}" ]; then
    BIN="$NI_BIN"
elif command -v network_inventory >/dev/null 2>&1; then
    BIN="network_inventory"
elif [ -x "./target/release/network_inventory" ]; then
    BIN="./target/release/network_inventory"
else
    echo "no encuentro 'network_inventory' (instalalo, o seteá NI_BIN=/ruta)" >&2
    exit 1
fi

# conversor a Excel (opcional; requiere python3)
XLSX_TOOL=""
if command -v python3 >/dev/null 2>&1; then
    if [ -n "${NI_XLSX:-}" ]; then
        XLSX_TOOL="$NI_XLSX"
    elif command -v inventario_a_excel.py >/dev/null 2>&1; then
        XLSX_TOOL="$(command -v inventario_a_excel.py)"
    elif [ -f "./inventario_a_excel.py" ]; then
        XLSX_TOOL="./inventario_a_excel.py"
    fi
fi

# base OUI
if [ -n "${OUI_DB:-}" ]; then
    OUI="$OUI_DB"
elif [ -f "/opt/network-inventory/mac-vendors-export.json" ]; then
    OUI="/opt/network-inventory/mac-vendors-export.json"
elif [ -f "./mac-vendors-export.json" ]; then
    OUI="./mac-vendors-export.json"
else
    OUI="mac-vendors-export.json"   # que la herramienta se queje si no está
fi

if [ "$#" -eq 0 ]; then
    echo "uso: <pcap> [nombre_base]    (o directamente flags de network_inventory)" >&2
    exit 1
fi

# modo avanzado: si el primer arg es una opción, pasamos todo crudo
case "$1" in
    -*) exec "$BIN" "$@" ;;
esac

PCAP="$1"
[ -f "$PCAP" ] || { echo "no encuentro '$PCAP'" >&2; exit 1; }
BASE="${2:-$(basename "$PCAP")}"
BASE="${BASE%.*}"

EXTRA=""
[ "${INCLUDE_MULTICAST:-0}" = "1" ] && EXTRA="--include-multicast"

echo "==> $PCAP"
# shellcheck disable=SC2086
"$BIN" --pcap "$PCAP" --oui-db "$OUI" $EXTRA --output "$BASE.csv" --quiet

if [ "${NO_XLSX:-0}" = "1" ] || [ -z "$XLSX_TOOL" ]; then
    [ -z "$XLSX_TOOL" ] && [ "${NO_XLSX:-0}" != "1" ] && \
        echo "(sin conversor a Excel disponible: dejo solo el CSV)" >&2
    echo "ok: $BASE.csv"
else
    python3 "$XLSX_TOOL" "$BASE.csv" -o "$BASE.xlsx"
    echo "ok: $BASE.csv + $BASE.xlsx"
fi
