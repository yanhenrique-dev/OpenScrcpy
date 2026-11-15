/** True inside the Tauri WebView; false in a plain browser preview. */
export function isTauri(): boolean {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** True on desktop Linux (including X11/XWayland), false on Windows/macOS. */
export function isLinux(): boolean {
    if (typeof navigator === 'undefined') return false;
    return /Linux/i.test(navigator.userAgent) && !/Android/i.test(navigator.userAgent);
}
