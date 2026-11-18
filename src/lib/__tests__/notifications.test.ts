import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { NotificationService, cleanText } from '../notifications';
import { setLocale } from '../../i18n';

const mockIsPermissionGranted = vi.hoisted(() => vi.fn());
const mockRequestPermission = vi.hoisted(() => vi.fn());
const mockSendNotification = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/plugin-notification', () => ({
    isPermissionGranted: mockIsPermissionGranted,
    requestPermission: mockRequestPermission,
    sendNotification: mockSendNotification
}));

function makeService(enabled = true) {
    const settings = { notificationsEnabled: enabled };
    let now = 1_000_000;
    const service = new NotificationService(settings, () => now);
    return {
        service,
        settings,
        advance: (ms: number) => {
            now += ms;
        }
    };
}

describe('NotificationService', () => {
    beforeEach(async () => {
        await setLocale('en');
        mockIsPermissionGranted.mockReset().mockResolvedValue(true);
        mockRequestPermission.mockReset().mockResolvedValue('granted');
        mockSendNotification.mockReset();
        vi.spyOn(document, 'hasFocus').mockReturnValue(false);
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('does nothing while notifications are disabled', () => {
        const { service } = makeService(false);
        service.deviceAdded('phone');

        expect(mockSendNotification).not.toHaveBeenCalled();
    });

    it('suppresses device events while the window is focused', () => {
        vi.spyOn(document, 'hasFocus').mockReturnValue(true);
        const { service } = makeService();

        service.deviceAdded('phone');
        service.deviceRemoved('phone');
        service.updateAvailable('2.1.0');

        expect(mockSendNotification).not.toHaveBeenCalled();
    });

    it('shows session errors even while the window is focused', () => {
        vi.spyOn(document, 'hasFocus').mockReturnValue(true);
        const { service } = makeService();

        service.sessionEnded('phone');

        expect(mockSendNotification).toHaveBeenCalledWith({
            title: 'Session ended',
            body: 'The session on phone ended unexpectedly.'
        });
    });

    it('deduplicates repeated events for 30 seconds', () => {
        const { service, advance } = makeService();

        service.deviceAdded('phone');
        service.deviceAdded('phone');
        expect(mockSendNotification).toHaveBeenCalledTimes(1);

        advance(30_001);
        service.deviceAdded('phone');
        expect(mockSendNotification).toHaveBeenCalledTimes(2);
    });

    it('avoids a disconnect notification right after a session ended', () => {
        const { service, advance } = makeService();

        service.sessionEnded('phone');
        service.deviceRemoved('phone');
        expect(mockSendNotification).toHaveBeenCalledTimes(1);

        advance(15_001);
        service.deviceRemoved('phone');
        expect(mockSendNotification).toHaveBeenCalledTimes(2);
    });

    it('checks and requests permission', async () => {
        const { service } = makeService();
        mockIsPermissionGranted.mockResolvedValueOnce(false);
        mockRequestPermission.mockResolvedValueOnce('denied');
        expect(await service.ensurePermission()).toBe(false);

        mockIsPermissionGranted.mockResolvedValueOnce(false);
        mockRequestPermission.mockResolvedValueOnce('granted');
        expect(await service.ensurePermission()).toBe(true);
    });

    it('never throws when the notification backend fails', () => {
        mockSendNotification.mockImplementationOnce(() => {
            throw new Error('no notification daemon');
        });
        const { service } = makeService();
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

        expect(() => service.deviceAdded('phone')).not.toThrow();
        expect(warn).toHaveBeenCalled();
    });

    it('cleans control characters and caps the text length', () => {
        expect(cleanText('  a\nb\tc  ', 20)).toBe('a b c');
        expect(cleanText('x'.repeat(200), 10)).toHaveLength(10);
        expect(cleanText('a\u0000b', 20)).toBe('a b');
    });
});
