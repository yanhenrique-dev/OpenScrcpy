//! KDE Plasma / KWin window integration.
//!
//! Wayland deliberately does not let a client query or place its own windows:
//! the compositor owns the geometry. That breaks the "remember window
//! position" feature and the recentre shortcut, which use `xdotool` on X11.
//!
//! KWin exposes a scripting interface over D-Bus instead: the app loads a
//! small JavaScript file into the compositor, the script reports the mirror
//! window's client-area position back to a tiny D-Bus service owned by the
//! app, and the same script can also move the window when the user recentres
//! it. Everything here is best effort and only active on a KDE Wayland
//! session; on X11 `xdotool` remains the primary path.

use std::collections::HashMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::interface;

/// Well-known name owned by the app while it runs.
const DBUS_NAME: &str = "dev.yanhenrique.openscrcpy.WindowPositions";
const DBUS_PATH: &str = "/dev/yanhenrique/openscrcpy/WindowPositions";
const DBUS_INTERFACE: &str = "dev.yanhenrique.openscrcpy.WindowPositions";

/// Latest client-area origin reported per scrcpy pid.
static POSITIONS: OnceLock<Mutex<HashMap<u32, (i32, i32)>>> = OnceLock::new();
/// Keeps the service connection (and its blocking executor) alive.
static SERVICE: OnceLock<Connection> = OnceLock::new();
static INIT: OnceLock<()> = OnceLock::new();

fn positions() -> &'static Mutex<HashMap<u32, (i32, i32)>> {
    POSITIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn locked_positions() -> std::sync::MutexGuard<'static, HashMap<u32, (i32, i32)>> {
    positions().lock().unwrap_or_else(|e| e.into_inner())
}

struct PositionServer;

#[interface(name = "dev.yanhenrique.openscrcpy.WindowPositions")]
impl PositionServer {
    /// Called by the KWin script. `token` is the scrcpy pid as a string and
    /// the coordinates are the client-area origin (`clientGeometry`), which
    /// matches the semantics used by the existing position cache.
    fn report(&self, token: String, x: i32, y: i32) {
        if let Ok(pid) = token.parse::<u32>() {
            locked_positions().insert(pid, (x, y));
        }
    }
}

/// True only for a KDE Wayland session. X11 keeps using xdotool and does not
/// need the D-Bus service or the scripts.
fn is_kwin_wayland() -> bool {
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|value| value.to_ascii_lowercase().contains("kde"));
    kde && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Starts the position service once per process. Non-fatal: without a session
/// bus (or if the name is taken), Wayland position memory stays disabled.
pub fn init() {
    if !is_kwin_wayland() || INIT.set(()).is_err() {
        return;
    }
    std::thread::spawn(|| {
        let builder = match zbus::blocking::connection::Builder::session() {
            Ok(builder) => builder,
            Err(error) => {
                eprintln!("[kwin] no session bus: {error}");
                return;
            }
        };
        let connection = match builder
            .name(DBUS_NAME)
            .and_then(|builder| builder.serve_at(DBUS_PATH, PositionServer))
            .and_then(|builder| builder.build())
        {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("[kwin] position service unavailable: {error}");
                return;
            }
        };
        let _ = SERVICE.set(connection);
    });
}

/// Latest known position for `pid`, populated asynchronously by the KWin
/// script. Returns `None` until the first report arrives (or on non-KDE).
pub fn cached_position(pid: u32) -> Option<(i32, i32)> {
    locked_positions().get(&pid).copied()
}

fn kwin_proxy() -> Option<Proxy<'static>> {
    let connection = Connection::session().ok()?;
    Proxy::new_owned(
        connection,
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
    )
    .ok()
}

/// Private directory for the scripts KWin reads. `XDG_RUNTIME_DIR` is owned by
/// the user with mode 0700, so another local user cannot pre-create a symlink
/// with our script name.
fn script_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")?;
    let dir = PathBuf::from(base).join("openscrcpy-kwin");
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => Some(dir),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Some(dir),
        Err(error) => {
            eprintln!(
                "[kwin] could not create script directory {}: {error}",
                dir.display()
            );
            None
        }
    }
}

