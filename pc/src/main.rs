//! ClipBridge — LAN clipboard sync. Windows peer.
//!
//! Discovery: UDP broadcast on 47823 (plus unicast reply, because Android often
//! drops inbound broadcast).
//! Sync: TCP 47824, one length-prefixed UTF-8 message per connection.
//! Dashboard: HTTP on 47825 — history + send-back, no framework, std only.

mod gui;

use arboard::Clipboard;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DISCOVERY_PORT: u16 = 47823;
const SYNC_PORT: u16 = 47824;
const HTTP_PORT: u16 = 47825;
const MAGIC: &str = "CLIPB1";
const MAX_MSG: usize = 32 << 20;
pub const TYPE_TEXT: u8 = 0;
const TYPE_IMAGE: u8 = 1;
/// ack: carries the 16-byte clip id the receiver just applied (Smart Connect's
/// ClipboardDataSynced). Sender retries until this arrives, so a dropped link
/// can no longer silently lose a clip.
const TYPE_SYNCED: u8 = 3;

type Outbox = Arc<Mutex<std::collections::HashMap<[u8; 16], (u8, Vec<u8>, Instant, u8)>>>;
type Seen = Arc<Mutex<std::collections::HashSet<[u8; 16]>>>;

static OUTBOX: std::sync::OnceLock<Outbox> = std::sync::OnceLock::new();
static SEEN: std::sync::OnceLock<Seen> = std::sync::OnceLock::new();
static SET_AT: std::sync::OnceLock<Mutex<Option<Instant>>> = std::sync::OnceLock::new();

fn set_at() -> &'static Mutex<Option<Instant>> {
    SET_AT.get_or_init(|| Mutex::new(None))
}

/// True just after we wrote a peer's clip onto our own clipboard: re-encoding a
/// received PNG changes its bytes, so hashing alone cannot stop the echo.
fn recently_set() -> bool {
    set_at()
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|t| t.elapsed() < Duration::from_millis(1500))
        .unwrap_or(false)
}

fn outbox() -> &'static Outbox {
    OUTBOX.get_or_init(|| Arc::new(Mutex::new(std::collections::HashMap::new())))
}

fn seen() -> &'static Seen {
    SEEN.get_or_init(|| Arc::new(Mutex::new(std::collections::HashSet::new())))
}

fn clip_id(payload: &[u8]) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(payload);
    let mut id = [0u8; 16];
    id.copy_from_slice(&d[..16]);
    id
}

// ---- pairing key -------------------------------------------------------
// Both ends derive the same key from the same 6-digit PIN:
//   key = SHA-256("clipbridge:" + pin)
// Each message carries a fresh 12-byte nonce; AES-256-GCM authenticates it.

static KEY: std::sync::RwLock<[u8; 32]> = std::sync::RwLock::new([0u8; 32]);
static KEY_INIT: std::sync::Once = std::sync::Once::new();

fn key() -> [u8; 32] {
    KEY_INIT.call_once(|| {
        *KEY.write().unwrap() = key_from(&pin());
    });
    *KEY.read().unwrap()
}

pub fn set_pin(p: &str) {
    *KEY.write().unwrap() = key_from(p);
}

fn key_from(pin: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"clipbridge:");
    h.update(pin.as_bytes());
    let d = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&d);
    k
}

fn encrypt(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit};
    use rand::RngCore;
    let cipher = Aes256Gcm::new_from_slice(key).expect("key");
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain)
        .unwrap_or_default();
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn decrypt(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit};
    if data.len() < 13 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher
        .decrypt(aes_gcm::Nonce::from_slice(&data[..12]), &data[12..])
        .ok()
}

pub fn pin() -> String {
    if let Ok(p) = std::env::var("CLIPBRIDGE_PIN") {
        if !p.trim().is_empty() {
            return p.trim().to_string();
        }
    }
    if let Ok(p) = std::fs::read_to_string("pin.txt") {
        if !p.trim().is_empty() {
            return p.trim().to_string();
        }
    }
    // first run: generate a random 6-digit code and keep it
    let n: u32 = rand::random::<u32>() % 900_000 + 100_000;
    let p = n.to_string();
    let _ = std::fs::write("pin.txt", &p);
    p
}

pub struct Item {
    pub at: Instant,
    pub text: String,
    pub from: String,
}


