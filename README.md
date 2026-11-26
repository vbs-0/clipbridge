# ClipBridge

Copy on your PC, paste on your phone. Copy on your phone, paste on your PC. Text and screenshots, over your own Wi-Fi, encrypted, with no cloud account and no sync service in the middle.

A Rust tray app for Windows and a 46 KB Kotlin app for Android.

| | |
|---|---|
| PC | Rust, Win32 GUI, ~950 KB binary |
| Android | Kotlin, no libraries, ~46 KB APK per ABI |
| Transport | UDP discovery + TCP sync on the LAN, AES-256-GCM |
| Server | none — the two devices talk directly |

## What it does

- **Text both ways** — copy on one device, it appears in the other's clipboard
- **Images both ways** — a screenshot on the PC lands in the phone's gallery; a picture on the phone can be sent to the PC as a PNG
- **Android share sheet** — send any text or image to ClipBridge from any app
- **ClipBridge keyboard** — an input method that reads the clipboard in the background, because Android only lets the focused app read it
- **History and manual send** — the PC's dashboard lists what went across and lets you send an entry back
- **Third-party push** — any app can broadcast `com.vbs.clipbridge.PUSH` with a `text` extra

## How it works

```
        UDP 47823  discovery (broadcast + unicast reply)
PC  <------------------------------>  Android
        TCP 47824  sync (length-prefixed frames)
        HTTP 47825 dashboard (PC only)
```

- **Discovery** — the PC broadcasts on 47823; Android replies by unicast, because broadcast is often filtered on phones. mDNS is also used to find peers by name.
- **Sync** — TCP 47824. Both sides listen *and* dial: the PC connects outbound so Windows firewall never has to allow an inbound rule, and the phone accepts the connection. A frame is a length prefix followed by one UTF-8 message.
- **Encryption** — key = `SHA-256("clipbridge:" + PIN)`; each frame is a 12-byte random nonce followed by AES-256-GCM ciphertext. The PIN is the only secret.
- **Clipboard on Windows** — `WM_CLIPBOARDUPDATE`, so the app wakes only when the clipboard actually changes; idle CPU is zero.

## Pairing

Both sides need the same PIN.

- PC: set `CLIPBRIDGE_PIN` in the environment, or use the field in the window (saved to `pin.txt`)
- Android: enter it in the app; it is stored in SharedPreferences

If the PINs differ, frames fail to decrypt and are dropped — that is the whole authentication model, so pick something long enough to matter.

## Build

**PC** (Rust 1.80+):

```sh
cd pc
cargo build --release          # target/release/clipbridge.exe
```

On Windows without the MSVC build tools, use the GNU toolchain:

```sh
rustup toolchain install stable-x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnu build --release
```

**Android** (JDK 17, Android SDK 35):

```sh
cd android
echo 'sdk.dir=/path/to/Android/Sdk' > local.properties
./gradlew assembleRelease      # app/build/outputs/apk/release/
```

Release builds are signed with the debug key unless you add a `keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`) next to `android/settings.gradle.kts`.

## Layout

```
pc/src/main.rs     discovery, sync protocol, crypto, clipboard, PNG, dashboard
pc/src/gui.rs      the Win32 window: link status, history, PIN field
android/.../Net.kt        sockets, discovery, clipboard watcher
android/.../Crypto.kt     SHA-256 key derivation, AES-GCM frames
android/.../ClipIME.kt    input method for background clipboard reads
android/.../ImageSink.kt  saves incoming images to MediaStore
android/.../History.kt    last N items
android/.../PushReceiver.kt  broadcast receiver for third-party pushes
```

## Notes on this repository

- Both halves were built and run against a real phone (Android 17) and a Windows 11 PC on the same Wi-Fi.
- The PC source contained a literal NUL byte inside the window-class string (`"ClipBridgeWnd\0"`); it is now the proper `\0` escape, which also stops editors treating `main.rs` as a binary.
- Release signing is optional now instead of hardcoded to the debug key.

## Limits

- LAN only, and the HTTP dashboard on 47825 has **no authentication** — it is bound to all interfaces, so anyone on the same network can read the history. Bind it to `127.0.0.1` or add a token before using this on a network you do not control.
- The PIN is stored in plain text on both sides (`pin.txt`, SharedPreferences).
- Android needs the keyboard enabled for background clipboard reads; without it, reads happen only while the app is open.
- No file sync beyond images, no clipboard history on the phone side beyond the last items.
