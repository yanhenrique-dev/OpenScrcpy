import { availableMonitors, getCurrentWindow, type Monitor } from '@tauri-apps/api/window';
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi';
import { isTauri } from './tauri';
import { loadStoredJSON, safeRemoveItem, safeSetItem } from './storage';

export interface MainWindowState {
    width: number;
    height: number;
    x: number;
    y: number;
    maximized: boolean;
}

const KEY = 'openscrcpy_window_state';
// The key used before 2.0.0 had a typo in the project name. Read it once so
// upgrades keep the window where the user left it, then migrate it.
const LEGACY_KEY = 'opensrcpy_window_state';
const MIN_WIDTH = 600;
const MIN_HEIGHT = 400;

function load(): MainWindowState | null {
    const stored = loadStoredJSON<MainWindowState | null>(KEY, null);
    const legacy = stored ? null : loadStoredJSON<MainWindowState | null>(LEGACY_KEY, null);
    const state = stored ?? legacy;
    if (!state) return null;

    const valid =
        Number.isFinite(state.width) &&
        Number.isFinite(state.height) &&
        Number.isFinite(state.x) &&
        Number.isFinite(state.y) &&
        state.width >= MIN_WIDTH &&
        state.height >= MIN_HEIGHT;
    if (!valid) return null;

    if (legacy) {
        safeSetItem(KEY, JSON.stringify(state));
        safeRemoveItem(LEGACY_KEY);
    }
    return state;
}

/** True when the saved rectangle intersects any connected monitor. Physical
 *  pixels are used consistently by both Tauri and the stored geometry. */
export function isOnAnyMonitor(
    state: MainWindowState,
    monitors: Pick<Monitor, 'position' | 'size'>[]
): boolean {
    if (monitors.length === 0) return false;
    const right = state.x + state.width;
    const bottom = state.y + state.height;
    return monitors.some(monitor => {
        const monitorRight = monitor.position.x + monitor.size.width;
        const monitorBottom = monitor.position.y + monitor.size.height;
        return state.x < monitorRight && right > monitor.position.x
            && state.y < monitorBottom && bottom > monitor.position.y;
    });
}

/** Restore the previous outer size/position and keep saving it as the user
 *  resizes or moves the main window. Returns a cleanup function. */
export function setupMainWindowState(): () => void {
    if (!isTauri()) return () => {};

    const win = getCurrentWindow();
    let disposed = false;
    let cleanup = () => {};
    let timer: ReturnType<typeof setTimeout> | undefined;

    const save = async () => {
        try {
            const maximized = await win.isMaximized();
            if (maximized) {
                const previous = load();
                if (previous) {
                    safeSetItem(KEY, JSON.stringify({ ...previous, maximized: true }));
                }
                return;
            }
            const [size, position] = await Promise.all([win.outerSize(), win.outerPosition()]);
            const state: MainWindowState = {
                width: size.width,
                height: size.height,
                x: position.x,
                y: position.y,
                maximized: false
            };
            safeSetItem(KEY, JSON.stringify(state));
        } catch {
            // Window may be closing or the platform does not expose geometry.
        }
    };

    const scheduleSave = () => {
        if (disposed) return;
        clearTimeout(timer);
        timer = setTimeout(() => void save(), 350);
    };

    void (async () => {
        try {
            const saved = load();
            if (saved) {
                await win.setSize(new PhysicalSize(saved.width, saved.height));
                // Never restore onto a monitor that is no longer connected:
                // the window would open completely off-screen. If the monitor
                // list cannot be read, keep Tauri's centered default position.
                let visible = false;
                try {
                    visible = isOnAnyMonitor(saved, await availableMonitors());
                } catch {
                    visible = false;
                }
                if (visible) await win.setPosition(new PhysicalPosition(saved.x, saved.y));
                if (saved.maximized) await win.maximize();
            }

            if (disposed) return;
            const unlistenResize = await win.onResized(scheduleSave);
            const unlistenMove = await win.onMoved(scheduleSave);
            cleanup = () => {
                unlistenResize();
                unlistenMove();
                clearTimeout(timer);
            };
        } catch {
            // Preview mode / unsupported window manager: keep the fixed size.
        }
    })();

    return () => {
        disposed = true;
        cleanup();
    };
}