fn script_path(name: &str) -> Option<PathBuf> {
    script_dir().map(|dir| dir.join(format!("{name}.js")))
}

/// Loads and runs a KWin script. Returns false if KWin is unreachable or the
/// script could not be registered.
fn run_script(name: &str, body: &str) -> bool {
    let Some(proxy) = kwin_proxy() else {
        eprintln!("[kwin] cannot reach the KWin scripting interface");
        return false;
    };
    // Drop a stale copy from a previous run before loading the fresh script.
    let _: Result<bool, _> = proxy.call("unloadScript", &(name,));
    let Some(path) = script_path(name) else {
        eprintln!("[kwin] XDG_RUNTIME_DIR is not set; cannot write {name}");
        return false;
    };
    if let Err(error) = std::fs::write(&path, body) {
        eprintln!("[kwin] could not write {}: {error}", path.display());
        return false;
    }
    let load_path = path.to_string_lossy().to_string();
    let loaded: Result<i32, _> = proxy.call("loadScript", &(load_path.as_str(), name));
    if let Err(error) = loaded {
        eprintln!("[kwin] loadScript {name} failed: {error}");
        let _ = std::fs::remove_file(&path);
        return false;
    }
    // KWin registers the script immediately but reads/compiles the file on
    // its own event loop; calling `start` in the same breath can run no
    // scripts at all (observed on Plasma 6). Let the loader catch up.
    std::thread::sleep(Duration::from_millis(150));
    let started: Result<(), _> = proxy.call("start", &());
    if let Err(error) = &started {
        eprintln!("[kwin] start {name} failed: {error}");
    }
    started.is_ok()
}

fn unload_script(name: &str) {
    if let Some(proxy) = kwin_proxy() {
        let _: Result<bool, _> = proxy.call("unloadScript", &(name,));
    }
    if let Some(path) = script_path(name) {
        let _ = std::fs::remove_file(path);
    }
    // Clean up the predictable temp file left by older builds.
    let _ = std::fs::remove_file(std::env::temp_dir().join(format!("{name}.js")));
}

/// Loads a script that reports the mirror window's position on creation and on
/// every move. When `position` is set, the script also applies it once the
/// window exists, replacing the Wayland-ignored `--window-x/--window-y`.
pub fn track_window(pid: u32, position: Option<(i32, i32)>) {
    if pid == 0 || !is_kwin_wayland() {
        return;
    }
    std::thread::spawn(move || {
        let name = format!("openscrcpy-pos-{pid}");
        let _ = run_script(&name, &track_script(pid, position));
    });
}

/// Stops reporting for `pid` and drops its cached position. Blocking D-Bus
/// call, but callers are the session monitor and teardown, never the UI thread.
pub fn stop_tracking(pid: u32) {
    if pid == 0 {
        return;
    }
    locked_positions().remove(&pid);
    if is_kwin_wayland() {
        unload_script(&format!("openscrcpy-pos-{pid}"));
    }
}

/// Asks KWin to centre the mirror window on its current screen and raise it.
/// One-shot script: it unloads itself a moment after running.
pub fn recenter(pid: u32) {
    if pid == 0 || !is_kwin_wayland() {
        return;
    }
    std::thread::spawn(move || {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let name = format!("openscrcpy-recenter-{pid}-{stamp}");
        if run_script(&name, &recenter_script(pid)) {
            // The script is asynchronous inside KWin; give it time to apply
            // the geometry before removing it.
            std::thread::sleep(Duration::from_secs(2));
            unload_script(&name);
        }
    });
}

