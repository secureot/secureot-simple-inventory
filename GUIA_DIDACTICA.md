# Inventario de red en Rust: cómo funciona por dentro

Estas notas acompañan al programa `mac_fabricante_pcap.rs`. La idea no es solo que entiendas el
código, sino que de paso saques en limpio unas cuantas cosas de redes, porque el programa toca
varias y bastante concretas.

Lo que hace, en criollo: agarra tramas Ethernet (de un archivo `.pcap` o escuchando en vivo una
interfaz), les saca las direcciones MAC, averigua de qué fabricante es cada una y cuenta cuántas
veces aparece. Lo que queda es una especie de censo de los equipos que se ven en la red.

El recorrido va de lo simple a lo que tiene más vuelo. Si solo te interesa la parte de redes,
con las secciones 2, 3 y 4 ya tenés la película completa. Lo de la sección 5 es para que el
programa aguante en producción y lo podés saltear sin culpa.


## 1. Qué se aprende leyéndolo

Conceptos de red que van apareciendo solos:

- Cómo es una trama Ethernet y en qué parte están las MAC.
- Qué es una dirección MAC y por qué sus primeros 3 bytes te dicen el fabricante.
- La diferencia entre unicast, multicast y broadcast, que se decide mirando un único bit.
- Qué quiere decir "capturar" tráfico: leer un `.pcap` contra escuchar en vivo, y qué es el modo
  promiscuo.
- Por qué el tipo de enlace de una captura importa y no es un detalle.

Y de yapa, patrones de Rust: `struct` y `enum`, `Option` y `Result`, `HashMap`, iteradores, el
operador `?` para errores, y `clap` para la línea de comandos.


## 2. Lo justo de teoría de redes

### La trama Ethernet

Cuando dos equipos se hablan en una LAN, la unidad de capa 2 es la trama. Lo que ve un capturador,
byte por byte, arranca así:

```
 Offset:  0        6        12   14                     ... fin
          +--------+--------+----+----------------------+
 Campo:   | MAC    | MAC    |Ether|     Payload          |
          | DESTINO| ORIGEN |Type |  (IP, ARP, etc.)     |
          +--------+--------+----+----------------------+
           6 bytes  6 bytes  2 bytes      46 a 1500 bytes
```

A este programa le alcanza con los primeros 12 bytes: 6 de la MAC destino y 6 de la origen. Todo
lo que viene después (el payload con IP, puertos, datos) lo ignora. Eso es justamente lo que lo
mantiene simple: vive en capa 2 y nada más.

Dos detalles que conviene saber. El preámbulo y el checksum (FCS) de la trama no aparecen en lo
capturado, libpcap ya los saca. Y si la trama trae etiqueta VLAN 802.1Q, esa etiqueta va después
de las dos MAC, así que los offsets 0 a 11 siguen siendo válidos. Por eso el código no necesita
hacer nada especial con VLAN.

### La MAC y el fabricante (OUI)

Una MAC son 6 bytes, casi siempre escritos `aa:bb:cc:dd:ee:ff`. No es un número al azar, viene
partida en dos mitades:

```
   aa : bb : cc : dd : ee : ff
   └──────┬─────┘  └─────┬─────┘
      OUI (3 bytes)   lo asigna
   = fabricante       el fabricante
```

Los primeros 3 bytes son el OUI, un bloque que la IEEE le da a cada fabricante. Hay una base
pública (la sacamos de maclookup.app) que traduce cada OUI a un nombre: `00:00:0C` es Cisco,
`B8:27:EB` es Raspberry Pi, `28:63:36` es Siemens. Con solo mirar la MAC ya tenés una idea de qué
clase de aparato es.

Algunos bloques se subdividen entre varios fabricantes usando más bits (prefijos de 28 o 36), y el
programa lo contempla. Pero para agarrar la idea alcanza con pensar "3 bytes igual fabricante".

### Unicast, multicast, broadcast: lo decide un bit

El bit más bajo del primer byte de la MAC destino dice si la trama va a un equipo o a varios:

```
 primer byte = 0x01  ->  0000 0001
                                 ^ este bit en 1 = multicast o broadcast
```

