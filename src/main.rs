//! Inventario de dispositivos a partir de tráfico Ethernet.
//! Lee las MAC de cada trama (de un .pcap o capturando en vivo), las cuenta, y recién
//! al final le pone nombre de fabricante a cada una usando la base OUI
//! duurante la captura solo se sacan y cuentan MAC; el lookup del fabricante se hace una vez por MAC, al cerrar.

use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use pcap::{Activated, Capture, Linktype};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// Argumentos de línea de comandos.
#[derive(Parser)]
#[command(author, version, about = "Inventario de red por MAC/fabricante desde PCAP o sniffing en vivo")]
struct Args {
    /// Archivo PCAP de entrada.
    #[arg(short, long, required_unless_present("iface"))]
    pcap: Option<String>,

    /// Interfaz de red para captura en vivo.
    #[arg(short, long, required_unless_present("pcap"), conflicts_with("pcap"))]
    iface: Option<String>,

    /// Base de datos OUI en JSON (https://maclookup.app/downloads/json-database).
    #[arg(short = 'j', long, default_value = "mac-vendors-export.json")]
    oui_db: String,

    /// Archivo de salida. Si se omite, imprime una tabla por stdout.
    #[arg(short, long)]
    output: Option<String>,

    /// Formato del archivo de salida.
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Csv)]
    format: OutputFormat,

    /// Filtro BPF opcional (ej: "ip" o "vlan").
    #[arg(long)]
    filter: Option<String>,

    /// Incluir MACs multicast/broadcast en el inventario.
    #[arg(long, default_value_t = false)]
    include_multicast: bool,

    /// Silenciar el progreso por stderr.
    #[arg(short, long, default_value_t = false)]
    quiet: bool,

    /// (Solo en vivo) Bytes capturados por trama. Solo se necesita la cabecera L2;
    /// un valor bajo reduce la copia kernel->usuario en enlaces rápidos.
    #[arg(long, default_value_t = 96)]
    snaplen: i32,

    /// (Solo en vivo) Tamaño del buffer de captura del kernel en bytes.
    /// Más buffer = más tolerancia a ráfagas sin descartar paquetes.
    #[arg(long, default_value_t = 4_194_304)]
    buffer_size: i32,

    /// (Solo en vivo) Hilos de captura. >1 activa PACKET_FANOUT (un socket AF_PACKET por
    /// hilo, balanceado por el kernel). Recomendado para enlaces 10G. Default 1 (sin fanout).
    #[arg(short = 't', long, default_value_t = 1)]
    threads: usize,

    /// (Solo en vivo, con --threads >1) Estrategia de reparto del kernel entre sockets.
    #[arg(long, value_enum, default_value_t = FanoutMode::Hash)]
    fanout: FanoutMode,

    /// (Solo en vivo) Capturar durante N segundos y luego escribir el inventario y salir.
    /// Útil para capturas periódicas vía systemd timer. Si se omite, corre hasta Ctrl-C/SIGTERM.
    #[arg(short = 'd', long)]
    duration: Option<u64>,
}

#[derive(Copy, Clone, ValueEnum)]
enum FanoutMode {
    /// Por hash de flujo: mantiene cada flujo en el mismo socket (sin reordenamiento).
    Hash,
    /// Por CPU de llegada.
    Cpu,
    /// Round-robin (load balance).
    Lb,
}

#[derive(Copy, Clone, ValueEnum)]
enum OutputFormat {
    Csv,
    Json,
}

// OUI: prefijo de 3 bytes -> fabricante

#[derive(Deserialize)]
struct RawManufacturer {
    #[serde(rename = "macPrefix")]
    mac_prefix: String,
    #[serde(rename = "vendorName")]
    vendor_name: String,
}

/// Una entrada OUI: prefijo alineado a 48 bits y cantidad de bits significativos.
struct OuiEntry {
    /// Valor del prefijo, ocupando los `bits` superiores de un número de 48 bits.
    prefix: u64,
    bits: u32,
    vendor_idx: u32,
}

/// Índice OUI agrupado por los primeros 3 bytes (todos los prefijos tienen >= 24 bits).
struct OuiDb {
    map: FxHashMap<[u8; 3], Vec<OuiEntry>>,
    vendors: Vec<String>,
}

