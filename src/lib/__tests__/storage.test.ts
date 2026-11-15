import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
    loadStoredJSON,
    loadStoredStringArray,
    safeGetItem,
    safeRemoveItem,
    safeSetItem,
    sanitizeWindowPositions,
    storageSnapshot
} from '../storage';

describe('safe storage helpers', () => {
    beforeEach(() => localStorage.clear());
    afterEach(() => vi.restoreAllMocks());

    it('reads and writes values', () => {
        safeSetItem('key', 'value');
        expect(safeGetItem('key')).toBe('value');
        safeRemoveItem('key');
        expect(safeGetItem('key')).toBeNull();
    });

    it('falls back when JSON is corrupt', () => {
        safeSetItem('broken', '{not-json');
        expect(loadStoredJSON('broken', { ok: true })).toEqual({ ok: true });
    });

    it('does not throw when storage writes are blocked', () => {
        vi.spyOn(localStorage, 'setItem').mockImplementation(() => {
            throw new DOMException('quota exceeded', 'QuotaExceededError');
        });

        expect(() => safeSetItem('key', 'value')).not.toThrow();
    });

    it('exports only allowlisted keys', () => {
        safeSetItem('scrcpy_config', '{"x":1}');
        safeSetItem('scrcpy_history', '["private"]');
        safeSetItem('other', 'secret');

        expect(storageSnapshot(['scrcpy_config'])).toEqual({
            scrcpy_config: '{"x":1}'
        });
    });

    it('redacts device and personal paths from the diagnostics config', () => {
        safeSetItem(
            'scrcpy_config',
            JSON.stringify({
                device: 'emulator-5554',
                scrcpyPath: '/home/alice/scrcpy',
                recordPath: '/home/alice/Videos',
                bitrate: 8
            })
        );

        const exported = JSON.parse(storageSnapshot(['scrcpy_config']).scrcpy_config);

        expect(exported.device).toBe('<redacted>');
        expect(exported.scrcpyPath).toBe('<redacted>');
        expect(exported.recordPath).toBe('<redacted>');
        expect(exported.bitrate).toBe(8);
    });

    it('redacts the full snapshot too and never exports device history', () => {
        safeSetItem('scrcpy_history', JSON.stringify(['192.168.0.5:5555']));
        safeSetItem(
            'scrcpy_config',
            JSON.stringify({ device: '192.168.0.5:5555', scrcpyPath: '/home/alice/scrcpy' })
        );

        const snapshot = storageSnapshot();
        const config = JSON.parse(snapshot.scrcpy_config);

        expect(snapshot.scrcpy_history).toBeUndefined();
        expect(config.device).toBe('<redacted>');
        expect(config.scrcpyPath).toBe('<redacted>');
    });

    it('rejects malformed stored arrays and window positions', () => {
        safeSetItem('history', JSON.stringify({ not: 'an array' }));
        safeSetItem('positions', JSON.stringify({
            good: { x: 10, y: 20 },
            bad: { x: 'ten', y: 20 },
            nil: null
        }));

        expect(loadStoredStringArray('history', ['fallback'])).toEqual(['fallback']);
        expect(sanitizeWindowPositions(JSON.parse(safeGetItem('positions') ?? '{}'))).toEqual({
            good: { x: 10, y: 20 }
        });
        expect(sanitizeWindowPositions(null)).toEqual({});
        expect(sanitizeWindowPositions([])).toEqual({});
    });
});
