#!/usr/bin/env bash
#
# capture-10g.sh: prepara una NIC para captura a alta tasa y lanza network_inventory
# con tantos hilos (PACKET_FANOUT) como colas RSS tenga la interfaz.
#
# Uso:
#   sudo ./capture-10g.sh <iface> [oui_db] [output]
#
# Variables de entorno (override opcional):
#   THREADS      Forzar nº de hilos (default: nº de colas RSS, mínimo 2)
#   SNAPLEN      Bytes por trama (default: 64; solo se necesita la cabecera L2)
#   BUFFER_SIZE  Buffer del kernel por socket, en bytes (default: 67108864 = 64 MiB)
#   FANOUT       hash | cpu | lb (default: hash)
#   RX_RING      Tamaño del ring RX de la NIC (default: max soportado)
#   BIN          Ruta al binario (default: ./target/release/network_inventory o en PATH)
#
set -euo pipefail

err() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
warn() { printf 'aviso: %s\n' "$*" >&2; }
info() { printf '==> %s\n' "$*" >&2; }

IFACE="${1:-}"
OUI_DB="${2:-mac-vendors-export.json}"
OUTPUT="${3:-}"
[ -n "$IFACE" ] || err "Falta la interfaz. Uso: sudo $0 <iface> [oui_db] [output]"

# Localizar el binario.
BIN="${BIN:-}"
if [ -z "$BIN" ]; then
  if [ -x "./target/release/network_inventory" ]; then
    BIN="./target/release/network_inventory"
  elif command -v network_inventory >/dev/null 2>&1; then
    BIN="$(command -v network_inventory)"
  else
    err "No encuentro network_inventory. Definí BIN=/ruta/al/binario."
  fi
fi

command -v ethtool >/dev/null 2>&1 || err "ethtool no está instalado (apt install ethtool)."
[ -e "/sys/class/net/$IFACE" ] || err "La interfaz '$IFACE' no existe."

# --- Detectar colas RSS (Combined) para dimensionar los hilos ---
detect_queues() {
  local q
  q="$(ethtool -l "$IFACE" 2>/dev/null \
        | awk '/^Current hardware settings:/{f=1} f&&/^Combined:/{print $2; exit}')"
  [ -n "$q" ] && [ "$q" -gt 0 ] 2>/dev/null && { echo "$q"; return; }
  nproc
}

NCPU="$(nproc)"
QUEUES="$(detect_queues)"
THREADS="${THREADS:-$QUEUES}"
# No tiene sentido más hilos que CPUs; y para activar fanout queremos al menos 2.
[ "$THREADS" -gt "$NCPU" ] && THREADS="$NCPU"
[ "$THREADS" -lt 2 ] && THREADS=2

SNAPLEN="${SNAPLEN:-64}"
BUFFER_SIZE="${BUFFER_SIZE:-67108864}"
FANOUT="${FANOUT:-hash}"

# --- Tuning de la NIC (best-effort; algunas NICs no soportan todos los toggles) ---
info "Desactivando coalescing (gro/lro/gso/tso) en $IFACE: evita super-tramas en la captura"
for feat in gro lro gso tso; do
  ethtool -K "$IFACE" "$feat" off >/dev/null 2>&1 || warn "no se pudo desactivar $feat (puede no estar soportado)"
done

info "Agrandando el ring RX de $IFACE"
RX_MAX="$(ethtool -g "$IFACE" 2>/dev/null \
          | awk '/^Pre-set maximums:/{f=1} f&&/^RX:/{print $2; exit}')"
RX_RING="${RX_RING:-$RX_MAX}"
if [ -n "${RX_RING:-}" ] && [ "$RX_RING" -gt 0 ] 2>/dev/null; then
  ethtool -G "$IFACE" rx "$RX_RING" >/dev/null 2>&1 || warn "no se pudo fijar rx ring a $RX_RING"
else
  warn "no pude leer el máximo de RX ring; dejo el valor actual"
fi

info "Interfaz=$IFACE  colasRSS=$QUEUES  CPUs=$NCPU  hilos=$THREADS  fanout=$FANOUT  snaplen=$SNAPLEN  buffer=$BUFFER_SIZE"

# --- Lanzar la captura ---
ARGS=(--iface "$IFACE" --oui-db "$OUI_DB"
      --threads "$THREADS" --fanout "$FANOUT"
      --snaplen "$SNAPLEN" --buffer-size "$BUFFER_SIZE")
[ -n "$OUTPUT" ] && ARGS+=(--output "$OUTPUT")

info "Ejecutando: $BIN ${ARGS[*]}"
info "Ctrl-C para terminar e imprimir/escribir el inventario."
exec "$BIN" "${ARGS[@]}"
