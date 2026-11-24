import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import AppBehavior from '../AppBehavior.svelte';
import { scrcpy } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';

vi.mock('@tauri-apps/plugin-notification', () => ({
    isPermissionGranted: vi.fn(() => Promise.resolve(true)),
    requestPermission: vi.fn(() => Promise.resolve('granted')),
    sendNotification: vi.fn()
}));

describe('AppBehavior', () => {
    beforeEach(async () => {
        await setLocale('en');
        localStorage.clear();
        scrcpy.backgroundMode = false;
        scrcpy.notificationsEnabled = false;
        scrcpy.config = { ...scrcpy.config, autoReconnect: false };
    });

    it('persists the background preference', async () => {
        render(AppBehavior);

        const toggle = screen.getByRole('switch', {
            name: 'Keep running in the background'
        });
        expect(toggle).toHaveAttribute('aria-checked', 'false');

        await fireEvent.click(toggle);

        expect(scrcpy.backgroundMode).toBe(true);
        expect(localStorage.getItem('scrcpy_background_mode')).toBe('true');
        expect(toggle).toHaveAttribute('aria-checked', 'true');
    });

    it('persists the notification preference', async () => {
        render(AppBehavior);

        const toggle = screen.getByRole('switch', { name: 'Native notifications' });
        expect(toggle).toHaveAttribute('aria-checked', 'false');

        await fireEvent.click(toggle);

        expect(scrcpy.notificationsEnabled).toBe(true);
        expect(localStorage.getItem('scrcpy_notifications')).toBe('true');
        expect(toggle).toHaveAttribute('aria-checked', 'true');
    });

    it('offers automatic reconnection for unexpected session ends', async () => {
        render(AppBehavior);

        const toggle = screen.getByRole('switch', { name: 'Reconnect automatically' });
        expect(toggle).toHaveAttribute('aria-checked', 'false');

        await fireEvent.click(toggle);

        expect(scrcpy.config.autoReconnect).toBe(true);
        expect(toggle).toHaveAttribute('aria-checked', 'true');
    });
});
