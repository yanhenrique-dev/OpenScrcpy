import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { scrcpy } from '../scrcpy.svelte';
import { alert, closeAlert } from '../alerts.svelte';
import { setLocale } from '../../i18n';

const mockInvoke = vi.hoisted(() => vi.fn());
const mockIsPermissionGranted = vi.hoisted(() => vi.fn());
const mockRequestPermission = vi.hoisted(() => vi.fn());
const mockSendNotification = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('@tauri-apps/api/event', () => ({
    listen: vi.fn(() => Promise.resolve(() => {}))
}));
vi.mock('@tauri-apps/api/app', () => ({
    getVersion: vi.fn(() => Promise.resolve('2.0.0'))
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-notification', () => ({
    isPermissionGranted: mockIsPermissionGranted,
    requestPermission: mockRequestPermission,
    sendNotification: mockSendNotification
}));

describe('background mode sync', () => {
    beforeEach(async () => {
        await setLocale('en');
        closeAlert();
        localStorage.clear();
        mockInvoke.mockReset().mockResolvedValue(null);
        mockIsPermissionGranted.mockReset().mockResolvedValue(true);
        mockRequestPermission.mockReset().mockResolvedValue('granted');
        mockSendNotification.mockReset();
        (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
        scrcpy.ready = true;
        scrcpy.backgroundMode = false;
        scrcpy.notificationsEnabled = false;
    });

    afterEach(() => {
        delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
        scrcpy.ready = false;
    });

    it('sends the localized tray labels to the backend', async () => {
        scrcpy.backgroundMode = true;
        scrcpy.syncBackgroundMode(true);

        await vi.waitFor(() => {
            expect(mockInvoke).toHaveBeenCalledWith('configure_background_mode', {
                enabled: true,
                showLabel: 'Show OpenScrcpy',
                quitLabel: 'Quit OpenScrcpy'
            });
        });
    });

    it('reverts the preference when the tray cannot be created', async () => {
        mockInvoke.mockRejectedValueOnce('system tray is unavailable');
        scrcpy.backgroundMode = true;

        scrcpy.syncBackgroundMode(true);

        await vi.waitFor(() => expect(scrcpy.backgroundMode).toBe(false));
        expect(alert.isOpen).toBe(true);
        expect(alert.title).toBe('Background Mode Unavailable');
    });

    it('enables notifications and sends a confirmation', async () => {
        await scrcpy.setNotificationsEnabled(true);

        expect(scrcpy.notificationsEnabled).toBe(true);
        expect(mockSendNotification).toHaveBeenCalledOnce();
    });

    it('reverts the notification preference when permission is denied', async () => {
        mockIsPermissionGranted.mockResolvedValueOnce(false);
        mockRequestPermission.mockResolvedValueOnce('denied');

        await scrcpy.setNotificationsEnabled(true);

        expect(scrcpy.notificationsEnabled).toBe(false);
        expect(alert.isOpen).toBe(true);
        expect(alert.title).toBe('Notifications Blocked');
    });
});
