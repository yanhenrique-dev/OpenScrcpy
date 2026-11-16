import {
    loadStoredJSON,
    loadStoredStringArray,
    safeGetItem,
    safeSetItem,
    sanitizeWindowPositions
} from '../storage';
import type { ColorMode, ScrcpyConfig, WindowPositions } from '../types';
import { DEFAULT_CONFIG, normalizeConfig } from '../config';

/** Configuration and preferences persisted in localStorage. */
export class SettingsStore {
    config = $state<ScrcpyConfig>({ ...DEFAULT_CONFIG });
    colorMode = $state<ColorMode>('system');
    historyDevices = $state<string[]>([]);
    defaultRecordPath = $state('');
    windowPositions = $state<WindowPositions>({});

    private colorMediaQuery: MediaQueryList | null = null;
    private colorMediaHandler: ((event: MediaQueryListEvent) => void) | null = null;

    load() {
        const savedConfig = loadStoredJSON<Partial<ScrcpyConfig> | null>('scrcpy_config', null);
        this.config = normalizeConfig(savedConfig);
        this.colorMode = (safeGetItem('scrcpy_color_mode') as ColorMode) || 'system';
        this.historyDevices = loadStoredStringArray('scrcpy_history', []).slice(0, 10);
        this.windowPositions = sanitizeWindowPositions(
            loadStoredJSON<unknown>('scrcpy_window_positions', {})
        );
    }

    updateConfig(patch: Partial<ScrcpyConfig>) {
        this.config = { ...this.config, ...patch };
    }

    setConfig(next: ScrcpyConfig) {
        this.config = next;
    }

    setDevice(device: string) {
        if (device) this.updateConfig({ device });
    }

    persistConfig() {
        safeSetItem('scrcpy_config', JSON.stringify(this.config));
    }

    persistWindowPositions() {
        safeSetItem('scrcpy_window_positions', JSON.stringify(this.windowPositions));
    }

    persistHistory() {
        safeSetItem('scrcpy_history', JSON.stringify(this.historyDevices));
    }

    addToHistory(ip: string) {
        if (!ip.includes(':')) return;
        this.historyDevices = [ip, ...this.historyDevices.filter(d => d !== ip)].slice(0, 10);
        this.persistHistory();
    }

    clearHistory() {
        this.historyDevices = [];
        this.persistHistory();
    }

    setColorMode(mode: ColorMode) {
        this.colorMode = mode;
        this.applyColorMode(mode);
        safeSetItem('scrcpy_color_mode', mode);
    }

    applyColorMode(mode: ColorMode = this.colorMode) {
        const apply = (dark: boolean) => {
            document.documentElement.setAttribute('data-mode', dark ? 'dark' : 'light');
        };

        if (this.colorMediaQuery && this.colorMediaHandler) {
            this.colorMediaQuery.removeEventListener('change', this.colorMediaHandler);
            this.colorMediaQuery = null;
            this.colorMediaHandler = null;
        }

        if (mode === 'system') {
            if (typeof window.matchMedia !== 'function') {
                apply(false);
                return;
            }
            const mq = window.matchMedia('(prefers-color-scheme: dark)');
            const handler = (event: MediaQueryListEvent) => apply(event.matches);
            apply(mq.matches);
            mq.addEventListener('change', handler);
            this.colorMediaQuery = mq;
            this.colorMediaHandler = handler;
        } else {
            apply(mode === 'dark');
        }
    }

    destroy() {
        if (this.colorMediaQuery && this.colorMediaHandler) {
            this.colorMediaQuery.removeEventListener('change', this.colorMediaHandler);
            this.colorMediaQuery = null;
            this.colorMediaHandler = null;
        }
    }
}
