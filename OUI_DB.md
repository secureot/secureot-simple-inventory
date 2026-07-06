# Base de fabricantes (OUI)

No va incluida en el paquete porque se actualiza seguido. Bajala de:

    https://maclookup.app/downloads/json-database

(botón "Download JSON database"; el enlace tiene un token que rota, así que conviene bajarla del
navegador y no con un curl a URL fija).

Guardala como `mac-vendors-export.json` según el caso:

- Instalación nativa: dejala en la raíz del proyecto antes de correr `./install.sh`; queda copiada a
  `/opt/network-inventory/mac-vendors-export.json`.
- Imagen Docker: dejala en la raíz del proyecto antes de buildear; el Dockerfile la buildea.
- Si ya está corriendo: la herramienta la busca en `/opt/network-inventory/`, o le pasás `--oui-db`
  (o la variable `OUI_DB`).

## Ejemplo de como se ve

```json
[
  {
    "macPrefix": "00:00:0C",
    "vendorName": "Cisco Systems, Inc",
    "private": false,
    "blockType": "MA-L",
    "lastUpdate": "2015/11/17"
  },
  {
    "macPrefix": "00:00:0D",
    "vendorName": "FIBRONICS LTD.",
    "private": false,
    "blockType": "MA-L",
    "lastUpdate": "2015/11/17"
  },
  {
    "macPrefix": "00:00:0E",
    "vendorName": "FUJITSU LIMITED",
    "private": false,
    "blockType": "MA-L",
    "lastUpdate": "2018/10/13"
  },
```
