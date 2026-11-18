import { beforeEach, describe, expect, it } from 'vitest';
import { SettingsStore } from '../stores/settings.svelte';

describe('SettingsStore background mode', () => {
    beforeEach(() => {
        localStorage.clear();
    });

    it('defaults to off', () => {
        const settings = new SettingsStore();
        settings.load();
        expect(settings.backgroundMode).toBe(false);
    });

    it('persists and reloads the preference', () => {
        const settings = new SettingsStore();
        settings.setBackgroundMode(true);

        const reloaded = new SettingsStore();
        reloaded.load();
        expect(reloaded.backgroundMode).toBe(true);

        reloaded.setBackgroundMode(false);
        const again = new SettingsStore();
        again.load();
        expect(again.backgroundMode).toBe(false);
    });

    it('treats malformed stored values as off', () => {
        localStorage.setItem('scrcpy_background_mode', 'yes');
        const settings = new SettingsStore();
        settings.load();
        expect(settings.backgroundMode).toBe(false);
    });

    it('persists notification preferences separately', () => {
        const settings = new SettingsStore();
        settings.setNotificationsEnabled(true);

        const reloaded = new SettingsStore();
        reloaded.load();
        expect(reloaded.notificationsEnabled).toBe(true);
        expect(reloaded.backgroundMode).toBe(false);

        reloaded.setNotificationsEnabled(false);
        const again = new SettingsStore();
        again.load();
        expect(again.notificationsEnabled).toBe(false);
    });
});
