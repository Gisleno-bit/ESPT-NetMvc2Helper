//! Captura pasiva de trafico UDP y calculo de estadisticas de netcode.
//!
//! Usa un raw socket de Windows con SIO_RCVALL. No necesita Npcap ni
//! ningun driver externo, pero SI necesita permisos de administrador.
//! No toca el proceso del juego ni el de Steam.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------
//  FFI de Winsock e IP Helper
// ---------------------------------------------------------------

#[link(name = "ws2_32")]
extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn socket(af: i32, ty: i32, proto: i32) -> usize;
    fn bind(s: usize, name: *const SockAddrIn, len: i32) -> i32;
    fn WSAIoctl(
        s: usize,
        code: u32,
        inbuf: *mut c_void,
        incb: u32,
        outbuf: *mut c_void,
        outcb: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
        routine: *mut c_void,
    ) -> i32;
    fn recv(s: usize, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn setsockopt(s: usize, level: i32, name: i32, val: *const u8, len: i32) -> i32;
    fn WSAGetLastError() -> i32;
}

#[link(name = "iphlpapi")]
extern "system" {
    fn IcmpCreateFile() -> isize;
    fn IcmpSendEcho(
        handle: isize,
        dest: u32,
        request: *const u8,
        request_size: u16,
        options: *const u8,
        reply: *mut u8,
        reply_size: u32,
        timeout: u32,
    ) -> u32;
}

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

const AF_INET: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_IP: i32 = 0;
const SIO_RCVALL: u32 = 0x9800_0001;
const SOL_SOCKET: i32 = 0xffff;
const SO_RCVTIMEO: i32 = 0x1006;
const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
const WSAETIMEDOUT: i32 = 10060;

// ---------------------------------------------------------------
//  Rangos de red conocidos
// ---------------------------------------------------------------

/// Prefijos de Valve (AS32590). Best-effort.
const RANGOS_VALVE: &[(&str, u32)] = &[
    ("45.121.184.0", 22),
    ("103.10.124.0", 23),
    ("103.28.54.0", 24),
    ("146.66.152.0", 21),
    ("153.254.86.0", 24),
    ("155.133.224.0", 19),
    ("162.254.192.0", 21),
    ("185.25.180.0", 22),
    ("190.217.33.0", 24),
    ("192.69.96.0", 22),
    ("205.185.194.0", 24),
    ("205.196.6.0", 24),
    ("208.64.200.0", 22),
    ("208.78.164.0", 22),
];

fn en_rango(ip: Ipv4Addr, base: &str, bits: u32) -> bool {
    let b: Ipv4Addr = match base.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if bits == 0 {
        return true;
    }
    let mask: u32 = u32::MAX << (32 - bits);
    (u32::from(ip) & mask) == (u32::from(b) & mask)
}

pub fn es_valve(ip: Ipv4Addr) -> bool {
    RANGOS_VALVE.iter().any(|(b, n)| en_rango(ip, b, *n))
}

