# Construir desde la RAÍZ del repo:
#   docker build -f docker/Dockerfile -t network-inventory .

# Etapa de compilación: genera el binario Rust.
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libpcap-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml ./
# COPY Cargo.lock ./        # descomentar si tenés lockfile, para builds reproducibles
COPY src ./src
RUN cargo build --release


# Imagen final, chica: solo lo necesario para correr.
FROM python:3.12-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends libpcap0.8 \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir openpyxl

COPY --from=builder /src/target/release/network_inventory /usr/local/bin/network_inventory
COPY inventario_a_excel.py /usr/local/bin/inventario_a_excel.py
COPY bin/pipeline.sh       /usr/local/bin/pipeline.sh
# base OUI horneada en la imagen (bajala antes de buildear, ver OUI_DB.md)
COPY mac-vendors-export.json /opt/network-inventory/mac-vendors-export.json

RUN chmod +x /usr/local/bin/inventario_a_excel.py /usr/local/bin/pipeline.sh

WORKDIR /data
ENTRYPOINT ["/usr/local/bin/pipeline.sh"]