// ---- event-driven clipboard (no polling) -------------------------------
// A message-only window registered with AddClipboardFormatListener gets
// WM_CLIPBOARDUPDATE the instant the clipboard changes. Idle cost is zero.
#[cfg(windows)]
fn spawn_clipboard_listener(tx: std::sync::mpsc::Sender<()>) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::DataExchange::AddClipboardFormatListener;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
        RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WM_CLIPBOARDUPDATE, WNDCLASSW,
    };

    static TX: OnceLock<std::sync::mpsc::Sender<()>> = OnceLock::new();
    let _ = TX.set(tx);

    unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_CLIPBOARDUPDATE {
            if let Some(tx) = TX.get() {
                let _ = tx.send(());
            }
        }
        DefWindowProcW(h, msg, w, l)
    }

    std::thread::spawn(move || unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class: Vec<u16> = "ClipBridgeWnd\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        AddClipboardFormatListener(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

fn main() {
    let peers: Arc<Mutex<HashSet<IpAddr>>> = Arc::new(Mutex::new(HashSet::new()));
    let last: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let history: Arc<Mutex<Vec<Item>>> = Arc::new(Mutex::new(Vec::new()));
    // remote clips wait here; only the main thread owns a working clipboard context
    let pending: Arc<Mutex<Option<(u8, Vec<u8>)>>> = Arc::new(Mutex::new(None));

    println!("ClipBridge");
    println!("  dashboard http://127.0.0.1:{HTTP_PORT}");
    println!("  discovery udp/{DISCOVERY_PORT}  sync tcp/{SYNC_PORT}");

    // ---- discovery -------------------------------------------------------
    {
        let peers = peers.clone();
        thread::spawn(move || {
            let sock = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("discovery bind failed: {e}");
                    return;
                }
            };
            let _ = sock.set_broadcast(true);
            let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
            let hello = format!("{}|{}", MAGIC, SYNC_PORT);
            // 255.255.255.255 follows the default route, which on a multi-homed
            // box is often Hyper-V/WSL rather than Wi-Fi. Also aim at the /24.
            let mut targets: Vec<SocketAddr> = vec![
                SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT)),
            ];
            if let IpAddr::V4(v4) = local_ip() {
                let o = v4.octets();
                targets.push(SocketAddr::from(([o[0], o[1], o[2], 255], DISCOVERY_PORT)));
            }
            let mut buf = [0u8; 128];
            loop {
                for t in &targets {
                    let _ = sock.send_to(hello.as_bytes(), t);
                }
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    let msg = String::from_utf8_lossy(&buf[..n]);
                    if msg.starts_with(MAGIC) && !is_self(from.ip()) {
                        let _ = sock.send_to(hello.as_bytes(), from);
                        let mut p = peers.lock().unwrap();
                        if p.insert(from.ip()) {
                            println!("  + peer {}", from.ip());
                        }
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
        });
    }

    // ---- sync links ------------------------------------------------------
    // The PC always dials out. Outbound is never blocked by the firewall, so a
    // phone on a Private network stays reachable even when inbound is refused.
    // Once a link is up it is duplex: phone->PC rides back on the same socket.
    let links: Links = Arc::new(Mutex::new(Vec::new()));

    {
        let links = links.clone();
        let last = last.clone();
        let history = history.clone();
        let pending = pending.clone();
        thread::spawn(move || {
            let Ok(listener) = TcpListener::bind(("0.0.0.0", SYNC_PORT)) else {
                eprintln!("sync bind failed");
                return;
            };
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                register(links.clone(), s, last.clone(), history.clone(), pending.clone());
            }
        });
    }

    {
        let links = links.clone();
        let last = last.clone();
        let history = history.clone();
        let pending = pending.clone();
        thread::spawn(move || loop {
            if links.lock().unwrap().is_empty() {
                if let IpAddr::V4(v4) = local_ip() {
                    let o = v4.octets();
                    let (tx, rx) = std::sync::mpsc::channel();
                    for i in 1..=254u8 {
                        if [o[0], o[1], o[2], i] == o {
                            continue;
                        }
                        let tx = tx.clone();
                        let ip = IpAddr::V4(std::net::Ipv4Addr::new(o[0], o[1], o[2], i));
                        thread::spawn(move || {
                            let addr = SocketAddr::new(ip, SYNC_PORT);
                            if let Ok(s) =
                                TcpStream::connect_timeout(&addr, Duration::from_millis(300))
                            {
                                let _ = tx.send((ip, s));
                            }
                        });
                    }
                    drop(tx);
                    for (ip, s) in rx {
                        println!("  + link {ip}");
                        register(links.clone(), s, last.clone(), history.clone(), pending.clone());
                        // a new link must receive the current clipboard
                        *last.lock().unwrap() = String::new();
                    }
                }
            }
            thread::sleep(Duration::from_secs(3));
        });
    }

    // ---- dashboard -------------------------------------------------------
    {
        let history = history.clone();
        let links = links.clone();
        let last = last.clone();
        thread::spawn(move || {
            let listener = match TcpListener::bind(("0.0.0.0", HTTP_PORT)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("dashboard bind failed: {e}");
                    return;
                }
            };
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                let history = history.clone();
                let links = links.clone();
                let last = last.clone();
                thread::spawn(move || {
                    let _ = handle_http(s, &history, &links, &last);
                });
            }
        });
    }

    // ---- send (clipboard poll) -------------------------------------------
    let mut clip = match Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("clipboard unavailable: {e}");
            return;
        }
    };

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    gui::spawn(history.clone(), links.clone());
    spawn_mdns(links.clone(), last.clone(), history.clone(), pending.clone());
    spawn_ack_retry(links.clone());
    spawn_clipboard_listener(tx);

    loop {
        // apply anything a peer sent, on the thread that owns the clipboard
        if let Some((kind, bytes)) = pending.lock().unwrap().take() {
            *set_at().lock().unwrap() = Some(Instant::now());
            match kind {
                TYPE_IMAGE => {
                    if let Some(img) = decode_png(&bytes) {
                        if let Err(e) = clip.set_image(img) {
                            println!("  ! clipboard image failed: {e}");
                        }
                        // keep a visible copy: the clipboard alone gives no clue
                        // that a screenshot arrived
                        let dir = std::path::Path::new("received");
                        let _ = std::fs::create_dir_all(dir);
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let path = dir.join(format!("clip-{stamp}.png"));
                        if std::fs::write(&path, &bytes).is_ok() {
                            println!("  saved {}", path.display());
                        }
                    }
                }
                _ => {
                    let t = String::from_utf8_lossy(&bytes).to_string();
                    *last.lock().unwrap() = format!("0:{}", hash(t.as_bytes()));
                    if let Err(e) = clip.set_text(t.clone()) {
                        println!("  ! clipboard set failed: {e}");
                    }
                }
            }
        }

        // text wins if present; otherwise look for an image (screenshot) on the clipboard
        let mut kind = TYPE_TEXT;
        let mut payload: Vec<u8> = Vec::new();
        let mut label = String::new();

        if let Ok(t) = clip.get_text() {
            if !t.is_empty() {
                payload = t.as_bytes().to_vec();
                label = t.clone();
            }
        }
        if payload.is_empty() {
            if let Ok(img) = clip.get_image() {
                if let Some(png) = encode_png(&img) {
                    kind = TYPE_IMAGE;
                    label = format!("[screenshot {}×{}]", img.width, img.height);
                    payload = png;
                }
            }
        }

        if !payload.is_empty() {
            let sig = format!("{kind}:{}", hash(&payload));
            let changed = {
                let mut l = last.lock().unwrap();
                if *l != sig {
                    *l = sig;
                    true
                } else {
                    false
                }
            };
            if changed && !recently_set() {
                push(&history, label.clone(), "pc".to_string());
                broadcast(&links, kind, &payload);
                if kind == TYPE_IMAGE {
                    println!("  -> screenshot {} KB", payload.len() / 1024);
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data.iter().take(4096) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (data.len() as u64)
}

fn encode_png(img: &arboard::ImageData) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&img.bytes).ok()?;
    }
    Some(out)
}