En 0 es unicast, un destinatario concreto, o sea un dispositivo real. En 1 es multicast, un grupo,
y el caso extremo `ff:ff:ff:ff:ff:ff` es broadcast, o sea todos.

Esto pesa en el inventario. Una MAC de broadcast o multicast no es un equipo físico, así que por
defecto el programa la deja afuera. Y una MAC multicast como origen directamente no tiene sentido
(nadie nace multicast), suele ser señal de tráfico raro o falsificado.

Hay otro bit interesante al lado, el de "administrada localmente". Cuando está en 1, la MAC la puso
el software y no viene quemada de fábrica, que es lo típico de la aleatorización de MAC de los
celulares de hoy. El programa lo deja anotado como dato.

### Capturar: archivo o en vivo

Hay dos maneras de alimentar al programa. Una es un archivo PCAP, una captura ya hecha con
Wireshark o tcpdump. Se lee de punta a punta, es reproducible y no pide permisos especiales, ideal
para aprender. La otra es en vivo: el programa escucha una interfaz como `eth0` en tiempo real. Para
ver todo el tráfico y no solo el que va dirigido a tu máquina hace falta el modo promiscuo, y eso sí
necesita privilegios.

Las dos se apoyan en libpcap, la librería de captura de toda la vida (la misma de Wireshark y
tcpdump). En Rust la usamos con el crate `pcap`.


## 3. El programa de un saque

En pseudocódigo, el centro es esto:

```
cargar base OUI (un mapa: 3 bytes -> fabricante)
abrir la captura (archivo o interfaz)
por cada paquete:
    sacar MAC destino (bytes 0..6) y MAC origen (bytes 6..12)
    si no es multicast, sumar 1 a su contador
al terminar:
    por cada MAC vista, buscar el fabricante
    ordenar e imprimir o guardar
```

El resto del código (rendimiento, multi-hilo, formatos de salida) son anillos alrededor de ese
centro.


## 4. El código por dentro

### Las dependencias

Cada crate hace una cosa y se combinan: `clap` para los argumentos, `pcap` para capturar, `serde`
para leer el JSON del OUI y escribir CSV o JSON, `rustc_hash` para un HashMap rápido, `anyhow` para
que el manejo de errores no sea un dolor. Esa composición de piezas chicas es muy del estilo Rust.

### Cómo guardamos una MAC

Una decisión que vale la pena mirar: la MAC no se guarda como texto `"aa:bb:cc:..."` sino como sus
6 bytes crudos, el tipo `[u8; 6]`. Es exactamente lo que viene en el paquete, sin convertir nada.

Para comparar rápido, a veces conviene verla como un solo número de 48 bits:

```rust
fn mac48(mac: &[u8; 6]) -> u64 {
    ((mac[0] as u64) << 40) | ((mac[1] as u64) << 32) | ... | (mac[5] as u64)
}
```

Eso apila los 6 bytes en un `u64` corriendo cada uno a su lugar con `<<`. Comparar dos MAC pasa a
ser comparar dos números, más simple y más veloz que comparar cadenas. Es un buen ejemplo de que en
redes se piensa en bits, no en strings.

### Los argumentos

```rust
#[derive(Parser)]
struct Args {
    #[arg(short, long, required_unless_present("iface"))]
    pcap: Option<String>,         // --pcap archivo.pcap
    #[arg(short, long, ...)]
    iface: Option<String>,        // --iface eth0
    #[arg(short = 'j', long, default_value = "mac-vendors-export.json")]
    oui_db: String,
    // ... salida, filtro, etc.
}
```

Con `#[derive(Parser)]`, `clap` arma todo el parseo a partir de la struct: cada campo se vuelve una
opción `--campo`, un `Option<String>` es opcional, y las reglas como `required_unless_present` o
`conflicts_with` expresan "dame `--pcap` o `--iface`, pero no los dos" sin que escribas un solo `if`
de validación.

### La base de fabricantes

El JSON de maclookup.app es una lista de objetos `{ "macPrefix": ..., "vendorName": ... }`, y
`serde` los lee directo a esta struct:

```rust
#[derive(Deserialize)]
struct RawManufacturer {
    #[serde(rename = "macPrefix")]  prefix: String,
    #[serde(rename = "vendorName")] name:   String,
}
```

