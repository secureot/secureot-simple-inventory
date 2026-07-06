#!/usr/bin/env bash
# Instala la herramienta nativamente: compila el binario y deja todo en el PATH.
# Contraparte "CLI" del build de Docker.
#   sudo ./install.sh
# Requiere: cargo, libpcap-dev (sudo apt install libpcap-dev), y python3 para el Excel.
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Compilando el binario Rust"
( cd "$here" && cargo build --release )

echo "==> Instalando en $PREFIX/bin"
install -m 0755 "$here/target/release/network_inventory" "$PREFIX/bin/network_inventory"
install -m 0755 "$here/inventario_a_excel.py"            "$PREFIX/bin/inventario_a_excel.py"
install -m 0755 "$here/bin/pipeline.sh"                  "$PREFIX/bin/pipeline.sh"
install -m 0755 "$here/bin/ni"                           "$PREFIX/bin/ni"

echo "==> Base OUI en /opt/network-inventory"
install -d /opt/network-inventory
if [ -f "$here/mac-vendors-export.json" ]; then
    install -m 0644 "$here/mac-vendors-export.json" /opt/network-inventory/mac-vendors-export.json
else
    echo "   falta mac-vendors-export.json; bajala y ponela en /opt/network-inventory/ (ver OUI_DB.md)"
fi

if command -v python3 >/dev/null 2>&1; then
    echo "==> Instalando openpyxl (para el Excel)"
    python3 -m pip install --quiet openpyxl 2>/dev/null \
        || python3 -m pip install --quiet --break-system-packages openpyxl 2>/dev/null \
        || echo "   no pude instalar openpyxl solo; si querés el .xlsx, instalalo a mano"
fi

echo "Listo. Probá:  ni tu-captura.pcap"
