//! Native Win32 window for ClipBridge. No webview, no extra crates.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::Item;

static H_STATUS: AtomicIsize = AtomicIsize::new(0);
static H_LIST: AtomicIsize = AtomicIsize::new(0);
static H_EDIT: AtomicIsize = AtomicIsize::new(0);
static H_SEND: AtomicIsize = AtomicIsize::new(0);
static H_PIN: AtomicIsize = AtomicIsize::new(0);
static H_PINSAVE: AtomicIsize = AtomicIsize::new(0);
static HISTORY: Mutex<Option<Arc<Mutex<Vec<Item>>>>> = Mutex::new(None);
static PEERS: Mutex<Option<crate::Links>> = Mutex::new(None);

const ID_SEND: usize = 1001;
const ID_PINSAVE: usize = 1002;
const ID_TIMER: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn spawn(history: Arc<Mutex<Vec<Item>>>, links: crate::Links) {
    *HISTORY.lock().unwrap() = Some(history);
    *PEERS.lock().unwrap() = Some(links);
    std::thread::spawn(|| unsafe { run_window() });
}

unsafe fn run_window() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        match msg {
            WM_CREATE => {
                let st = wide("STATIC");
                let lb = wide("LISTBOX");
                let ed = wide("EDIT");
                let bt = wide("BUTTON");
                let empty = wide("");

                let mk = |cls: &Vec<u16>, text: &Vec<u16>, style: u32, x: i32, y: i32, w: i32, hh: i32, id: usize, ex: u32| {
                    CreateWindowExW(
                        ex, cls.as_ptr(), text.as_ptr(), style, x, y, w, hh, h,
                        id as *mut _, std::ptr::null_mut(), std::ptr::null(),
                    )
                };

                let status = mk(&st, &empty, WS_CHILD | WS_VISIBLE, 16, 10, 700, 18, 0, 0);
                let pin_label = wide("PAIRING PIN");
                let _ = mk(&st, &pin_label, WS_CHILD | WS_VISIBLE, 16, 38, 90, 18, 0, 0);
                let pin = mk(&ed, &wide(&crate::pin()), WS_CHILD | WS_VISIBLE | WS_BORDER, 112, 34, 90, 24, 0, 0);
                let save = mk(&bt, &wide("Set PIN"), WS_CHILD | WS_VISIBLE, 208, 34, 78, 24, ID_PINSAVE, 0);
                let list = mk(&lb, &empty, WS_CHILD | WS_VISIBLE | WS_VSCROLL | 0x0080 | WS_EX_CLIENTEDGE as u32, 16, 70, 700, 260, 0, WS_EX_CLIENTEDGE);
                let edit = mk(&ed, &empty, WS_CHILD | WS_VISIBLE | WS_VSCROLL | 0x0004 | 0x0040, 16, 340, 560, 56, 0, WS_EX_CLIENTEDGE);
                let send = mk(&bt, &wide("Send to phone"), WS_CHILD | WS_VISIBLE, 584, 340, 132, 56, ID_SEND, 0);

                H_STATUS.store(status as isize, Ordering::SeqCst);
                H_PIN.store(pin as isize, Ordering::SeqCst);
                H_PINSAVE.store(save as isize, Ordering::SeqCst);
                H_LIST.store(list as isize, Ordering::SeqCst);
                H_EDIT.store(edit as isize, Ordering::SeqCst);
                H_SEND.store(send as isize, Ordering::SeqCst);

                SetTimer(h, ID_TIMER, 1000, None);
                refresh();
                0
            }
            WM_TIMER => {
                refresh();
                0
            }
            WM_COMMAND => {
                let id = w & 0xffff;
                if id == ID_SEND {
                    send_from_box();
                } else if id == ID_PINSAVE {
                    save_pin();
                }
                0
            }
            WM_SIZE => {
                let width = (l & 0xffff) as i32;
                let height = ((l >> 16) & 0xffff) as i32;
                let g = |a: &AtomicIsize| a.load(Ordering::SeqCst) as HWND;
                let (status, list, edit, btn) =
                    (g(&H_STATUS), g(&H_LIST), g(&H_EDIT), g(&H_SEND));
                if !status.is_null() {
                    MoveWindow(status, 16, 10, width - 32, 18, 1);
                    MoveWindow(list, 16, 70, width - 32, height - 70 - 96, 1);
                    MoveWindow(edit, 16, height - 80, width - 164, 56, 1);
                    MoveWindow(btn, width - 140, height - 80, 124, 56, 1);
                }
                0
            }
            WM_CLOSE => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, msg, w, l),
        }
    }

    unsafe fn refresh() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let status = H_STATUS.load(Ordering::SeqCst) as HWND;
        let list = H_LIST.load(Ordering::SeqCst) as HWND;
        if status.is_null() || list.is_null() {
            return;
        }

        let n_peers = PEERS
            .lock()
            .ok()
            .and_then(|p| p.as_ref().cloned())
            .map(|p| p.lock().map(|s| s.len()).unwrap_or(0))
            .unwrap_or(0);

        let state = if n_peers > 0 { "CONNECTED" } else { "waiting for phone…" };
        let text = format!(
            "ClipBridge  ·  {state}  ({n_peers} peer)  ·  PIN {}  ·  http://127.0.0.1:47825",
            crate::pin()
        );
        SetWindowTextW(status, wide(&text).as_ptr());
        SendMessageW(list, 0x0184, 0, 0);

        let items: Vec<String> = HISTORY
            .lock()
            .ok()
            .and_then(|h| h.as_ref().cloned())
            .map(|h| {
                h.lock()
                    .map(|v| {
                        v.iter()
                            .take(40)
                            .map(|i| {
                                let s = i.at.elapsed().as_secs();
                                let ago = if s < 60 { format!("{s}s") } else { format!("{}m", s / 60) };
                                let dir = if i.from == "pc" { "^" } else { "v" };
                                let one = i.text.replace(['\r', '\n'], " ");
                                let short: String = one.chars().take(110).collect();
                                format!("{dir} {ago:>4}  [{}]  {short}", i.from)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        for it in items {
            SendMessageW(list, 0x0180, 0, wide(&it).as_ptr() as isize);
        }
    }

    unsafe fn save_pin() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let pin = H_PIN.load(Ordering::SeqCst) as HWND;
        if pin.is_null() {
            return;
        }
        let mut buf = [0u16; 32];
        let n = GetWindowTextW(pin, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return;
        }
        let text = String::from_utf16_lossy(&buf[..n as usize]);
        if text.trim().is_empty() {
            return;
        }
        let _ = std::fs::write("pin.txt", text.trim());
        crate::set_pin(text.trim());
    }

    unsafe fn send_from_box() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let edit = H_EDIT.load(Ordering::SeqCst) as HWND;
        if edit.is_null() {
            return;
        }
        let mut buf = [0u16; 4096];
        let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return;
        }
        let text = String::from_utf16_lossy(&buf[..n as usize]);
        if text.trim().is_empty() {
            return;
        }
        SetWindowTextW(edit, wide("").as_ptr());
        if let Ok(h) = HISTORY.lock() {
            if let Some(hist) = h.as_ref() {
                crate::push(hist, text.clone(), "pc".to_string());
            }
        }
        if let Ok(p) = PEERS.lock() {
            if let Some(peers) = p.as_ref() {
                crate::broadcast(peers, crate::TYPE_TEXT, text.as_bytes());
            }
        }
    }

    let hinst = GetModuleHandleW(std::ptr::null());
    let class = wide("ClipBridgeMain");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: std::ptr::null_mut(),
        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
        hbrBackground: 6usize as *mut core::ffi::c_void,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("ClipBridge").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        120,
        80,
        750,
        500,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinst,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        return;
    }
    ShowWindow(hwnd, SW_SHOW);

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