`OuiDb::load` arma un mapa de los 3 primeros bytes hacia el fabricante. Y `lookup` toma una MAC y
devuelve el nombre, o `None` si no lo conoce:

```rust
fn lookup(&self, mac: &[u8; 6]) -> Option<&str> {
    let key = [mac[0], mac[1], mac[2]];   // los 3 bytes del OUI
    let entries = self.map.get(&key)?;    // ¿hay fabricante para este OUI?
    // ... elegir la coincidencia y devolver el nombre
}
```

Si recién arrancás, leelo como un simple `mapa[primeros_3_bytes]`. El código hace algo un poco más
fino para los OUI subdivididos (elige el prefijo más específico que matchee), pero la idea de fondo
es esa.

### El inventario y el corazón del programa

El inventario es un mapa de MAC a contadores:

```rust
struct DeviceStats { src_count: u64, dst_count: u64 }   // veces como origen / destino
type Inventory = FxHashMap<[u8; 6], DeviceStats>;
```

Y esta es la función más importante de todas, la que procesa un paquete:

```rust
fn handle_packet(data: &[u8], inventory: &mut Inventory, include_multicast: bool) {
    if data.len() < 14 { return; }              // ni siquiera entra la cabecera Ethernet

    let dst: [u8; 6] = data[0..6].try_into().unwrap();   // MAC destino = bytes 0..6
    let src: [u8; 6] = data[6..12].try_into().unwrap();  // MAC origen  = bytes 6..12

    if include_multicast || !is_multicast(&src) {
        inventory.entry(src).or_default().src_count += 1;
    }
    if include_multicast || !is_multicast(&dst) {
        inventory.entry(dst).or_default().dst_count += 1;
    }
}
```

Acá está toda la teoría de la sección 2 hecha código. `data[0..6]` y `data[6..12]` son literalmente
las dos MAC de la trama. `is_multicast` mira ese bit (`mac[0] & 0x01`) para descartar broadcast y
multicast. Y `inventory.entry(src).or_default().src_count += 1` es el patrón clásico de "buscá esta
MAC; si no está creala en cero; sumale uno", que es como contás sin perderte ninguna aparición.

Fijate que acá no se busca el fabricante, solo se cuentan bytes. El fabricante se resuelve una vez
por MAC, al final, y en la sección 5 explico por qué.

Las dos ayudas de un bit son tan cortas como esperás:

```rust
fn is_multicast(mac: &[u8; 6]) -> bool            { mac[0] & 0x01 != 0 }
fn is_locally_administered(mac: &[u8; 6]) -> bool { mac[0] & 0x02 != 0 }
```

### El bucle de captura

Leer paquetes es un bucle de toda la vida: pedí el próximo, procesalo, repetí.

```rust
loop {
    if stop.load(Ordering::Relaxed) { break; }   // ¿nos pidieron parar? (Ctrl-C o --duration)
    match cap.next_packet() {
        Ok(packet) => handle_packet(packet.data, inventory, include_multicast),
        Err(pcap::Error::NoMorePackets) => break,    // se acabó el archivo
        Err(pcap::Error::TimeoutExpired) => continue, // en vivo: no llegó nada, seguimos
        Err(e) => { eprintln!("Error: {e}"); break; }
    }
}
```

El mismo bucle sirve para archivo y para captura en vivo, porque en Rust la función es genérica
sobre los dos tipos de captura. Con un archivo, `next_packet` avisa `NoMorePackets` al llegar al
final. En vivo, devuelve `TimeoutExpired` cuando pasó un ratito sin tráfico, y ahí aprovechamos para
chequear si nos pidieron terminar.

### La salida

Al final, `build_rows` recorre el inventario y recién ahí, una vez por cada MAC, llama a `lookup`
para ponerle el nombre del fabricante, calcula el total y arma una fila ordenada por apariciones.
Después se imprime una tabla, o se guarda como CSV o JSON. El crate `csv` se encarga de poner
comillas si un nombre de fabricante tiene comas, como "Cisco Systems, Inc", que es un detalle fácil
de pasar por alto.

### main, atando todo