impl OuiDb {
    fn load(path: &str) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("No se pudo abrir {path}"))?;
        let raw: Vec<RawManufacturer> = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("No se pudo parsear el JSON {path}"))?;

        let mut map: FxHashMap<[u8; 3], Vec<OuiEntry>> = FxHashMap::default();
        let mut vendors: Vec<String> = Vec::with_capacity(raw.len());
        let mut skipped = 0u64;

        for m in raw {
            match parse_prefix(&m.mac_prefix) {
                Some((key, prefix, bits)) => {
                    let vendor_idx = vendors.len() as u32;
                    vendors.push(m.vendor_name);
                    map.entry(key).or_default().push(OuiEntry { prefix, bits, vendor_idx });
                }
                None => skipped += 1,
            }
        }

        // De más largo a más corto, así el primero que matchee es el prefijo más específico.
        for entries in map.values_mut() {
            entries.sort_by(|a, b| b.bits.cmp(&a.bits));
        }

        if skipped > 0 {
            eprintln!("Aviso: {skipped} prefijos OUI no se pudieron parsear y se omitieron.");
        }
        Ok(OuiDb { map, vendors })
    }

    /// Devuelve el fabricante de una MAC, eligiendo el prefijo más específico que matchee.
    fn lookup(&self, mac: &[u8; 6]) -> Option<&str> {
        let key = [mac[0], mac[1], mac[2]];
        let entries = self.map.get(&key)?;
        let mac_u48 = mac48(mac);
        // Ya vienen ordenadas de más larga a más corta, así que tomamos la primera que matchea.
        for e in entries {
            let shift = 48 - e.bits;
            if (mac_u48 >> shift) == (e.prefix >> shift) {
                return Some(&self.vendors[e.vendor_idx as usize]);
            }
        }
        None
    }
}

/// Convierte una MAC de 6 bytes a un entero de 48 bits.
#[inline]
fn mac48(mac: &[u8; 6]) -> u64 {
    ((mac[0] as u64) << 40)
        | ((mac[1] as u64) << 32)
        | ((mac[2] as u64) << 24)
        | ((mac[3] as u64) << 16)
        | ((mac[4] as u64) << 8)
        | (mac[5] as u64)
}

/// Parsea un macPrefix ("AA:BB:CC" o "AA:BB:CC:DD/28") a (clave de 3 bytes, prefijo u48, bits).
fn parse_prefix(s: &str) -> Option<([u8; 3], u64, u32)> {
    let (hex_part, bits_part) = match s.split_once('/') {
        Some((h, b)) => (h, Some(b)),
        None => (s, None),
    };

    let mut octets = [0u8; 6];
    let mut n = 0usize;
    for part in hex_part.split([':', '-']) {
        if n >= 6 {
            break;
        }
        octets[n] = u8::from_str_radix(part.trim(), 16).ok()?;
        n += 1;
    }
    if n < 3 {
        return None; // un OUI siempre tiene al menos 24 bits
    }

    let bits = match bits_part {
        Some(b) => b.trim().parse::<u32>().ok()?,
        None => (n as u32) * 8,
    };
    if bits == 0 || bits > 48 {
        return None;
    }

    let mut prefix: u64 = 0;
    for (i, &o) in octets.iter().enumerate().take(n) {
        prefix |= (o as u64) << (40 - 8 * i);
    }
    Some(([octets[0], octets[1], octets[2]], prefix, bits))
}

// Inventario: MAC -> contadores (origen / destino)

#[derive(Default, Clone, Copy)]
struct DeviceStats {
    src_count: u64,
    dst_count: u64,
}

/// Estadísticas agregadas de captura (sumables entre sockets/hilos).
/// Solo se leen desde el módulo de fanout (Linux); en otras plataformas quedan sin usar.
#[derive(Default, Clone, Copy)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct CapStats {
    received: u64,
    dropped: u64,
    if_dropped: u64,
}

type Inventory = FxHashMap<[u8; 6], DeviceStats>;

#[inline]
fn is_multicast(mac: &[u8; 6]) -> bool {
    mac[0] & 0x01 != 0 // cubre broadcast (ff:ff:..) y multicast
}

#[inline]
fn is_locally_administered(mac: &[u8; 6]) -> bool {
    mac[0] & 0x02 != 0 // típico de MACs aleatorizadas
}