pub fn es_privada(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10
        || (o[0] == 172 && (16..32).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
        || o[0] == 127
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || o[0] == 0
        || o[0] >= 224
}

// ---------------------------------------------------------------
//  Estado compartido con la interfaz
// ---------------------------------------------------------------

#[derive(Clone, Default)]
pub struct PeerView {
    pub ip: String,
    pub pps_in: f64,
    pub pps_out: f64,
    pub mean_ms: f64,
    pub jitter_ms: f64,
    pub max_gap_ms: f64,
    pub huecos: u64,
    pub rtt_ms: Option<u32>,
    pub hist: Vec<f32>,
}

/// Una linea de la tabla de diagnostico: cualquier IP publica vista.
#[derive(Clone, Default)]
pub struct Candidato {
    pub ip: String,
    pub pps_in: f64,
    pub pps_out: f64,
    pub mean_ms: f64,
    /// true si cumple la firma de un netcode de 60 Hz.
    pub es_juego: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub activo: bool,
    pub error: Option<String>,
    pub ip_local: String,
    pub peer: Option<PeerView>,
    pub valve_pps: f64,
    pub otros: usize,
    pub grabando: bool,
    /// Si esta activo, acepta cualquier flujo (para depurar).
    pub laxo: bool,
    // --- diagnostico ---
    pub total_ip: u64,
    pub total_udp: u64,
    pub candidatos: Vec<Candidato>,
}

pub type Compartido = Arc<Mutex<Snapshot>>;

// ---------------------------------------------------------------
//  Estadisticas internas por peer
// ---------------------------------------------------------------

struct Peer {
    ultimo: Option<Instant>,
    visto: Instant,
    intervalos: VecDeque<f64>,
    entrantes: VecDeque<Instant>,
    salientes: VecDeque<Instant>,
}

/// Firma de un netcode de juego de pelea a 60 Hz:
///  - flujo entrante sostenido (>= 30 pps)
///  - bidireccional y simetrico (ni un lado 3x el otro)
///  - intervalo medio corto (<= 50 ms)
fn parece_juego(pps_in: f64, pps_out: f64, mean_ms: f64) -> bool {
    if pps_in < 30.0 || pps_out < 30.0 {
        return false;
    }
    if mean_ms <= 0.0 || mean_ms > 50.0 {
        return false;
    }
    let mayor = pps_in.max(pps_out);
    let menor = pps_in.min(pps_out).max(0.001);
    mayor / menor <= 3.0
}

impl Peer {
    fn nuevo() -> Self {
        Peer {
            ultimo: None,
            visto: Instant::now(),
            intervalos: VecDeque::with_capacity(300),
            entrantes: VecDeque::with_capacity(300),
            salientes: VecDeque::with_capacity(300),
        }
    }

    fn entrada(&mut self, ahora: Instant) {
        self.visto = ahora;
        if let Some(prev) = self.ultimo {
            let ms = ahora.duration_since(prev).as_secs_f64() * 1000.0;
            if ms < 2000.0 {
                self.intervalos.push_back(ms);
                if self.intervalos.len() > 300 {
                    self.intervalos.pop_front();
                }
            }
        }
        self.ultimo = Some(ahora);
        self.entrantes.push_back(ahora);
    }

    fn salida(&mut self, ahora: Instant) {
        self.visto = ahora;
        self.salientes.push_back(ahora);
    }

    fn podar(&mut self, ahora: Instant) {
        let corte = ahora - Duration::from_secs(2);
        while self.entrantes.front().map_or(false, |t| *t < corte) {
            self.entrantes.pop_front();
        }
        while self.salientes.front().map_or(false, |t| *t < corte) {
            self.salientes.pop_front();
        }
    }

    fn pps_in(&self) -> f64 {
        self.entrantes.len() as f64 / 2.0
    }

    fn pps_out(&self) -> f64 {
        self.salientes.len() as f64 / 2.0
    }

    fn stats(&self) -> (f64, f64, f64, u64) {
        if self.intervalos.len() < 3 {
            return (0.0, 0.0, 0.0, 0);
        }
        let n = self.intervalos.len() as f64;
        let media: f64 = self.intervalos.iter().sum::<f64>() / n;
        let var: f64 = self
            .intervalos
            .iter()
            .map(|x| (x - media) * (x - media))
            .sum::<f64>()
            / n;
        let jitter = var.sqrt();
        let pico = self.intervalos.iter().cloned().fold(0.0_f64, f64::max);
        let umbral = (media * 3.0).max(50.0);
        let huecos = self.intervalos.iter().filter(|x| **x > umbral).count() as u64;
        (media, jitter, pico, huecos)
    }
}

// ---------------------------------------------------------------
//  Deteccion de la IP local
// ---------------------------------------------------------------

pub fn ip_local() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()? {
        std::net::SocketAddr::V4(a) => Some(*a.ip()),
        _ => None,
    }
}

// ---------------------------------------------------------------
//  Ping ICMP
// ---------------------------------------------------------------

fn ping(handle: isize, ip: Ipv4Addr) -> Option<u32> {
    if handle == -1 || handle == 0 {
        return None;
    }
    let datos = [0u8; 32];
    let mut reply = [0u8; 256];
    let dest = u32::from_le_bytes(ip.octets());
    let n = unsafe {
        IcmpSendEcho(
            handle,
            dest,
            datos.as_ptr(),
            datos.len() as u16,
            std::ptr::null(),
            reply.as_mut_ptr(),
            reply.len() as u32,
            600,
        )
    };
    if n == 0 {
        return None;
    }
    let status = u32::from_le_bytes([reply[4], reply[5], reply[6], reply[7]]);
    if status != 0 {
        return None;
    }
    Some(u32::from_le_bytes([
        reply[8], reply[9], reply[10], reply[11],
    ]))
}