```
1. cargar la base OUI
2. preparar un flag de stop compartido
3. enganchar Ctrl-C  -> pone el flag en true (así no perdemos lo capturado)
4. si hay --duration, lanzar un hilo que pone el flag en true a los N segundos
5. según --pcap o --iface, abrir la captura y correr el bucle
6. armar las filas (acá se resuelven los fabricantes) y mostrar o guardar
```

El flag de stop es un `AtomicBool` compartido (`Arc<AtomicBool>`), una variable booleana que varias
partes del programa pueden leer y escribir sin pisarse. Tanto Ctrl-C como `--duration` lo único que
hacen es poner `stop = true`, y el bucle lo ve y termina ordenado, escribiendo el inventario. Es un
patrón de coordinación bien común y vale la pena tenerlo claro.


## 5. La parte que podés saltear

Todo lo de abajo está para que la herramienta sea rápida y robusta en serio. Para entender la red no
hace falta, con las secciones 2 a 4 ya está.

Por qué buscar el fabricante al final y no en cada paquete. Buscar el fabricante cuesta. En una
captura puede haber millones de paquetes pero apenas cientos de equipos distintos. Si lo buscáramos
en cada paquete repetiríamos el mismo trabajo millones de veces al pedo. Contando primero y
resolviendo al final, la búsqueda se hace una sola vez por MAC. Misma respuesta, una fracción del
trabajo.

Por qué `FxHashMap` y no el `HashMap` común. El de la librería estándar usa un hash resistente a
ataques, que es más lento. Como acá las claves son MAC de nuestra propia red y no hay un adversario
eligiéndolas para hacernos colisionar, usamos `FxHashMap`, que es más simple y rápido. Pensalo como
un HashMap, pero apurado.

Por qué validamos el tipo de enlace. El programa da por sentado que la captura es Ethernet, con las
MAC en los bytes 0 a 11. Pero hay capturas que no lo son. Por ejemplo `tcpdump -i any` genera otro
formato (LINUX_SLL) donde esos bytes no son las MAC. La función `check_datalink` lo detecta y
rechaza la captura con un mensaje claro, en vez de escupir un inventario mal en silencio. La moraleja
sirve para cualquier cosa: validá tus supuestos sobre el formato de los datos.

El reporte de paquetes perdidos. Capturando en vivo, si llega tráfico más rápido de lo que podemos
procesar, el sistema descarta paquetes. Al terminar, el programa dice cuántos recibió y cuántos se
perdieron. En una auditoría eso es clave, porque te dice si el inventario está completo o le falta
gente.

El multi-hilo con PACKET_FANOUT. Para enlaces muy rápidos (pensá 10 Gbit/s) un solo hilo puede
quedarse corto. La opción `--threads N` abre N capturas en paralelo y le pide al kernel que reparta
el tráfico entre ellas. Cada hilo lleva su propio inventario, así no hay que sincronizar nada en el
camino caliente, y al final se fusionan. Es la parte más enroscada del código y la que menos tiene
que ver con aprender redes, es pura optimización. Para cualquier uso didáctico dejá `--threads 1`,
que es el valor por defecto, y olvidate de esto.


## 6. Cómo se compila

El programa usa Cargo, el gestor de proyectos de Rust. Con un comando baja las dependencias,
compila y te deja el ejecutable.

### Lo que necesitás

Dos cosas instaladas. Primero Rust con Cargo; si no lo tenés, va con rustup:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Y segundo libpcap, la librería de captura a la que el crate `pcap` se enlaza. En Debian o Ubuntu:

```bash
sudo apt install libpcap-dev
```

(En Fedora o RHEL es `libpcap-devel`; en macOS ya viene con el sistema.)

### Armar el proyecto

Cargo espera el código en `src/main.rs`. El archivo que tenés, `mac_fabricante_pcap.rs`, es ese
archivo principal, solo hay que ponerlo en su lugar:

```bash
cargo new network_inventory
cd network_inventory
cp /ruta/a/mac_fabricante_pcap.rs src/main.rs
```

Después reemplazá el `Cargo.toml` que generó Cargo por este, que declara las dependencias:

