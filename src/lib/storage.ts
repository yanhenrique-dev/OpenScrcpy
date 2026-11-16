/** Defensive localStorage helpers. Storage can throw in private mode, when the
 *  quota is full, or when the webview disables it; none of those should break
 *  the UI. */
import type { WindowPositions } from './types';

let storageWarned = false;

function warnOnce(error: unknown) {
    if (storageWarned) return;
    storageWarned = true;
    console.warn('localStorage unavailable; changes stay in memory for this session.', error);
}

export function safeGetItem(key: string): string | null {
    try {
        return localStorage.getItem(key);
    } catch (e) {
        warnOnce(e);
        return null;
    }
}

export function safeSetItem(key: string, value: string): void {
    try {
        localStorage.setItem(key, value);
    } catch (e) {
        warnOnce(e);
    }
}

export function safeRemoveItem(key: string): void {
    try {
        localStorage.removeItem(key);
    } catch (e) {
        warnOnce(e);
    }
}

export function loadStoredJSON<T>(key: string, fallback: T): T {
    try {
        const raw = safeGetItem(key);
        return raw ? (JSON.parse(raw) as T) : fallback;
    } catch (e) {
        console.error(`Failed to parse ${key}`, e);
        return fallback;
    }
}

/** Parses a stored string array, rejecting objects/strings/null and
 *  non-string entries. */
export function loadStoredStringArray(key: string, fallback: string[] = []): string[] {
    const value = loadStoredJSON<unknown>(key, fallback);
    return Array.isArray(value)
        ? value.filter((entry): entry is string => typeof entry === 'string')
        : fallback;
}

/** Rebuilds window positions from untrusted storage, dropping malformed
 *  entries and prototype-pollution keys. */
export function sanitizeWindowPositions(value: unknown): WindowPositions {
    const out: WindowPositions = {};
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return out;
    for (const [key, pos] of Object.entries(value)) {
        if (key === '__proto__' || key === 'constructor' || key === 'prototype') continue;
        if (typeof pos !== 'object' || pos === null || Array.isArray(pos)) continue;
        const x = (pos as { x?: unknown }).x;
        const y = (pos as { y?: unknown }).y;
        if (typeof x === 'number' && Number.isFinite(x) && typeof y === 'number' && Number.isFinite(y)) {
            out[key] = { x, y };
        }
    }
    return out;
}

/** Remove device identifiers and personal paths from a diagnostics snapshot.
 *  The config shape is intentionally kept so support can still see settings. */
function redactStoredValue(key: string, value: string): string {
    if (key !== 'scrcpy_config') return value;
    try {
        const parsed: unknown = JSON.parse(value);
        if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return value;
        const config = parsed as Record<string, unknown>;
        for (const field of ['device', 'scrcpyPath', 'recordPath']) {
            if (field in config) config[field] = '<redacted>';
        }
        return JSON.stringify(config);
    } catch {
        return value;
    }
}

/** Snapshot for diagnostics. Device history is never included, and sensitive
 *  fields of the config (serial, custom paths) are redacted. */
export function storageSnapshot(allowlist?: string[]): Record<string, string> {
    const out: Record<string, string> = {};
    try {
        if (allowlist) {
            for (const key of allowlist) {
                const value = safeGetItem(key);
                if (value !== null) out[key] = redactStoredValue(key, value);
            }
            return out;
        }
        for (let i = 0; i < localStorage.length; i++) {
            const key = localStorage.key(i);
            if (!key || key === 'scrcpy_history') continue;
            const value = safeGetItem(key);
            if (value !== null) out[key] = redactStoredValue(key, value);
        }
    } catch (e) {
        warnOnce(e);
    }
    return out;
}