fn track_script(pid: u32, position: Option<(i32, i32)>) -> String {
    let (has_target, target_x, target_y) = match position {
        Some((x, y)) => ("true", x, y),
        None => ("false", 0, 0),
    };
    format!(
        r#"(function () {{
    var target = {pid};
    var token = "{pid}";
    var hasTarget = {has_target};
    var targetX = {target_x};
    var targetY = {target_y};

    function report(w) {{
        // KWin marshals integral JS numbers as D-Bus int32; round here so the
        // call always matches the (sii) service signature, including on
        // fractional HiDPI coordinates.
        callDBus("{DBUS_NAME}", "{DBUS_PATH}", "{DBUS_INTERFACE}",
                "Report", token,
                Math.round(w.clientGeometry.x), Math.round(w.clientGeometry.y));
    }}

    var applied = false;
    function applyTarget(w) {{
        if (!hasTarget || applied || w.frameGeometry.width <= 0 || w.frameGeometry.height <= 0) {{
            return;
        }}
        applied = true;
        var dx = w.clientGeometry.x - w.frameGeometry.x;
        var dy = w.clientGeometry.y - w.frameGeometry.y;
        w.frameGeometry = {{
            x: Math.round(targetX - dx),
            y: Math.round(targetY - dy),
            width: w.frameGeometry.width,
            height: w.frameGeometry.height
        }};
    }}

    function attach(w) {{
        applyTarget(w);
        report(w);
        w.frameGeometryChanged.connect(function () {{
            applyTarget(w);
            report(w);
        }});
    }}

    var wins = workspace.windowList();
    for (var i = 0; i < wins.length; i++) {{
        if (wins[i].pid === target) {{
            attach(wins[i]);
            return;
        }}
    }}
    workspace.windowAdded.connect(function (w) {{
        if (w.pid === target) {{
            attach(w);
        }}
    }});
}})();
"#
    )
}

fn recenter_script(pid: u32) -> String {
    format!(
        r"(function () {{
    var target = {pid};
    var wins = workspace.windowList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if (w.pid !== target) {{
            continue;
        }}
        var area = workspace.clientArea(KWin.PlacementArea, w);
        w.frameGeometry = {{
            x: Math.round(area.x + (area.width - w.frameGeometry.width) / 2),
            y: Math.round(area.y + (area.height - w.frameGeometry.height) / 2),
            width: w.frameGeometry.width,
            height: w.frameGeometry.height
        }};
        workspace.activeWindow = w;
        return;
    }}
}})();
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_caches_by_pid() {
        PositionServer.report("424242".to_string(), 10, 21);
        assert_eq!(cached_position(424_242), Some((10, 21)));
    }

    #[test]
    fn track_script_applies_and_reports_the_requested_position() {
        let body = track_script(1234, Some((100, 200)));
        assert!(body.contains("var target = 1234;"));
        assert!(body.contains("var token = \"1234\";"));
        assert!(body.contains("var hasTarget = true;"));
        assert!(body.contains("var targetX = 100;"));
        assert!(body.contains("var targetY = 200;"));
        assert!(body.contains("frameGeometryChanged.connect"));
        assert!(body.contains("callDBus"));
    }

    #[test]
    fn track_script_without_target_only_reports() {
        let body = track_script(1234, None);
        assert!(body.contains("var hasTarget = false;"));
        assert!(body.contains("callDBus"));
    }

    #[test]
    fn recenter_script_centres_and_raises_the_window() {
        let body = recenter_script(1234);
        assert!(body.contains("var target = 1234;"));
        assert!(body.contains("workspace.clientArea(KWin.PlacementArea, w)"));
        assert!(body.contains("workspace.activeWindow = w"));
    }

    /// End-to-end check against a live KDE Wayland session. Ignored by default
    /// because it needs a visible window:
    ///
    /// ```text
    /// TEST_WINDOW_PID=<pid> cargo test kwin_reports -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "requires a live KDE Wayland session and TEST_WINDOW_PID"]
    fn kwin_reports_a_real_window_position() {
        let pid: u32 = std::env::var("TEST_WINDOW_PID")
            .expect("TEST_WINDOW_PID must point at a visible window")
            .parse()
            .expect("TEST_WINDOW_PID must be a number");
        init();
        // Give the service thread time to acquire its bus name.
        std::thread::sleep(Duration::from_millis(600));
        track_window(pid, None);
        for _ in 0..20 {
            if let Some(position) = cached_position(pid) {
                println!("reported position: {position:?}");
                stop_tracking(pid);
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        stop_tracking(pid);
        panic!("KWin did not report a position for pid {pid}");
    }
}