```toml
[package]
name = "network_inventory"
version = "0.2.0"
edition = "2021"

[dependencies]
clap = { version = "=4.4.18", features = ["derive"] }
pcap = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
rustc-hash = "1.1"
csv = "1.3"
ctrlc = "=3.4.4"
libc = "0.2"

[profile.release]      # optimizaciones, opcionales pero recomendadas para captura
opt-level = 3
lto = "thin"
codegen-units = 1
panic = "abort"
```

Las versiones de `clap` y `ctrlc` están fijadas a propósito para que compile también en toolchains
viejos (lo probé con Rust 1.75). Con un Rust al día (`rustup update`) anda igual y no tenés que tocar
nada.

### Compilar y probar

```bash
cargo build --release
```

La primera vez tarda un poco porque baja y compila las dependencias. El ejecutable queda en
`target/release/network_inventory`.

Lo del `--release` no es decorativo. Sin esa bandera, Cargo hace una compilación de desarrollo, que
compila rápido pero corre lento. Con `--release` optimiza, que es lo que querés para capturar
tráfico de verdad.

Para ver que quedó bien:

```bash
./target/release/network_inventory --help   # la ayuda con todas las opciones
cargo test                                  # los tests incluidos, si querés
```

Si ves la ayuda con `--pcap`, `--iface` y compañía, ya está listo para la sección 7.

### El archivo de fabricantes

Para que el inventario pueda ponerle nombre a cada MAC hace falta la base OUI en JSON. Se baja
gratis desde la página de maclookup.app:

```
https://maclookup.app/downloads/json-database
```

Entrá y tocá "Download JSON database" (son unos 6 MB, alrededor de 58.000 prefijos, y la actualizan
seguido). Guardalo como `mac-vendors-export.json` al lado del ejecutable, o pasale la ruta con
`--oui-db`. El formato es justo el que el programa lee; cada entrada se ve así:

```json
{ "macPrefix": "00:00:00", "vendorName": "XEROX CORPORATION",
  "private": false, "blockType": "MA-L", "lastUpdate": "2015/11/17" }
```

Ojo con una cosa: el botón de descarga genera un enlace con un token que va cambiando, así que lo
más práctico es bajar el archivo desde el navegador en vez de con un `curl` fijo.


## 7. Probalo vos

La mejor forma de que esto quede es correrlo. Algunas ideas:

1. Conseguí un PCAP de juguete con unas pocas tramas, o capturá diez segundos de tu red con
   `sudo tcpdump -i eth0 -w prueba.pcap` y cortá con Ctrl-C.
2. Corré el inventario sobre ese archivo:
   ```bash
   ./network_inventory --pcap prueba.pcap --oui-db mac-vendors-export.json
   ```
3. Buscá tu router. Suele ser la MAC con más apariciones; mirá qué fabricante le sale.
4. Mirá el broadcast: volvé a correr con `--include-multicast` y vas a ver aparecer
   `ff:ff:ff:ff:ff:ff`, el "todos" de la red.
5. Cazá MACs aleatorizadas: si exportás a CSV con `--output inv.csv`, la columna
   `locally_administered` te marca los equipos (casi siempre celulares) que esconden su MAC real.

Cada experimento engancha una línea de código con algo que pasa de verdad en tu red.


## 8. Glosario corto

Trama Ethernet: la unidad de datos de capa 2; empieza con MAC destino más MAC origen.
MAC: dirección física de 6 bytes de una interfaz de red.
OUI: los primeros 3 bytes de la MAC, que identifican al fabricante.
Unicast, multicast, broadcast: a uno, a un grupo, a todos; se distinguen por un bit.
PCAP: el formato de archivo (y la librería, libpcap) para capturas de tráfico.
Modo promiscuo: hacer que la interfaz te entregue todo el tráfico, no solo el tuyo.
Datalink o LINKTYPE: el tipo de enlace de una captura (Ethernet, SLL, RAW).
Inventario: acá, el mapa de MAC a fabricante y conteos que produce el programa.


## 9. Fuentes para seguir aprendiendo, desde cero

Esto es un proyecto para aprender, así que acá van las fuentes de donde salen los conceptos que
usa. Todas son gratis y las revisé antes de ponerlas: existen, están vigentes y son las que se
suele recomendar de verdad, no un relleno genérico. Van de lo más elemental a lo más específico del
código.

