# Imagen Docker

Empaqueta el binario Rust, el conversor a Excel y el pipeline en una sola imagen. La forma cómoda de
usarla es con `bin/ni` (ver el README principal); acá va lo específico de la imagen.

## Construir

Desde la raíz del repo (no desde `docker/`):

```bash
docker build -f docker/Dockerfile -t network-inventory .
```

Antes de buildear, dejá `mac-vendors-export.json` en la raíz (ver `OUI_DB.md`). Queda horneado en la
imagen.

## Correr

Con el lanzador:

```bash
ni --docker captura.pcap     # o simplemente 'ni' si no tenés el binario nativo instalado
```

A mano, es lo que hace `ni` por dentro:

```bash
docker run --rm --user "$(id -u):$(id -g)" \
    -v "$PWD:/data" -w /data \
    network-inventory captura.pcap
```

El entrypoint de la imagen es `bin/pipeline.sh`, el mismo script que corre en modo nativo. Por eso el
comportamiento es idéntico en las dos vías: mismos modos, mismas variables
(`INCLUDE_MULTICAST`, `NO_XLSX`, `OUI_DB`) y el mismo modo avanzado pasando flags crudos.

## Cambiar la base OUI sin rebuildear

```bash
docker run --rm --user "$(id -u):$(id -g)" \
    -v "$PWD:/data" -w /data \
    -e OUI_DB=/data/mac-vendors-export.json \
    network-inventory captura.pcap
```

## Nota

Procesa archivos pcap, no captura en vivo. Para captura en vivo dentro de un contenedor harían falta
`--network host` y `--cap-add NET_RAW NET_ADMIN`, y no es la idea acá.