pub fn hilo_ping(estado: Compartido) {
    let handle = unsafe { IcmpCreateFile() };
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        let objetivo = {
            let s = estado.lock().unwrap();
            s.peer.as_ref().and_then(|p| p.ip.parse::<Ipv4Addr>().ok())
        };
        if let Some(ip) = objetivo {
            let rtt = ping(handle, ip);
            let mut s = estado.lock().unwrap();
            if let Some(p) = s.peer.as_mut() {
                if p.ip == ip.to_string() {
                    p.rtt_ms = rtt;
                }
            }
        }
    }
}

// ---------------------------------------------------------------
//  Hilo principal de captura
// ---------------------------------------------------------------

pub fn hilo_captura(estado: Compartido) {
    let local = match ip_local() {
        Some(v) => v,
        None => {
            let mut s = estado.lock().unwrap();
            s.error = Some("No he podido detectar tu IP local.".into());
            return;
        }
    };

    println!("[netmon] IP local detectada: {}", local);

    {
        let mut s = estado.lock().unwrap();
        s.ip_local = local.to_string();
    }

    let mut wsadata = [0u8; 512];
    unsafe {
        WSAStartup(0x0202, wsadata.as_mut_ptr());
    }

    let sock = unsafe { socket(AF_INET, SOCK_RAW, IPPROTO_IP) };
    if sock == INVALID_SOCKET {
        let e = unsafe { WSAGetLastError() };
        println!("[netmon] socket() fallo: {}", e);
        let mut s = estado.lock().unwrap();
        s.error = Some(format!(
            "socket() fallo ({}). Ejecuta como ADMINISTRADOR.",
            e
        ));
        return;
    }

    let dir = SockAddrIn {
        sin_family: AF_INET as u16,
        sin_port: 0,
        sin_addr: u32::from_le_bytes(local.octets()),
        sin_zero: [0; 8],
    };
    if unsafe { bind(sock, &dir, std::mem::size_of::<SockAddrIn>() as i32) } == SOCKET_ERROR {
        let e = unsafe { WSAGetLastError() };
        println!("[netmon] bind() fallo: {}", e);
        let mut s = estado.lock().unwrap();
        s.error = Some(format!("bind() fallo ({}).", e));
        return;
    }

    let mut on: u32 = 1;
    let mut devueltos: u32 = 0;
    if unsafe {
        WSAIoctl(
            sock,
            SIO_RCVALL,
            &mut on as *mut u32 as *mut c_void,
            4,
            std::ptr::null_mut(),
            0,
            &mut devueltos,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == SOCKET_ERROR
    {
        let e = unsafe { WSAGetLastError() };
        println!("[netmon] SIO_RCVALL fallo: {}", e);
        let mut s = estado.lock().unwrap();
        s.error = Some(format!(
            "SIO_RCVALL fallo ({}). Hace falta ADMINISTRADOR.",
            e
        ));
        return;
    }

    let timeout: u32 = 200;
    unsafe {
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout as *const u32 as *const u8,
            4,
        );
    }

    println!("[netmon] captura activa. Escuchando...");

    {
        let mut s = estado.lock().unwrap();
        s.activo = true;
    }

    let mut buf = vec![0u8; 65536];
    let mut peers: HashMap<Ipv4Addr, Peer> = HashMap::new();
    let mut valve: VecDeque<Instant> = VecDeque::new();
    let mut ultimo_calculo = Instant::now();
    let mut ultimo_log = Instant::now();
    let mut total_ip: u64 = 0;
    let mut total_udp: u64 = 0;

    loop {
        let n = unsafe { recv(sock, buf.as_mut_ptr(), buf.len() as i32, 0) };
        let ahora = Instant::now();

        if n > 0 {
            total_ip += 1;
            let n = n as usize;
            if n >= 20 {
                let ihl = ((buf[0] & 0x0f) as usize) * 4;
                let proto = buf[9];
                if proto == 17 && n >= ihl + 8 {
                    total_udp += 1;
                    let src = Ipv4Addr::new(buf[12], buf[13], buf[14], buf[15]);
                    let dst = Ipv4Addr::new(buf[16], buf[17], buf[18], buf[19]);

                    if dst == local && !es_privada(src) {
                        if es_valve(src) {
                            valve.push_back(ahora);
                        } else {
                            peers.entry(src).or_insert_with(Peer::nuevo).entrada(ahora);
                        }
                    } else if src == local && !es_privada(dst) && !es_valve(dst) {
                        peers.entry(dst).or_insert_with(Peer::nuevo).salida(ahora);
                    }
                }
            }
        } else if n == SOCKET_ERROR {
            let e = unsafe { WSAGetLastError() };
            if e != WSAETIMEDOUT {
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        if ahora.duration_since(ultimo_calculo) < Duration::from_millis(250) {
            continue;
        }
        ultimo_calculo = ahora;

        let corte = ahora - Duration::from_secs(2);
        while valve.front().map_or(false, |t| *t < corte) {
            valve.pop_front();
        }

        peers.retain(|_, p| ahora.duration_since(p.visto) < Duration::from_secs(6));
        for p in peers.values_mut() {
            p.podar(ahora);
        }

        // Tabla de diagnostico: todo lo publico que se esta viendo.
        let mut candidatos: Vec<Candidato> = peers
            .iter()
            .map(|(ip, p)| {
                let (media, _, _, _) = p.stats();
                Candidato {
                    ip: ip.to_string(),
                    pps_in: p.pps_in(),
                    pps_out: p.pps_out(),
                    mean_ms: media,
                    es_juego: parece_juego(p.pps_in(), p.pps_out(), media),
                }
            })
            .collect();
        candidatos.sort_by(|a, b| {
            (b.pps_in + b.pps_out)
                .partial_cmp(&(a.pps_in + a.pps_out))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidatos.truncate(6);

        let laxo_actual = { estado.lock().unwrap().laxo };

        let mejor = peers
            .iter()
            .filter(|(_, p)| {
                let (media, _, _, _) = p.stats();
                if laxo_actual {
                    p.pps_in() + p.pps_out() >= 5.0
                } else {
                    parece_juego(p.pps_in(), p.pps_out(), media)
                }
            })
            .max_by(|a, b| {
                let x = a.1.pps_in() + a.1.pps_out();
                let y = b.1.pps_in() + b.1.pps_out();
                x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(ip, p)| (*ip, p));

        let vista = mejor.map(|(ip, p)| {
            let (media, jitter, pico, huecos) = p.stats();
            let hist: Vec<f32> = p
                .intervalos
                .iter()
                .rev()
                .take(120)
                .rev()
                .map(|v| *v as f32)
                .collect();
            PeerView {
                ip: ip.to_string(),
                pps_in: p.pps_in(),
                pps_out: p.pps_out(),
                mean_ms: media,
                jitter_ms: jitter,
                max_gap_ms: pico,
                huecos,
                rtt_ms: None,
                hist,
            }
        });

        let grabar;
        {
            let mut s = estado.lock().unwrap();
            let rtt_previo = s.peer.as_ref().and_then(|p| p.rtt_ms);
            let misma = match (&s.peer, &vista) {
                (Some(a), Some(b)) => a.ip == b.ip,
                _ => false,
            };
            s.peer = vista.map(|mut v| {
                if misma {
                    v.rtt_ms = rtt_previo;
                }
                v
            });
            s.valve_pps = valve.len() as f64 / 2.0;
            s.otros = peers.len();
            s.total_ip = total_ip;
            s.total_udp = total_udp;
            s.candidatos = candidatos;
            grabar = s.grabando;
        }

        if grabar && ahora.duration_since(ultimo_log) >= Duration::from_secs(1) {
            ultimo_log = ahora;
            let s = estado.lock().unwrap();
            if let Some(p) = &s.peer {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if let Ok(mut f) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("mvc-netmon-log.csv")
                {
                    let _ = writeln!(
                        f,
                        "{},{},{:.1},{:.1},{:.2},{:.2},{:.1},{},{}",
                        ts,
                        p.ip,
                        p.pps_in,
                        p.pps_out,
                        p.mean_ms,
                        p.jitter_ms,
                        p.max_gap_ms,
                        p.huecos,
                        p.rtt_ms.map(|v| v.to_string()).unwrap_or_default()
                    );
                }
            }
        }
    }
}