fn decode_png(data: &[u8]) -> Option<arboard::ImageData<'static>> {
    let mut reader = png::Decoder::new(data).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some(arboard::ImageData {
        width: info.width as usize,
        height: info.height as usize,
        bytes: buf.into(),
    })
}

pub fn push(history: &Arc<Mutex<Vec<Item>>>, text: String, from: String) {
    let mut h = history.lock().unwrap();
    if h.first().map(|i| i.text == text).unwrap_or(false) {
        return;
    }
    h.insert(0, Item { at: Instant::now(), text, from });
    h.truncate(60);
}

type Links = Arc<Mutex<Vec<Arc<Mutex<TcpStream>>>>>;
pub type Hist = Arc<Mutex<Vec<Item>>>;
type Pend = Arc<Mutex<Option<(u8, Vec<u8>)>>>;
type Last = Arc<Mutex<String>>;

/// Keep one accepted/dialled socket alive, read frames from it forever and
/// hand each decrypted payload to the shared handler.
fn register(links: Links, s: TcpStream, last: Last, history: Hist, pending: Pend) -> bool {
    let _ = s.set_nodelay(true);
    // the scan and mDNS can both reach the same phone; one link per peer is enough
    if let Ok(pa) = s.peer_addr() {
        let dup = links.lock().unwrap().iter().any(|l| {
            l.lock()
                .ok()
                .and_then(|x| x.peer_addr().ok())
                .map(|p| p.ip() == pa.ip())
                .unwrap_or(false)
        });
        if dup {
            return false;
        }
    }
    let Ok(mut rd) = s.try_clone() else { return false };
    let sock = Arc::new(Mutex::new(s));
    links.lock().unwrap().push(sock.clone());
    thread::spawn(move || {
        while let Some(plain) = read_frame(&mut rd) {
            handle_plain(plain, &last, &history, &pending, &links);
        }
        links.lock().unwrap().retain(|x| !Arc::ptr_eq(x, &sock));
        println!("  - link closed");
    });
    true
}