### Redes, desde el principio

- **Beej's Guide to Network Concepts** (https://beej.us/guide/bgnet0/): arranca de cero: qué es
  una red, capas, Ethernet, IP, puertos, todo con Python como vehículo. Es la puerta de entrada si
  nunca tocaste el tema. Del mismo autor, cuando quieras pasar a programar sockets en C, está
  **Beej's Guide to Network Programming** (https://beej.us/guide/bgnet/), un clásico de los 90 que
  se sigue actualizando.
- **Wikipedia: Organizationally unique identifier**
  (https://en.wikipedia.org/wiki/Organizationally_unique_identifier): el resumen más accesible de
  cómo se arma una MAC, qué son los bits U/L e I/G, y cómo el OUI se volvió MA-L/MA-M/MA-S. Buen
  complemento a la sección 2.2 de esta guía.

### La fuente oficial de las MAC y el OUI

- **IEEE Registration Authority, listado público**
  (https://regauth.standards.ieee.org/standards-ra-web/pub/view.html): de acá sale, en última
  instancia, la base `mac-vendors-export.json` que usa el programa. maclookup.app la arma tomando
  estos mismos datos y los deja en un JSON cómodo de leer; esta es la fuente primaria si alguna vez
  querés verificar una asignación puntual o entender la diferencia entre MA-L, MA-M y MA-S
  directamente de quien los asigna.

### El formato PCAP y libpcap

- **"Programming with pcap" de Tim Carstens** (https://www.tcpdump.org/pcap.html): el tutorial de
  referencia para programar con libpcap en C: abrir una captura, poner un filtro, el callback por
  paquete. El código de este proyecto en Rust es, conceptualmente, lo mismo que este tutorial pero
  con el crate `pcap` haciendo de intermediario.
  En la misma página de tcpdump.org hay una lista completa de tutoriales y papers, incluida la
  guía de Julia Evans "Let's learn tcpdump!" para el lado de usar la herramienta en vez de programarla.
- **`pcap(3PCAP)` man page** (https://www.tcpdump.org/manpages/pcap.3pcap.html): la referencia
  formal de las funciones de libpcap (`pcap_open_offline`, `pcap_next`, `pcap_datalink`, etc.). El
  crate de Rust que usamos es un envoltorio fino sobre estas mismas funciones, así que esta página
  explica el comportamiento real por debajo.
- **Lista de LINKTYPEs** (https://www.tcpdump.org/linktypes.html): la tabla oficial de qué
  significa cada valor de datalink (Ethernet, LINUX_SLL, RAW, etc.). Es la referencia exacta detrás
  de `check_datalink` en el código.

### Rust

- **The Rust Programming Language** ("el libro", https://doc.rust-lang.org/book/): el libro
  oficial y gratuito. Cubre todo lo que usa este proyecto: ownership, `struct`/`enum`, `Option`/
  `Result`, closures, hilos. Viene instalado localmente con `rustup doc --book`.
  Si preferís aprender viendo código en vez de leyendo texto corrido, la alternativa es
  **Rust by Example** (enlazada desde https://www.rust-lang.org/learn), con ejemplos ejecutables.
  Y si querés practicar escribiendo, **Rustlings** (también desde esa misma página) son ejercicios
  cortos con el compilador guiándote.

### El crate `pcap` de Rust específicamente

- **Documentación del crate** (https://docs.rs/pcap): la referencia de la API que usa
  `main.rs`: `Capture`, `Device`, `Activated`, el patrón `from_file`/`from_device` seguido de
  `.open()`. Cuando el código llama algo como `cap.next_packet()` o `cap.get_datalink()`, es acá
  donde está documentado qué devuelve cada uno.
  El repositorio (https://github.com/rust-pcap/pcap) tiene además ejemplos corriendo que sirven
  para comparar con las funciones `run_capture`/`run_live_fanout` del proyecto.

Con esto y la guía completa (secciones 1 a 8) tenés de dónde sale cada pieza: la teoría de redes,
el formato de los datos, el lenguaje, y la librería puntual que se usa para leerlos.