/// El corazón del programa: agarra una trama, saca las dos MAC y suma al inventario.
/// Ojo: acá NO se busca el fabricante (eso es caro y se hace una sola vez por MAC, al final).
#[inline]
fn handle_packet(data: &[u8], inventory: &mut Inventory, include_multicast: bool) {
    if data.len() < 14 {
        return; // ni siquiera entra la cabecera Ethernet
    }
    // Las MAC son los primeros 12 bytes (destino, luego origen). Si hay tag VLAN, va después.
    let dst: [u8; 6] = data[0..6].try_into().unwrap();
    let src: [u8; 6] = data[6..12].try_into().unwrap();

    // Un origen multicast no existe como dispositivo real, así que lo salteamos.
    if include_multicast || !is_multicast(&src) {
        inventory.entry(src).or_default().src_count += 1;
    }
    if include_multicast || !is_multicast(&dst) {
        inventory.entry(dst).or_default().dst_count += 1;
    }
}

/// Mismo bucle para archivo y para captura en vivo (genérico sobre el tipo de captura).
fn run_capture<T: Activated>(
    cap: &mut Capture<T>,
    inventory: &mut Inventory,
    include_multicast: bool,
    quiet: bool,
    stop: &Arc<AtomicBool>,
) {
    let mut packets: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match cap.next_packet() {
            Ok(packet) => {
                handle_packet(packet.data, inventory, include_multicast);
                packets += 1;
                if !quiet && packets % 500_000 == 0 {
                    eprint!("\rPaquetes procesados: {packets}");
                }
            }
            Err(pcap::Error::NoMorePackets) => break, // se acabó el .pcap
            Err(pcap::Error::TimeoutExpired) => continue, // en vivo: no llegó nada, volvemos a chequear stop
            Err(e) => {
                eprintln!("\nError de captura: {e}");
                break;
            }
        }
    }
    if !quiet {
        eprintln!("\rPaquetes procesados: {packets}        ");
    }
}

/// Verifica que la captura sea Ethernet. Si no, los offsets de MAC (0..12) son inválidos
/// y el inventario sería basura. Capturas con `tcpdump -i any` usan LINUX_SLL (no Ethernet)
/// y no llevan ambas MACs; un PCAP guardado como LINKTYPE_RAW tampoco tiene capa 2.
fn check_datalink<T: Activated>(cap: &Capture<T>) -> Result<()> {
    let dl = cap.get_datalink();
    if dl != Linktype::ETHERNET {
        let name = dl.get_name().unwrap_or_else(|_| format!("{dl:?}"));
        anyhow::bail!(
            "Datalink no soportado: {name} (esperado Ethernet, LINKTYPE_ETHERNET=1).\n\
             Esta herramienta lee las MACs de la cabecera Ethernet. Capturas con 'tcpdump -i any' \
             (LINUX_SLL) o sin capa 2 (RAW) no sirven; capturá sobre una interfaz Ethernet concreta."
        );
    }
    Ok(())
}

// Captura multi-hilo con PACKET_FANOUT, para cuando un solo socket no da abasto.
// AF_PACKET/PACKET_FANOUT son API de Linux; en otros sistemas (macOS, BSD) no existen,
// así que todo este bloque solo se compila en Linux. Ver el branch de --threads en main
// para el mensaje que se muestra si alguien lo pide en otra plataforma.
#[cfg(target_os = "linux")]
mod fanout {
    use super::{
        handle_packet, AtomicBool, Capture, CapStats, Context, FanoutMode, FxHashMap,
        Inventory, Ordering, Result,
    };
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::sync::Arc;
    use std::thread;

    // Constantes del kernel (estables en Linux; las fijamos para no depender de la versión de libc).
    const PACKET_FANOUT: libc::c_int = 18;
    const FANOUT_HASH: u32 = 0;
    const FANOUT_LB: u32 = 1;
    const FANOUT_CPU: u32 = 2;