fn read_frame(s: &mut TcpStream) -> Option<Vec<u8>> {
    let _ = s.set_read_timeout(None);
    let mut len_buf = [0u8; 4];
    s.read_exact(&mut len_buf).ok()?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len < 2 || len > MAX_MSG {
        return None;
    }
    let mut data = vec![0u8; len];
    s.read_exact(&mut data).ok()?;
    let plain = decrypt(&key(), &data);
    if plain.is_none() {
        println!("  ! bad key — PIN mismatch");
    }
    plain
}

fn handle_plain(plain: Vec<u8>, last: &Last, history: &Hist, pending: &Pend, links: &Links) {
    if plain.is_empty() {
        return;
    }
    let kind = plain[0];
    let payload = &plain[1..];

    if kind == TYPE_SYNCED {
        if payload.len() >= 16 {
            let mut id = [0u8; 16];
            id.copy_from_slice(&payload[..16]);
            if outbox().lock().unwrap().remove(&id).is_some() {
                println!("  ✓ synced");
            }
        }
        return;
    }

    // duplicate delivery after a retry: acknowledge again, do not re-apply
    let id = clip_id(payload);
    if !seen().lock().unwrap().insert(id) {
        send_raw(links, TYPE_SYNCED, &id);
        return;
    }

    if kind == TYPE_IMAGE {
        if let Some(img) = decode_png(payload) {
            // hand off to the main thread, which owns the clipboard context
            *pending.lock().unwrap() = Some((TYPE_IMAGE, payload.to_vec()));
            *last.lock().unwrap() = format!("{kind}:{}", hash(payload));
            push(history, format!("[screenshot {}×{}]", img.width, img.height), "phone".into());
            println!("  <- image {}x{}", img.width, img.height);
        }
        send_raw(links, TYPE_SYNCED, &id);
        return;
    }

    let text = String::from_utf8_lossy(payload).to_string();
    if text.is_empty() {
        return;
    }
    *last.lock().unwrap() = format!("{kind}:{}", hash(payload));
    *pending.lock().unwrap() = Some((TYPE_TEXT, text.as_bytes().to_vec()));
    push(history, text.clone(), "phone".into());
    println!("  <- {} chars", text.chars().count());
    send_raw(links, TYPE_SYNCED, &id);
}

fn send_raw(links: &Links, kind: u8, body: &[u8]) {
    let targets: Vec<Arc<Mutex<TcpStream>>> = links.lock().unwrap().iter().cloned().collect();
    let mut plain = Vec::with_capacity(body.len() + 1);
    plain.push(kind);
    plain.extend_from_slice(body);
    let data = encrypt(&key(), &plain);
    for l in targets {
        let d = data.clone();
        thread::spawn(move || {
            if let Ok(mut s) = l.lock() {
                let _ = s.write_all(&(d.len() as u32).to_le_bytes());
                let _ = s.write_all(&d);
                let _ = s.flush();
            }
        });
    }
}

