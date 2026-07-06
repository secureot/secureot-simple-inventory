# Inventario de red en Rust

Herramienta para armar un inventario de dispositivos de red a partir de archivos PCAP o de captura
en vivo. Identifica cada equipo por su dirección MAC y le asocia el fabricante usando una base OUI en
JSON (por ejemplo la de [maclookup.app](https://maclookup.app/downloads/json-database)).

> v0.2.0: reescritura orientada a rendimiento y archivos grandes. Ver [Cambios](#cambios-respecto-a-v01).

## Qué hace

- Lee archivos `.pcap` o captura en vivo desde una interfaz.
- Saca las MAC de origen y destino de cada trama Ethernet (incluidas las que llevan tag VLAN 802.1Q).
- Resuelve el fabricante con coincidencia de prefijo más largo (soporta bloques MA-L /24, MA-M /28 y MA-S /36).
- Lleva conteos separados de origen y destino por dispositivo único.
- Filtra multicast/broadcast por defecto (configurable) y los marca cuando se incluyen.
- Marca las MAC administradas localmente, típicas de la aleatorización.
- Exporta a CSV o JSON reales, o imprime una tabla por stdout.
- Captura en vivo con corte limpio por Ctrl-C y filtro BPF opcional.

## Requisitos

- Rust y Cargo (probado con 1.75; instalá desde [rustup.rs](https://rustup.rs)).
- libpcap:

  ```bash
  sudo apt install libpcap-dev
  ```

- La base OUI en JSON: se baja de https://maclookup.app/downloads/json-database con el botón
  "Download JSON database".

## Compilar

```bash
cargo build --release
```

El binario queda en `target/release/network_inventory`.

## Un solo comando: `ni`

`bin/ni` unifica las dos formas de correr todo el pipeline (pcap -> CSV -> Excel): nativo o dentro de
Docker, con el mismo comando. Por defecto elige solo: usa el binario nativo si está instalado, y si
no cae a la imagen Docker.

```bash
ni captura.pcap             # auto: nativo si está, si no Docker
ni --docker captura.pcap    # forzar Docker
ni --native captura.pcap    # forzar nativo
```

Deja `captura.csv` y `captura.xlsx` en la carpeta actual. Acepta las mismas variables que el pipeline
(`INCLUDE_MULTICAST=1`, `NO_XLSX=1`, `OUI_DB=...`), y si el primer argumento empieza con `-`, pasa los
flags crudos a `network_inventory` (modo avanzado).

Para tenerlo disponible como comando, hay dos caminos de despliegue:

- Nativo: `sudo ./install.sh` compila el binario y deja `network_inventory`, `inventario_a_excel.py`,
  `pipeline.sh` y `ni` en el PATH, más la base OUI en `/opt/network-inventory/`. Requiere `cargo`,
  `libpcap-dev` y `python3`.
- Docker: `docker build -f docker/Dockerfile -t network-inventory .` (desde la raíz). Después
  `ni` en modo auto o `--docker` usa esa imagen.

## Uso

```
Uso: network_inventory [OPCIONES]

Opciones:
  -p, --pcap <PCAP>          Archivo PCAP de entrada
  -i, --iface <IFACE>        Interfaz para captura en vivo
  -j, --oui-db <OUI_DB>      Base OUI JSON [por defecto: mac-vendors-export.json]
  -o, --output <OUTPUT>      Archivo de salida (si se omite, imprime tabla)
  -f, --format <FORMAT>      Formato de salida: csv | json [por defecto: csv]
      --filter <FILTER>      Filtro BPF (ej: "ip", "vlan")
      --include-multicast    Incluir MAC multicast/broadcast en el inventario
  -d, --duration <N>         (En vivo) Capturar N segundos y salir (útil para timers)
      --snaplen <N>          (En vivo) Bytes capturados por trama [por defecto: 96]
      --buffer-size <N>      (En vivo) Buffer del kernel en bytes [por defecto: 4194304]
  -t, --threads <N>          (En vivo) Hilos de captura; >1 activa PACKET_FANOUT [por defecto: 1]
      --fanout <MODE>        (En vivo, >1 hilo) Reparto del kernel: hash | cpu | lb [por defecto: hash]
  -q, --quiet                Silenciar el progreso por stderr
```

Un aviso importante sobre la captura: la herramienta lee las MAC de la cabecera Ethernet. Un PCAP
tomado con `tcpdump -i any` (LINUX_SLL) o sin capa 2 (RAW) se rechaza con un mensaje claro, porque
ahí los offsets de la MAC no son válidos. Capturá siempre sobre una interfaz Ethernet concreta como
`eth0`, no sobre `any`.

### Ejemplos

```bash
# Analizar un PCAP y mostrar la tabla
./target/release/network_inventory --pcap captura.pcap

# Exportar a CSV
./target/release/network_inventory --pcap captura.pcap --output inventario.csv

# Exportar a JSON
./target/release/network_inventory --pcap captura.pcap -o inventario.json -f json

# Captura en vivo (Ctrl-C para terminar e imprimir el inventario)
sudo ./target/release/network_inventory --iface eth0

# Sin sudo: otorgar solo la capability necesaria
sudo setcap cap_net_raw,cap_net_admin+eip ./target/release/network_inventory
./target/release/network_inventory --iface eth0

# Solo tráfico IP, con otra base OUI
./target/release/network_inventory --pcap red.pcap --filter ip --oui-db vendors.json
```

## Salida

Tabla por stdout, ordenada por total de apariciones:

```
----- Inventario de Dispositivos (3 únicos) -----
MAC                Vendor                              Src    Dst  Total
b8:27:eb:22:22:22  Raspberry Pi Foundation               1      4      5
00:00:0c:11:11:11  Cisco Systems, Inc                    3      1      4
28:63:36:33:33:33  Siemens AG                            2      0      2
```

CSV (los nombres con comas quedan bien comillados):

```csv
mac,vendor,src_count,dst_count,total,multicast,locally_administered
00:00:0c:11:11:11,"Cisco Systems, Inc",3,1,4,false,false
```

## Reporte en Excel y Power BI

`inventario_a_excel.py` toma el CSV o el JSON de salida y arma un Excel con colores, filtros y hojas
de resumen (por fabricante, y totales con un par de gráficos):

```bash
python3 inventario_a_excel.py inventario.csv -o reporte.xlsx   # requiere openpyxl
```

Para Power BI conviene usar directo el CSV (Obtener datos -> Texto/CSV), que ya trae todos los
valores; si tenés la captura corriendo por systemd y reescribiendo el CSV, Power BI lo puede
recargar.

## Docker

Si no querés instalar nada nativo, corré todo en un contenedor. La imagen empaqueta el binario, el
conversor a Excel y el pipeline; el entrypoint es el mismo `bin/pipeline.sh` que se usa nativo, así
que se comporta igual.

```bash
# construir (desde la raíz del repo)
docker build -f docker/Dockerfile -t network-inventory .

# usar (el lanzador arma el docker run por vos)
ni --docker captura.pcap
```

Los detalles (base OUI, cómo cambiarla sin rebuildear) están en `docker/README.md`.

## Por qué escala a archivos grandes

- Lookup diferido: durante la captura no se busca el fabricante, solo se cuentan MAC. La resolución
  OUI ocurre una vez por dispositivo único, al final. En una captura de millones de paquetes con
  cientos de dispositivos, eso son cientos de lookups en vez de millones.
- Sin asignaciones en el camino caliente: las MAC se manejan como `[u8; 6]` y los OUI como enteros.
  No se formatean strings ni se clonan nombres de fabricante por paquete.
- Memoria acotada por dispositivos, no por paquetes: el inventario crece con la cantidad de MAC
  únicas (el tamaño de la red), no con la cantidad de tramas.
- Hashing rápido (`FxHashMap`) para el mapa de inventario.

## Captura a alta velocidad (SPAN 10G)

`--threads`/`--fanout` usan `PACKET_FANOUT`, una API de Linux (AF_PACKET). En macOS o BSD el
binario compila igual (todo lo demás anda normal), pero pedir `--threads` mayor a 1 ahí da un
error claro en vez de fallar a medias; en esas plataformas usá `--threads 1` (el default).

Primero el encuadre. "10 Gbit/s" a tramas de 64 bytes son ~14,88 Mpps: ningún capturador userspace
sobre libpcap sostiene eso, y para inventario OT tampoco es el objetivo. En un SPAN 10G con
Modbus/S7comm el pps medio está muy por debajo de línea; el riesgo real son las microráfagas. La meta
es absorber ráfagas con buffer, mantener barato el trabajo por paquete, y repartir en varias colas
para tener margen. AF_PACKET con `PACKET_FANOUT` alcanza; DPDK o AF_XDP son innecesarios acá.

### 1) Preparación de SO/NIC (la palanca más grande, sin tocar la herramienta)

```bash
# Desactivar coalescing: GRO/LRO juntan paquetes y distorsionan lo capturado (verías super-tramas).
sudo ethtool -K eth0 gro off lro off tso off gso off

# Agrandar los ring buffers de la NIC para tolerar ráfagas.
sudo ethtool -G eth0 rx 4096

# Ver descartes a nivel driver/NIC.
ethtool -S eth0 | grep -iE "drop|miss|fifo|nobuf"
cat /proc/net/dev
```

Si el SPAN llega por varias colas RSS, asigná IRQs a CPUs dedicadas y dejá esos cores libres para la
captura.

### 2) Ajustes dentro de la herramienta

```bash
# Snaplen mínimo (solo hace falta la cabecera L2) + buffer grande + varios hilos con fanout.
sudo ./target/release/network_inventory \
  --iface eth0 --threads 4 --fanout hash \
  --snaplen 64 --buffer-size 67108864
```

- `--snaplen 64`: copia solo ~64 bytes por trama en vez de la trama entera, o sea menos trabajo
  kernel->usuario en enlaces rápidos. Para este inventario sobra (las MAC están en los primeros 12 bytes).
- `--buffer-size`: el ring del kernel por socket. Más grande, más microráfaga absorbida.
- `--threads N` con `--fanout`: abre N sockets AF_PACKET en un grupo `PACKET_FANOUT`; el kernel
  reparte el tráfico y cada hilo procesa su parte con un inventario local (sin locks en el camino
  caliente), y al final se fusionan. `hash` mantiene cada flujo en el mismo socket, `cpu` reparte por
  CPU de llegada, `lb` es round-robin.
- Guard: con `--threads 1` no se usa fanout. Un socket único dentro de un grupo fanout puede
  interferir con la entrega en loopback y algunos contenedores.

### 3) Medir si alcanza

Tras la captura, la herramienta imprime `recibidos / descartados(buffer) / descartados(iface)` (vía
`pcap_stats`) y avisa si hubo descartes. Si `descartados(buffer) > 0`, subí `--buffer-size` o
`--threads`; si `descartados(iface) > 0`, el cuello está en la NIC o el driver (mirá `ethtool -S` y
`ethtool -G`).

### Script de arranque

`scripts/capture-10g.sh` autodetecta las colas RSS, aplica el tuning de NIC y lanza la captura con
tantos hilos como colas (mínimo 2):

```bash
sudo ./scripts/capture-10g.sh eth0 mac-vendors-export.json inventario.csv
# Override: THREADS=8 BUFFER_SIZE=134217728 sudo ./scripts/capture-10g.sh eth0
```

## Despliegue como servicio (systemd)

Para correr sin root, con capabilities mínimas (`CAP_NET_RAW` y `CAP_NET_ADMIN`) y una unidad
endurecida. Los archivos están en `systemd/`.

```bash
# 1) Binario y usuario sin privilegios
sudo install -m 0755 target/release/network_inventory /usr/local/bin/
sudo useradd --system --no-create-home --shell /usr/sbin/nologin netcap

# 2) Estado + base OUI
sudo install -d -o netcap -g netcap /var/lib/network-inventory
# Descargá el JSON desde https://maclookup.app/downloads/json-database (el enlace lleva un token que
# rota, así que conviene bajarlo del navegador) y dejalo en su lugar:
sudo install -o netcap -g netcap mac-vendors-export.json /var/lib/network-inventory/

# 3) Configuración y unidades
sudo install -m 0644 systemd/network-inventory.env.example /etc/default/network-inventory
sudo cp systemd/network-inventory-tune@.service systemd/network-inventory@.service /etc/systemd/system/
sudo systemctl daemon-reload

# 4) Arrancar la captura en eth0 (arrastra el tuning como dependencia)
sudo systemctl start network-inventory@eth0.service

# 5) Detener -> SIGINT -> vuelca el inventario a /var/lib/network-inventory/inventario-eth0.csv
sudo systemctl stop network-inventory@eth0.service
```

Un par de notas:

- El servicio escribe el inventario al detenerse. La unidad usa `KillSignal=SIGINT` para que
  `systemctl stop` dispare el corte limpio; con `SIGTERM` (el default) abortaría sin volcar el archivo.
- Ajustá `THREADS`, `BUFFER_SIZE`, etc. en `/etc/default/network-inventory`.
- Si la captura no arranca por el sandbox, ampliá `RestrictAddressFamilies` (por ejemplo agregando
  `AF_INET`) en la unidad.

## Notas técnicas

- Las MAC de origen multicast se descartan por defecto (origen inválido o spoofeado). Las de
  broadcast/multicast como destino también, salvo `--include-multicast`, porque no representan
  dispositivos físicos. Cuando se incluyen, quedan marcadas.
- La coincidencia de prefijo más largo distingue sub-asignaciones MA-M/MA-S dentro de un MA-L
  genérico. Los prefijos se indexan por sus primeros 3 bytes y se ordenan por longitud.
- El bit locally administered (`mac[0] & 0x02`) se reporta para ayudar a detectar MAC aleatorizadas.
- Las MAC van antes del tag VLAN 802.1Q, así que los offsets 0 a 11 valen también en tramas con tag.
- Solo Ethernet: al abrir se valida el datalink (`get_datalink`). Capturas LINUX_SLL o RAW se rechazan
  en vez de producir un inventario mal en silencio.
- Reporte de descartes: tras una captura en vivo se imprimen recibidos y descartados vía `pcap_stats`.
  Si hubo descartes conviene subir `--buffer-size` o acotar con `--filter`; es lo relevante en un SPAN
  de OT, donde la idea es tolerar ráfagas sin perder paquetes.

## Tests

```bash
cargo test
```

Cubren el parseo de prefijos MA-L/MA-M, la coincidencia de prefijo más largo, la detección de
multicast y el filtrado de origen multicast.

## Cambios respecto a v0.1

- Lookup de fabricante diferido al final (antes: uno por paquete).
- MAC como `[u8; 6]` y OUI como enteros; cero asignaciones de string por paquete.
- `FxHashMap` para el inventario.
- Conteos separados de origen y destino (antes: mezclados).
- Filtrado de multicast/broadcast (antes: contados como dispositivos).
- Coincidencia de prefijo más largo MA-L/MA-M/MA-S (antes: solo /24).
- Exportación real a CSV/JSON (antes: solo stdout, sin comillado).
- `process_offline` y `process_live` unificadas en una función genérica.
- Captura en vivo con corte limpio por Ctrl-C y filtro BPF opcional.
- Manejo de errores con `anyhow` (antes: `unwrap`/`process::exit` dispersos).
- Validación de datalink: rechaza capturas no-Ethernet (LINUX_SLL/RAW) en vez de generar basura.
- Reporte de paquetes descartados (`pcap_stats`) tras captura en vivo.
- `--snaplen` y `--buffer-size` para tunear la captura en vivo.
- `--threads` con `--fanout`: captura multi-socket con `PACKET_FANOUT` e inventarios por hilo
  fusionados al final (escalado hacia 10G, sin DPDK).
- `--duration` para capturas acotadas por tiempo.
- `--pcap` y `--iface` ahora son mutuamente excluyentes (antes: uno se ignoraba en silencio).
