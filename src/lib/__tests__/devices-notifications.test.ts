import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DevicesStore } from '../stores/devices.svelte';
import { LogStore } from '../stores/logs.svelte';
import { SettingsStore } from '../stores/settings.svelte';
import { NotificationService } from '../notifications';
import { setLocale } from '../../i18n';

const mockInvoke = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('@tauri-apps/plugin-notification', () => ({
    isPermissionGranted: vi.fn(() => Promise.resolve(true)),
    requestPermission: vi.fn(() => Promise.resolve('granted')),
    sendNotification: vi.fn()
}));

describe('DevicesStore notification baseline', () => {
    let devices: string[];

    beforeEach(async () => {
        await setLocale('en');
        localStorage.clear();
        devices = ['phone1'];
        mockInvoke.mockReset().mockImplementation((command: string) => {
            if (command === 'get_devices') {
                return Promise.resolve({ devices });
            }
            if (command === 'get_mdns_devices') {
                return Promise.resolve({ services: [] });
            }
            if (command === 'adb_shell') {
                return Promise.resolve({ success: false });
            }
            return Promise.resolve(null);
        });
    });

    it('does not notify for devices present on the first refresh', async () => {
        const settings = new SettingsStore();
        const notifications = new NotificationService(settings);
        const added = vi.spyOn(notifications, 'deviceAdded');
        const store = new DevicesStore(settings, new LogStore(), notifications);

        await store.fetchDevicesOnce();
        expect(added).not.toHaveBeenCalled();

        devices = ['phone1', 'phone2'];
        await store.fetchDevicesOnce();
        expect(added).toHaveBeenCalledWith('phone2');
    });

    it('notifies when a device disappears after the baseline', async () => {
        const settings = new SettingsStore();
        const notifications = new NotificationService(settings);
        const removed = vi.spyOn(notifications, 'deviceRemoved');
        const store = new DevicesStore(settings, new LogStore(), notifications);

        await store.fetchDevicesOnce();
        devices = [];
        await store.fetchDevicesOnce();

        expect(removed).toHaveBeenCalledWith('phone1');
    });
});