pub fn broadcast(links: &Links, kind: u8, payload: &[u8]) {
    let targets: Vec<Arc<Mutex<TcpStream>>> = links.lock().unwrap().iter().cloned().collect();
    let n = targets.len();
    let id = clip_id(payload);
    seen().lock().unwrap().insert(id);
    if n > 0 {
        outbox().lock().unwrap().insert(id, (kind, payload.to_vec(), Instant::now(), 0));
    }
    let mut plain = Vec::with_capacity(payload.len() + 1);
    plain.push(kind);
    plain.extend_from_slice(payload);
    let data = encrypt(&key(), &plain);
    for l in targets {
        let d = data.clone();
        thread::spawn(move || {
            if let Ok(mut s) = l.lock() {
                let _ = s.write_all(&(d.len() as u32).to_le_bytes());
                let _ = s.write_all(&d);
                let _ = s.flush();
            }
        });
    }
    println!("  -> {} bytes to {} link(s)", payload.len(), n);
}

/// mDNS browse, the way Smart Connect finds peers (`_readyfor._tcp`). The phone
/// advertises `_clipbridge._tcp`; we dial whatever resolves.
fn spawn_mdns(links: Links, last: Last, history: Hist, pending: Pend) {
    thread::spawn(move || {
        let Ok(mdns) = mdns_sd::ServiceDaemon::new() else {
            return;
        };
        let Ok(rx) = mdns.browse("_clipbridge._tcp.local.") else {
            return;
        };
        while let Ok(ev) = rx.recv() {
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = ev {
                let port = info.get_port();
                for ip in info.get_addresses() {
                    let addr = SocketAddr::new(*ip, port);
                    if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_millis(800)) {
                        if register(links.clone(), s, last.clone(), history.clone(), pending.clone()) {
                            println!("  + link {ip} (mdns)");
                            *last.lock().unwrap() = String::new();
                        }
                        break;
                    }
                }
            }
        }
    });
}

/// Retry anything unacknowledged: three attempts, 3 s apart. This is the
/// reliability Smart Connect gets from ClipboardDataSynced.
fn spawn_ack_retry(links: Links) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(3));
        let stale: Vec<([u8; 16], u8, Vec<u8>, u8)> = {
            let mut ob = outbox().lock().unwrap();
            let mut out = Vec::new();
            ob.retain(|id, (kind, payload, at, tries)| {
                if at.elapsed() < Duration::from_secs(3) {
                    return true;
                }
                if *tries >= 3 {
                    println!("  ! no ack after 3 tries, dropping");
                    return false;
                }
                *tries += 1;
                *at = Instant::now();
                out.push((*id, *kind, payload.clone(), *tries));
                true
            });
            out
        };
        for (id, kind, payload, tries) in stale {
            if links.lock().unwrap().is_empty() {
                continue;
            }
            println!("  ↻ retry {tries}/3 {:?}", &id[..4]);
            let mut plain = Vec::with_capacity(payload.len() + 1);
            plain.push(kind);
            plain.extend_from_slice(&payload);
            let data = encrypt(&key(), &plain);
            let targets: Vec<Arc<Mutex<TcpStream>>> =
                links.lock().unwrap().iter().cloned().collect();
            for l in targets {
                let d = data.clone();
                thread::spawn(move || {
                    if let Ok(mut s) = l.lock() {
                        let _ = s.write_all(&(d.len() as u32).to_le_bytes());
                        let _ = s.write_all(&d);
                        let _ = s.flush();
                    }
                });
            }
        }
    });
}

// ------------------------------------------------------------------ dashboard

