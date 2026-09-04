use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

const FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(700);

pub fn spawn_app_name_watcher() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        if let Err(e) = watch_app_names(tx.clone()) {
            eprintln!("active window watcher failed: {e}");
            fallback_poll_app_names(tx);
        }
    });

    rx
}

#[cfg(target_os = "linux")]
fn watch_app_names(tx: Sender<String>) -> Result<(), String> {
    if let Some(socket_path) = sway::socket_path() {
        sway::watch_app_names(tx, socket_path)
    } else {
        x11::watch_app_names(tx)
    }
}

#[cfg(target_os = "windows")]
fn watch_app_names(tx: Sender<String>) -> Result<(), String> {
    win::watch_app_names(tx)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn watch_app_names(_tx: Sender<String>) -> Result<(), String> {
    Err("active window events are not implemented on this platform".to_string())
}

fn fallback_poll_app_names(tx: Sender<String>) {
    let mut last_app_name = String::new();

    loop {
        let Ok(active_window) = active_win_pos_rs::get_active_window() else {
            thread::sleep(FALLBACK_POLL_INTERVAL);
            continue;
        };

        if active_window.app_name != last_app_name {
            last_app_name = active_window.app_name;
            if tx.send(last_app_name.clone()).is_err() {
                break;
            }
        }

        thread::sleep(FALLBACK_POLL_INTERVAL);
    }
}

#[cfg(target_os = "linux")]
mod sway {
    use std::{
        fs::{read_dir, read_link},
        os::unix::net::UnixStream,
        path::{Path, PathBuf},
        sync::mpsc::Sender,
        thread,
    };

    use swayipc::{Connection, Event, EventType, Node, WindowChange};

    pub fn socket_path() -> Option<PathBuf> {
        if let Some(socket_path) = std::env::var_os("SWAYSOCK").filter(|value| !value.is_empty()) {
            return Some(socket_path.into());
        }

        let sudo_uid = std::env::var("SUDO_UID").ok()?;
        let socket_prefix = format!("sway-ipc.{sudo_uid}.");
        let runtime_dir = PathBuf::from("/run/user").join(sudo_uid);

        read_dir(runtime_dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                    return false;
                };

                file_name.starts_with(&socket_prefix)
                    && file_name.ends_with(".sock")
                    && UnixStream::connect(path).is_ok()
            })
    }

    pub fn watch_app_names(tx: Sender<String>, socket_path: PathBuf) -> Result<(), String> {
        loop {
            match watch_connection(&tx, &socket_path) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    eprintln!("Sway IPC watcher failed: {error}; retrying");
                    thread::sleep(super::FALLBACK_POLL_INTERVAL);
                }
            }
        }
    }

    fn watch_connection(tx: &Sender<String>, socket_path: &Path) -> Result<(), String> {
        let events = connect(socket_path)?
            .subscribe([EventType::Window])
            .map_err(|e| e.to_string())?;
        if !send_initial_app_name(tx, socket_path)? {
            return Ok(());
        }

        for event in events {
            let Event::Window(event) = event.map_err(|e| e.to_string())? else {
                continue;
            };

            if event.change != WindowChange::Focus {
                continue;
            }

            let Some(app_name) = node_app_name(&event.container) else {
                continue;
            };

            if tx.send(app_name).is_err() {
                return Ok(());
            }
        }

        Ok(())
    }

    fn send_initial_app_name(tx: &Sender<String>, socket_path: &Path) -> Result<bool, String> {
        let tree = connect(socket_path)?
            .get_tree()
            .map_err(|e| e.to_string())?;
        let Some(node) = tree.find_as_ref(|node| node.focused) else {
            return Ok(true);
        };
        let Some(app_name) = node_app_name(node) else {
            return Ok(true);
        };

        Ok(tx.send(app_name).is_ok())
    }

    fn connect(socket_path: &Path) -> Result<Connection, String> {
        UnixStream::connect(socket_path)
            .map(Connection::from)
            .map_err(|e| e.to_string())
    }

    fn node_app_name(node: &Node) -> Option<String> {
        node.app_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                node.window_properties
                    .as_ref()?
                    .class
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| process_name(node.pid?))
    }

    fn process_name(pid: i32) -> Option<String> {
        let process_path = read_link(format!("/proc/{pid}/exe")).ok()?;
        process_path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use std::fs::read_link;
    use std::sync::mpsc::Sender;

    use xcb::x;

    pub fn watch_app_names(tx: Sender<String>) -> Result<(), String> {
        let (conn, screen_num) = xcb::Connection::connect(None).map_err(|e| e.to_string())?;
        let screen = conn
            .get_setup()
            .roots()
            .nth(screen_num as usize)
            .ok_or_else(|| "X11 screen not found".to_string())?;
        let root_window = screen.root();
        let active_window_atom = intern_atom(&conn, b"_NET_ACTIVE_WINDOW", true)?;

        if active_window_atom == x::ATOM_NONE {
            return Err("EWMH _NET_ACTIVE_WINDOW is not supported".to_string());
        }

        conn.check_request(conn.send_request_checked(&x::ChangeWindowAttributes {
            window: root_window,
            value_list: &[x::Cw::EventMask(x::EventMask::PROPERTY_CHANGE)],
        }))
        .map_err(|e| e.to_string())?;

        if let Ok(Some(app_name)) = current_app_name(&conn, root_window, active_window_atom) {
            let _ = tx.send(app_name);
        }

        while let Ok(event) = conn.wait_for_event() {
            let xcb::Event::X(x::Event::PropertyNotify(event)) = event else {
                continue;
            };

            if event.window() != root_window || event.atom() != active_window_atom {
                continue;
            }

            let Ok(Some(app_name)) = current_app_name(&conn, root_window, active_window_atom)
            else {
                continue;
            };

            if tx.send(app_name).is_err() {
                break;
            }
        }

        Ok(())
    }

    fn current_app_name(
        conn: &xcb::Connection,
        root_window: x::Window,
        active_window_atom: x::Atom,
    ) -> Result<Option<String>, String> {
        let active_window = conn.send_request(&x::GetProperty {
            delete: false,
            window: root_window,
            property: active_window_atom,
            r#type: x::ATOM_WINDOW,
            long_offset: 0,
            long_length: 1,
        });
        let active_window = conn
            .wait_for_reply(active_window)
            .map_err(|e| e.to_string())?;
        let Some(active_window) = active_window.value::<x::Window>().first() else {
            return Ok(None);
        };

        get_window_app_name(conn, *active_window).map(Some)
    }

    fn get_window_app_name(conn: &xcb::Connection, window: x::Window) -> Result<String, String> {
        let window_class = get_window_class(conn, window)?;
        let mut process_name = window_class
            .split('\0')
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();

        if let Some(app_name) = process_name.pop() {
            return Ok(app_name.to_string());
        }

        let pid = get_window_pid(conn, window)?;
        let process_path = read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
        let app_name = process_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_string();

        Ok(app_name)
    }

    fn get_window_class(conn: &xcb::Connection, window: x::Window) -> Result<String, String> {
        let window_class = conn.send_request(&x::GetProperty {
            delete: false,
            window,
            property: x::ATOM_WM_CLASS,
            r#type: x::ATOM_STRING,
            long_offset: 0,
            long_length: 1024,
        });
        let window_class = conn
            .wait_for_reply(window_class)
            .map_err(|e| e.to_string())?;

        Ok(String::from_utf8_lossy(window_class.value()).into_owned())
    }

    fn get_window_pid(conn: &xcb::Connection, window: x::Window) -> Result<u32, String> {
        let window_pid_atom = intern_atom(conn, b"_NET_WM_PID", true)?;
        let window_pid = conn.send_request(&x::GetProperty {
            delete: false,
            window,
            property: window_pid_atom,
            r#type: x::ATOM_ANY,
            long_offset: 0,
            long_length: 1,
        });
        let window_pid = conn.wait_for_reply(window_pid).map_err(|e| e.to_string())?;

        Ok(*window_pid.value::<u32>().first().unwrap_or(&0))
    }

    fn intern_atom(
        conn: &xcb::Connection,
        name: &[u8],
        only_if_exists: bool,
    ) -> Result<x::Atom, String> {
        let atom = conn.send_request(&x::InternAtom {
            only_if_exists,
            name,
        });

        Ok(conn.wait_for_reply(atom).map_err(|e| e.to_string())?.atom())
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::sync::{mpsc::Sender, Mutex};

    use windows::Win32::{
        Foundation::{HMODULE, HWND},
        UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, PostQuitMessage, SetWinEventHook, TranslateMessage,
            UnhookWinEvent, EVENT_SYSTEM_FOREGROUND, HWINEVENTHOOK, MSG, WINEVENT_OUTOFCONTEXT,
            WINEVENT_SKIPOWNPROCESS,
        },
    };

    static APP_NAME_TX: Mutex<Option<Sender<String>>> = Mutex::new(None);

    pub fn watch_app_names(tx: Sender<String>) -> Result<(), String> {
        if let Ok(mut app_name_tx) = APP_NAME_TX.lock() {
            *app_name_tx = Some(tx);
        }

        send_current_app_name();

        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                HMODULE(0),
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };

        if hook.0 == 0 {
            return Err("SetWinEventHook failed".to_string());
        }

        let result = message_loop();

        unsafe {
            UnhookWinEvent(hook);
        }

        if let Ok(mut app_name_tx) = APP_NAME_TX.lock() {
            *app_name_tx = None;
        }

        result
    }

    fn message_loop() -> Result<(), String> {
        let mut msg = MSG::default();

        loop {
            let result = unsafe { GetMessageW(&mut msg, HWND(0), 0, 0) };
            match result.0 {
                -1 => return Err("GetMessageW failed".to_string()),
                0 => return Ok(()),
                _ => unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                },
            }
        }
    }

    unsafe extern "system" fn win_event_proc(
        _hook: HWINEVENTHOOK,
        _event: u32,
        _hwnd: HWND,
        _id_object: i32,
        _id_child: i32,
        _event_thread: u32,
        _event_time: u32,
    ) {
        send_current_app_name();
    }

    fn send_current_app_name() {
        let Ok(active_window) = active_win_pos_rs::get_active_window() else {
            return;
        };
        let Ok(tx) = APP_NAME_TX.lock() else {
            return;
        };
        let Some(tx) = tx.as_ref() else {
            return;
        };

        if tx.send(active_window.app_name).is_err() {
            unsafe {
                PostQuitMessage(0);
            }
        }
    }
}