    /// Une un socket AF_PACKET a un grupo PACKET_FANOUT. Todos los sockets con el mismo
    /// `group_id` reciben el tráfico balanceado por el kernel según `fanout_type`.
    fn set_fanout(fd: RawFd, group_id: u16, fanout_type: u32) -> std::io::Result<()> {
        let arg: u32 = ((fanout_type & 0xffff) << 16) | (group_id as u32);
        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_PACKET,
                PACKET_FANOUT,
                &arg as *const u32 as *const libc::c_void,
                std::mem::size_of::<u32>() as libc::socklen_t,
            )
        };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Captura en vivo con `threads` sockets en un grupo PACKET_FANOUT.
    /// Cada hilo mantiene su propio inventario (sin locks en el camino caliente);
    /// se fusionan al terminar. Devuelve el inventario combinado y las stats sumadas.
    pub fn run_live_fanout(
        iface: &str,
        threads: usize,
        snaplen: i32,
        buffer_size: i32,
        filter: Option<&str>,
        fanout: FanoutMode,
        include_multicast: bool,
        stop: &Arc<AtomicBool>,
    ) -> Result<(Inventory, CapStats)> {
        let fanout_type = match fanout {
            FanoutMode::Hash => FANOUT_HASH,
            FanoutMode::Cpu => FANOUT_CPU,
            FanoutMode::Lb => FANOUT_LB,
        };
        // group_id único por proceso para no chocar con otras instancias.
        let group_id = (std::process::id() & 0xffff) as u16;

        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let iface = iface.to_string();
            let filter = filter.map(|s| s.to_string());
            let stop = stop.clone();
            handles.push(thread::spawn(move || -> Result<(Inventory, Option<pcap::Stat>)> {
                let mut cap = Capture::from_device(iface.as_str())
                    .with_context(|| format!("Dispositivo no encontrado: {iface}"))?
                    .promisc(true)
                    .snaplen(snaplen)
                    .buffer_size(buffer_size)
                    .timeout(500)
                    .open()
                    .with_context(|| format!("No se pudo iniciar captura en {iface}"))?;
                super::check_datalink(&cap)?;
                // Solo activamos fanout con más de un hilo: un único socket dentro de un grupo
                // fanout puede romper la entrega de paquetes en algunos entornos (loopback, etc.).
                set_fanout(cap.as_raw_fd(), group_id, fanout_type)
                    .map_err(|e| anyhow::anyhow!("PACKET_FANOUT falló (¿kernel/permX?): {e}"))?;
                if let Some(f) = filter.as_deref() {
                    cap.filter(f, true).with_context(|| format!("Filtro BPF inválido: {f}"))?;
                }
                let mut inv: Inventory = FxHashMap::default();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match cap.next_packet() {
                        Ok(packet) => handle_packet(packet.data, &mut inv, include_multicast),
                        Err(pcap::Error::TimeoutExpired) => continue,
                        Err(pcap::Error::NoMorePackets) => break,
                        Err(_) => break,
                    }
                }
                let stat = cap.stats().ok();
                Ok((inv, stat))
            }));
        }

        // Fusión de inventarios y suma de estadísticas.
        let mut merged: Inventory = FxHashMap::default();
        let mut totals = CapStats::default();
        for h in handles {
            match h.join() {
                Ok(Ok((inv, stat))) => {
                    for (mac, s) in inv {
                        let e = merged.entry(mac).or_default();
                        e.src_count += s.src_count;
                        e.dst_count += s.dst_count;
                    }
                    if let Some(st) = stat {
                        totals.received += st.received as u64;
                        totals.dropped += st.dropped as u64;
                        totals.if_dropped += st.if_dropped as u64;
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("un hilo de captura entró en panic"),
            }
        }
        Ok((merged, totals))
    }
}
#[cfg(target_os = "linux")]
use fanout::run_live_fanout;

#[derive(Serialize)]
struct DeviceRow {
    mac: String,
    vendor: String,
    src_count: u64,
    dst_count: u64,
    total: u64,
    multicast: bool,
    locally_administered: bool,
}

fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

fn build_rows(inventory: &Inventory, oui: &OuiDb) -> Vec<DeviceRow> {
    let mut rows: Vec<DeviceRow> = inventory
        .iter()
        .map(|(mac, stats)| DeviceRow {
            mac: format_mac(mac),
            vendor: oui.lookup(mac).unwrap_or("Unknown").to_string(),
            src_count: stats.src_count,
            dst_count: stats.dst_count,
            total: stats.src_count + stats.dst_count,
            multicast: is_multicast(mac),
            locally_administered: is_locally_administered(mac),
        })
        .collect();
    // Ordenar por total descendente, luego por MAC ascendente para estabilidad.
    rows.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.mac.cmp(&b.mac)));
    rows
}