fn handle_http(
    mut s: TcpStream,
    history: &Hist,
    links: &Links,
    last: &Last,
) -> std::io::Result<()> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let mut reader = BufReader::new(s.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("GET");
    let path = parts.get(1).copied().unwrap_or("/");

    if method == "POST" && path.starts_with("/send") {
        let raw = String::from_utf8_lossy(&body).to_string();
        // the dashboard form posts application/x-www-form-urlencoded: t=<text>
        let text = if let Some(rest) = raw.strip_prefix("t=") {
            urldecode(rest)
        } else {
            raw
        };
        if !text.trim().is_empty() {
            *last.lock().unwrap() = text.clone();
            if let Ok(mut c) = Clipboard::new() {
                let _ = c.set_text(text.clone());
            }
            push(history, text.clone(), "pc".into());
            broadcast(links, TYPE_TEXT, text.as_bytes());
        }
        let _ = s.write_all(b"HTTP/1.1 303 See Other\r\nLocation: /\r\nContent-Length: 0\r\n\r\n");
        return Ok(());
    }

    let page = render(history, links);
    let bytes = page.as_bytes();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(bytes)?;
    s.flush()
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn render(history: &Hist, links: &Links) -> String {
    let h = history.lock().unwrap();
    let n_peers = links.lock().unwrap().len();
    let mut rows = String::new();
    for it in h.iter() {
        let secs = it.at.elapsed().as_secs();
        let ago = if secs < 60 { format!("{secs}s") } else { format!("{}m", secs / 60) };
        let esc = it.text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let short: String = esc.chars().take(400).collect();
        let dir = if it.from == "pc" { "↑" } else { "↓" };
        rows.push_str(&format!(
            "<div class=c><div class=m>{dir} {ago} · {}</div><pre>{short}</pre></div>",
            it.from
        ));
    }
    if rows.is_empty() {
        rows = "<p class=empty>Nothing yet. Copy something on either device.</p>".into();
    }
    format!(
        r#"<!doctype html><html><head><meta charset=utf-8><meta http-equiv=refresh content=2>
<title>ClipBridge</title><style>
*{{box-sizing:border-box}}
body{{margin:0;background:#14120F;color:#F2EDE4;font:14px/1.5 system-ui,sans-serif;padding:28px}}
h1{{font-size:26px;margin:0 0 4px;letter-spacing:-.5px}}
.sub{{color:#6F675C;font-size:12px;margin-bottom:22px}}
.badge{{display:inline-block;background:#1E1B17;border:1px solid #2C2823;color:#E8B04B;
padding:4px 10px;border-radius:10px;font-size:11px;margin-right:6px}}
form{{display:flex;gap:8px;margin:18px 0 24px}}
textarea{{flex:1;background:#1E1B17;border:1px solid #2C2823;color:#F2EDE4;border-radius:12px;
padding:12px;font:13px/1.4 ui-monospace,monospace;resize:vertical;min-height:64px}}
button{{background:#D9663A;color:#14120F;border:0;border-radius:12px;padding:0 20px;
font-weight:700;font-size:13px;cursor:pointer}}
.c{{background:#1E1B17;border:1px solid #2C2823;border-radius:14px;padding:12px 14px;margin-bottom:10px}}
.m{{color:#6F675C;font-size:11px;margin-bottom:6px}}
pre{{margin:0;white-space:pre-wrap;word-break:break-word;font:13px/1.45 ui-monospace,monospace}}
.empty{{color:#6F675C}}
</style></head><body>
<h1>ClipBridge</h1>
<div class=sub>phone ⇄ pc clipboard</div>
<div><span class=badge>{n_peers} peer(s)</span><span class=badge>http 47825</span></div>
<form method=post action=/send><textarea name=t placeholder="Type or paste, then Send to phone"></textarea>
<button>Send</button></form>
{rows}
</body></html>"#
    )
}

/// True when `ip` is one of this machine's own addresses. A UDP connect makes
/// the OS pick the source address it would use to reach that peer — if that is
/// the peer itself, we are looking at our own multi-homed adapter (Hyper-V, WSL).
fn is_self(ip: IpAddr) -> bool {
    let Ok(s) = UdpSocket::bind(("0.0.0.0", 0)) else {
        return false;
    };
    match s.connect((ip, 9)) {
        Ok(()) => s.local_addr().map(|a| a.ip() == ip).unwrap_or(false),
        Err(_) => false,
    }
}

fn local_ip() -> IpAddr {
    if let Ok(s) = UdpSocket::bind(("0.0.0.0", 0)) {
        if s.connect(("8.8.8.8", 80)).is_ok() {
            if let Ok(a) = s.local_addr() {
                return a.ip();
            }
        }
    }
    IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
}
