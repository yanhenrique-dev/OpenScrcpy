import {
    isPermissionGranted,
    requestPermission,
    sendNotification
} from '@tauri-apps/plugin-notification';
import { t } from '../i18n';

const DEDUP_MS = 30_000;
const SESSION_GRACE_MS = 15_000;

interface NotificationSettings {
    notificationsEnabled: boolean;
}

interface NotifyOptions {
    key: string;
    title: string;
    body: string;
    /** Session errors are important even while the window is focused. */
    force?: boolean;
}

/** Native notifications for devices, sessions and updates. */
export class NotificationService {
    private readonly lastSent = new Map<string, number>();
    private readonly sessionEndedAt = new Map<string, number>();

    constructor(
        private readonly settings: NotificationSettings,
        private readonly now: () => number = Date.now
    ) {}

    reset() {
        this.lastSent.clear();
        this.sessionEndedAt.clear();
    }

    async ensurePermission(): Promise<boolean> {
        try {
            if (await isPermissionGranted()) return true;
            return (await requestPermission()) === 'granted';
        } catch (error) {
            console.warn('Could not request notification permission', error);
            return false;
        }
    }

    notifyEnabled() {
        this.send({
            key: 'enabled',
            title: t('notifications.enabledTitle'),
            body: t('notifications.enabledBody'),
            force: true
        });
    }

    sessionFailed(device: string) {
        this.send({
            key: `session-start:${device}`,
            title: t('notifications.sessionFailedTitle'),
            body: t('notifications.sessionFailedBody', { device }),
            force: false
        });
    }

    sessionEnded(device: string) {
        this.sessionEndedAt.set(device, this.now());
        this.send({
            key: `session-end:${device}`,
            title: t('notifications.sessionEndedTitle'),
            body: t('notifications.sessionEndedBody', { device }),
            force: true
        });
    }

    deviceAdded(device: string) {
        this.send({
            key: `device-added:${device}`,
            title: t('notifications.deviceConnectedTitle'),
            body: t('notifications.deviceConnectedBody', { device }),
            force: false
        });
    }

    deviceRemoved(device: string) {
        // Unplugging a mirrored device already produced a session-ended
        // notification; do not repeat the same event twice.
        if (this.now() - (this.sessionEndedAt.get(device) ?? 0) < SESSION_GRACE_MS) return;
        this.send({
            key: `device-removed:${device}`,
            title: t('notifications.deviceDisconnectedTitle'),
            body: t('notifications.deviceDisconnectedBody', { device }),
            force: false
        });
    }

    updateAvailable(version: string) {
        this.send({
            key: `update:${version}`,
            title: t('notifications.updateTitle'),
            body: t('notifications.updateBody', { version }),
            force: false
        });
    }

    private send(options: NotifyOptions): boolean {
        if (!this.settings.notificationsEnabled) return false;
        if (!options.force && !isWindowInBackground()) return false;

        const timestamp = this.now();
        const previous = this.lastSent.get(options.key);
        if (previous !== undefined && timestamp - previous < DEDUP_MS) return false;

        try {
            sendNotification({
                title: cleanText(options.title, 80),
                body: cleanText(options.body, 180)
            });
            this.lastSent.set(options.key, timestamp);
            return true;
        } catch (error) {
            // A missing notification daemon must never break a session or a
            // device refresh; the in-app console remains the source of truth.
            console.warn('Could not show notification', error);
            return false;
        }
    }
}

export function isWindowInBackground(): boolean {
    if (typeof document === 'undefined') return true;
    return document.visibilityState === 'hidden' || !document.hasFocus();
}

/** Native notifications render as plain text; strip control characters and
 *  keep backend/device strings from producing multi-line or huge banners. */
export function cleanText(value: string, limit: number): string {
    return value
        .replace(/[\u0000-\u001f\u007f]+/g, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .slice(0, limit);
}