fn print_table(rows: &[DeviceRow]) {
    println!("\n----- Inventario de Dispositivos ({} únicos) -----", rows.len());
    println!(
        "{:<18} {:<32} {:>6} {:>6} {:>6}",
        "MAC", "Vendor", "Src", "Dst", "Total"
    );
    for r in rows {
        let vendor: String = r.vendor.chars().take(32).collect();
        println!(
            "{:<18} {:<32} {:>6} {:>6} {:>6}",
            r.mac, vendor, r.src_count, r.dst_count, r.total
        );
    }
}

fn write_csv(path: &str, rows: &[DeviceRow]) -> Result<()> {
    let mut w = csv::Writer::from_path(path).with_context(|| format!("No se pudo crear {path}"))?;
    for r in rows {
        w.serialize(r)?;
    }
    w.flush()?;
    Ok(())
}

fn write_json(path: &str, rows: &[DeviceRow]) -> Result<()> {
    let file = File::create(path).with_context(|| format!("No se pudo crear {path}"))?;
    serde_json::to_writer_pretty(file, rows)?;
    Ok(())
}

// main

fn main() -> Result<()> {
    let args = Args::parse();

    let oui = OuiDb::load(&args.oui_db)?;
    if !args.quiet {
        eprintln!("OUI cargados: {} fabricantes.", oui.vendors.len());
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        // Con Ctrl-C no queremos perder lo capturado: marcamos el flag y dejamos que el
        // bucle termine solo y escriba el inventario.
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }

    // --duration: un hilo aparte que, pasados N segundos, levanta el mismo flag de stop.
    // Cómodo para correr capturas acotadas (por ejemplo desde un timer de systemd).
    if let Some(secs) = args.duration {
        let stop = stop.clone();
        thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            stop.store(true, Ordering::Relaxed);
        });
    }

    let mut inventory: Inventory = FxHashMap::default();

    if let Some(pcap_path) = args.pcap.as_deref() {
        if !args.quiet {
            eprintln!("Procesando PCAP: {pcap_path}");
        }
        let mut cap = Capture::from_file(pcap_path)
            .with_context(|| format!("No se pudo abrir el PCAP {pcap_path}"))?;
        check_datalink(&cap)?;
        if let Some(f) = args.filter.as_deref() {
            cap.filter(f, true).with_context(|| format!("Filtro BPF inválido: {f}"))?;
        }
        run_capture(&mut cap, &mut inventory, args.include_multicast, args.quiet, &stop);
    } else if let Some(iface) = args.iface.as_deref() {
        let threads = args.threads.max(1);
        if threads > 1 {
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!(
                    "--threads {threads} pide PACKET_FANOUT, que es una API de Linux (AF_PACKET). \
                     En este sistema operativo no está disponible: usá --threads 1 (default)."
                );
            }
            #[cfg(target_os = "linux")]
            {
                if !args.quiet {
                    eprintln!(
                        "Captura en vivo: {iface} con {threads} hilos + PACKET_FANOUT (Ctrl-C para terminar)"
                    );
                }
                let (inv, stats) = run_live_fanout(
                    iface,
                    threads,
                    args.snaplen,
                    args.buffer_size,
                    args.filter.as_deref(),
                    args.fanout,
                    args.include_multicast,
                    &stop,
                )?;
                inventory = inv;
                if !args.quiet {
                    eprintln!(
                        "Captura: recibidos={}, descartados(buffer)={}, descartados(iface)={}",
                        stats.received, stats.dropped, stats.if_dropped
                    );
                }
                if stats.dropped > 0 || stats.if_dropped > 0 {
                    eprintln!(
                        "AVISO: se descartaron paquetes. Subí --buffer-size, agregá hilos (--threads) o acotá con --filter."
                    );
                }
            }
        } else {
            if !args.quiet {
                eprintln!("Capturando en vivo: {iface} (Ctrl-C para terminar)");
            }
            let mut cap = Capture::from_device(iface)
                .with_context(|| format!("Dispositivo no encontrado: {iface}"))?
                .promisc(true)
                .snaplen(args.snaplen)
                .buffer_size(args.buffer_size)
                .timeout(500) // ms: permite chequear el flag de stop entre lecturas
                .open()
                .with_context(|| format!("No se pudo iniciar captura en {iface}"))?;
            check_datalink(&cap)?;
            if let Some(f) = args.filter.as_deref() {
                cap.filter(f, true).with_context(|| format!("Filtro BPF inválido: {f}"))?;
            }
            run_capture(&mut cap, &mut inventory, args.include_multicast, args.quiet, &stop);
            // Reporte de drops: clave en una auditoría para saber si la captura fue completa.
            if let Ok(stats) = cap.stats() {
                if !args.quiet {
                    eprintln!(
                        "Captura: recibidos={}, descartados(buffer)={}, descartados(iface)={}",
                        stats.received, stats.dropped, stats.if_dropped
                    );
                }
                if stats.dropped > 0 || stats.if_dropped > 0 {
                    eprintln!(
                        "AVISO: se descartaron paquetes. Subí --buffer-size o reducí el tráfico con --filter."
                    );
                }
            }
        }
    }

    let rows = build_rows(&inventory, &oui);

    match args.output.as_deref() {
        Some(path) => {
            match args.format {
                OutputFormat::Csv => write_csv(path, &rows)?,
                OutputFormat::Json => write_json(path, &rows)?,
            }
            if !args.quiet {
                eprintln!("Inventario escrito en {path} ({} dispositivos).", rows.len());
            }
        }
        None => print_table(&rows),
    }

    Ok(())
}

// tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ma_l() {
        let (key, prefix, bits) = parse_prefix("00:00:0C").unwrap();
        assert_eq!(key, [0x00, 0x00, 0x0C]);
        assert_eq!(bits, 24);
        assert_eq!(prefix >> 24, 0x00000C);
    }

    #[test]
    fn parse_ma_m_with_mask() {
        let (key, _prefix, bits) = parse_prefix("8C:1F:64:00:00:00/28").unwrap();
        assert_eq!(key, [0x8C, 0x1F, 0x64]);
        assert_eq!(bits, 28);
    }

    #[test]
    fn longest_prefix_match() {
        let mut map: FxHashMap<[u8; 3], Vec<OuiEntry>> = FxHashMap::default();
        let (k1, p1, b1) = parse_prefix("8C:1F:64").unwrap(); // MA-L genérico
        let (k2, p2, b2) = parse_prefix("8C:1F:64:10:00:00/28").unwrap(); // MA-M específico
        map.entry(k1).or_default().push(OuiEntry { prefix: p1, bits: b1, vendor_idx: 0 });
        map.entry(k2).or_default().push(OuiEntry { prefix: p2, bits: b2, vendor_idx: 1 });
        for v in map.values_mut() {
            v.sort_by(|a, b| b.bits.cmp(&a.bits));
        }
        let db = OuiDb { map, vendors: vec!["Generic".into(), "Specific".into()] };
        // MAC dentro del bloque MA-M -> debe ganar el prefijo más largo.
        assert_eq!(db.lookup(&[0x8C, 0x1F, 0x64, 0x12, 0x34, 0x56]), Some("Specific"));
        // MAC fuera del bloque MA-M pero dentro del MA-L -> genérico.
        assert_eq!(db.lookup(&[0x8C, 0x1F, 0x64, 0xF0, 0x00, 0x01]), Some("Generic"));
    }

    #[test]
    fn multicast_detection() {
        assert!(is_multicast(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])); // broadcast
        assert!(is_multicast(&[0x01, 0x00, 0x5E, 0x00, 0x00, 0x01])); // IPv4 multicast
        assert!(!is_multicast(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55])); // unicast
    }

    #[test]
    fn handle_skips_multicast_src() {
        let mut inv: Inventory = FxHashMap::default();
        // src multicast, dst unicast
        let mut frame = vec![0u8; 14];
        frame[0..6].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // dst unicast
        frame[6..12].copy_from_slice(&[0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]); // src multicast
        handle_packet(&frame, &mut inv, false);
        assert!(inv.contains_key(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        assert!(!inv.contains_key(&[0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]));
    }
}
