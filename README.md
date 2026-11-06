# rawhid-rust

PC companion for the QMK `keyballish/keyball39:default` keymap. Run one daemon
on each PC connected through the KVM. It sends the focused application name,
answers datetime requests, and transfers a plain-text clipboard slot stored in
the keyboard's RAM.

## Clipboard

With matching keyboard firmware and this daemon running:

1. Copy text normally in the source application.
2. Press **UTIL + C** (`MC_CSTR`) to store the current clipboard on the board.
3. Switch PCs if desired, then press **UTIL + V** (`MC_CPST`). Release the
   trigger and modifiers to complete the paste.

The slot holds the first **100 Unicode scalar values** (at most 400 UTF-8 bytes),
including Japanese, newlines, and emoji. Combined emoji and combining marks may
count as multiple values. NUL is rejected. Empty text replaces the slot with an
empty string. Failed or interrupted uploads retain the old slot. A powered KVM
switch retains the slot; power loss/reset clears it. There is no flash write.

Pasting replaces the destination PC's clipboard. The firmware waits for a
successful clipboard update before emitting Command+V on macOS, Ctrl+Shift+V
for recognized terminals, or Ctrl+V elsewhere. Customized application shortcuts
may need adjustment in `clipboard_hid.c:terminal_application()`. Another key
press, USB reset/suspend, or a 3-second deadline cancels a pending paste.
A copy/store only captures the **existing clipboard**, not the current selection.

Clipboard helpers:

- Linux Wayland: `wl-copy`, `wl-paste` from `wl-clipboard`.
- Linux X11: `xclip` with `UTF8_STRING` and the `CLIPBOARD` selection.
- macOS: built-in `pbcopy`, `pbpaste`.
- Windows: built-in Windows PowerShell `Get-Clipboard` / `Set-Clipboard`.

For an existing `sudo rawhid-rust` launcher, Linux clipboard commands run as
`SUDO_UID` via `sudo -n -u '#uid'`, with the user's runtime directory and a
Wayland socket discovered if the environment was stripped. Prefer running as
the desktop user with HID device permissions configured. Desktop/session access
and clipboard ownership are required on each PC. Clipboard helpers run outside
the HID loop, have a 2-second deadline and a 1 MiB read limit, and never log text.
Linux is build/test verified; macOS/Windows helpers need testing on those hosts.

## KVM and application synchronization

A single focus watcher stays alive while USB is disconnected. Pending events
are coalesced to the latest name. On HID reconnection that name is immediately
resent; a 1-second refresh also repairs stale keyboard state when the KVM keeps
the host-side HID handle alive and the focused application has not changed.
USB open retries occur every 500 ms. Watcher errors retry independently of USB.
Application-name and datetime report IDs (`0x01`, `0x02`) are unchanged, so this
sync fix also works with older firmware.

The old implementation recreated watchers on every USB reconnect and only sent
changed names. That could leave the other PC's application profile on a powered
keyboard after switching back to an unchanged app. Also, hidapi writes now always
contain report ID zero plus **32 payload bytes** on every OS; short writes fail.
The raw interface is selected by VID/PID **and** usage page `0xff60` / usage `0x61`.
Linux uses an abstract socket lock to reject additional updated daemon instances;
older versions do not participate, so stop the old process when upgrading.

Inspect focus events without opening the keyboard:

```sh
cargo run -- --watch-app-names
```

This does not confirm the physical KVM's power behavior or its Raw HID support.
A KVM that disconnects board power clears the slot; a KVM that exposes only boot
keyboard reports cannot carry this protocol.

## Build and verify

On macOS, run as the logged-in desktop user. Application detection uses
`NSWorkspace` activation notifications on the main run loop, keeping USB and
clipboard work on background threads. It reads the application's localized name
directly, without enumerating windows, polling focus, or requesting Screen
Recording / Accessibility access. Temporary Cocoa objects are drained after
each run-loop event. The watcher remains active while the keyboard is absent.
The HID device is opened in non-exclusive mode so the OS can continue receiving
normal keyboard and mouse input.

Build an application bundle on the Mac with:

```sh
bash build-app.sh
open "RawHID Rust.app"
```

The bundle runs without a Dock icon. Stop any previously running copy before
starting another. For focus diagnostics use `cargo run -- --watch-app-names`.
macOS runtime behavior, clipboard access, and physical USB/KVM operation must
be verified on a Mac; Linux-only tests do not exercise the AppKit backend.

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

Replace/restart the existing daemon with `target/release/rawhid-rust` and flash
both keyboard halves with the matching firmware to enable clipboard keys.
The firmware also contains host C regression tests; see its keymap README.

## Wire protocol

All packets below are exactly 32 bytes at QMK. For `hidapi::write`, prefix one
zero report-ID byte (33 bytes total); unnumbered input reports have no prefix.
Integers are little-endian. Padding is zero and is not part of the text.

| Event | Direction | Payload after event byte |
| --- | --- | --- |
| `0x01` application | PC → board | UTF-8 application name, at most 31 bytes |
| `0x02` datetime | Either | Request or ASCII datetime response |
| `0x10` store request | Board → PC | token (u8) |
| `0x11` store begin | PC → board | token, total bytes (u16) |
| `0x12` store data | PC → board | token, offset (u16), count (u8), up to 27 bytes |
| `0x13` store commit | PC → board | token |
| `0x14` store status | Board → PC | token, status (0 success / 1 rejected) |
| `0x15` paste begin | Board → PC | token, total bytes (u16) |
| `0x16` paste data | Board → PC | token, offset (u16), count (u8), up to 27 bytes |
| `0x17` paste commit | Board → PC | token |
| `0x18` paste ready | PC → board | token, status (0 success / 1 failed) |

The board increments the token for every new operation. Only an in-order,
complete, valid UTF-8 upload of at most 100 scalar values commits. UTF-8 may span
packet boundaries. Paste packets are emitted at most once per 4 ms from the
scan task. Stale PC worker completions are filtered by USB connection generation
and token; a disconnected operation is never replayed on a new connection.
