import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isTauri } from './tauri';
import { showAlert } from './alerts.svelte';
import { t } from '../i18n';
import type { AudioStatus, CommandResult, ColorMode, ScrcpyConfig, WindowPositions } from './types';
import { LogStore } from './stores/logs.svelte';
import { SettingsStore } from './stores/settings.svelte';
import { DevicesStore } from './stores/devices.svelte';
import { SessionStore } from './stores/session.svelte';
import { EngineStore } from './stores/engine.svelte';

export { DEFAULT_CONFIG, normalizeConfig } from './config';

/**
 * Public facade for the renderer. The actual work is split by domain:
 * settings (preferences), devices (discovery), sessions (scrcpy processes),
 * engine (binary/downloads/updates) and logs. Components keep using the
 * familiar `scrcpy.*` API, so this split is internal.
 */
class ScrcpyStore {
    readonly logStore = new LogStore();
    readonly settings = new SettingsStore();
    readonly deviceStore = new DevicesStore(this.settings, this.logStore);
    readonly sessionStore = new SessionStore(this.settings, this.logStore, this.deviceStore);
    readonly engineStore = new EngineStore(this.settings, this.logStore);

    ready = $state(false);

    private initialized = false;
    private mounted = true;
    private unlisteners: UnlistenFn[] = [];
    private intervals: ReturnType<typeof setInterval>[] = [];

    // ── State facade ───────────────────────────────────────────────────────
    get config(): ScrcpyConfig {
        return this.settings.config;
    }
    set config(value: ScrcpyConfig) {
        this.settings.config = value;
    }

    get colorMode(): ColorMode {
        return this.settings.colorMode;
    }
    set colorMode(value: ColorMode) {
        this.settings.colorMode = value;
    }

    get historyDevices(): string[] {
        return this.settings.historyDevices;
    }
    set historyDevices(value: string[]) {
        this.settings.historyDevices = value;
    }

    get defaultRecordPath(): string {
        return this.settings.defaultRecordPath;
    }

    get windowPositions(): WindowPositions {
        return this.settings.windowPositions;
    }
    set windowPositions(value: WindowPositions) {
        this.settings.windowPositions = value;
    }

    get logs() {
        return this.logStore.logs;
    }
    set logs(value) {
        this.logStore.logs = value;
    }

    get devices(): string[] {
        return this.deviceStore.devices;
    }
    set devices(value: string[]) {
        this.deviceStore.devices = value;
    }

    get deviceModels(): Record<string, string> {
        return this.deviceStore.deviceModels;
    }
    set deviceModels(value: Record<string, string>) {
        this.deviceStore.deviceModels = value;
    }

    get deviceFriendlyNames(): Record<string, string> {
        return this.deviceStore.deviceFriendlyNames;
    }
    set deviceFriendlyNames(value: Record<string, string>) {
        this.deviceStore.deviceFriendlyNames = value;
    }

    get unavailableDevices() {
        return this.deviceStore.unavailableDevices;
    }
    set unavailableDevices(value) {
        this.deviceStore.unavailableDevices = value;
    }

    get mdnsDevices() {
        return this.deviceStore.mdnsDevices;
    }
    set mdnsDevices(value) {
        this.deviceStore.mdnsDevices = value;
    }

    get activeDevice(): string {
        return this.deviceStore.activeDevice;
    }
    set activeDevice(value: string) {
        this.deviceStore.activeDevice = value;
    }

    get isRefreshing(): boolean {
        return this.deviceStore.isRefreshing;
    }
    set isRefreshing(value: boolean) {
        this.deviceStore.isRefreshing = value;
    }

    get wirelessSwitching(): string | null {
        return this.deviceStore.wirelessSwitching;
    }
    set wirelessSwitching(value: string | null) {
        this.deviceStore.wirelessSwitching = value;
    }

    get runningDevices(): string[] {
        return this.sessionStore.runningDevices;
    }
    set runningDevices(value: string[]) {
        this.sessionStore.runningDevices = value;
    }

    get isStarting(): boolean {
        return this.sessionStore.isStarting;
    }
    set isStarting(value: boolean) {
        this.sessionStore.isStarting = value;
    }

    get detectedCameras() {
        return this.sessionStore.detectedCameras;
    }
    set detectedCameras(value) {
        this.sessionStore.detectedCameras = value;
    }

    get downloadProgress(): number {
        return this.engineStore.downloadProgress;
    }
    set downloadProgress(value: number) {
        this.engineStore.downloadProgress = value;
    }

    get downloadError(): string | null {
        return this.engineStore.downloadError;
    }
    set downloadError(value: string | null) {
        this.engineStore.downloadError = value;
    }

    get isDownloading(): boolean {
        return this.engineStore.isDownloading;
    }
    set isDownloading(value: boolean) {
        this.engineStore.isDownloading = value;
    }

    get scrcpyStatus() {
        return this.engineStore.scrcpyStatus;
    }
    set scrcpyStatus(value) {
        this.engineStore.scrcpyStatus = value;
    }

    get renderDriverSupport() {
        return this.engineStore.renderDriverSupport;
    }
    set renderDriverSupport(value) {
        this.engineStore.renderDriverSupport = value;
    }

    get isOnboardingOpen(): boolean {
        return this.engineStore.isOnboardingOpen;
    }
    set isOnboardingOpen(value: boolean) {
        this.engineStore.isOnboardingOpen = value;
    }

    get appVersion(): string {
        return this.engineStore.appVersion;
    }

    get appUpdate() {
        return this.engineStore.appUpdate;
    }

    get sessionRunning(): boolean {
        return this.sessionStore.sessionRunning;
    }

    get recordingSupported(): boolean {
        return this.sessionStore.recordingSupported;
    }

    get audioStatus(): Record<string, AudioStatus> {
        return this.sessionStore.audioStatusByDevice;
    }

    // ── Lifecycle ──────────────────────────────────────────────────────────
    init = async () => {
        if (this.initialized) return;
        this.initialized = true;
        this.mounted = true;
        this.deviceStore.activate();

        this.settings.load();
        this.engineStore.scrcpyStatus = { found: false, message: t('common.loading') };

        await this.engineStore.loadVersion();
        if (!this.mounted) return;

        await this.engineStore.loadDefaultRecordPath();
        if (!this.mounted) return;

        this.settings.applyColorMode();
        await this.engineStore.checkScrcpy(this.settings.config.scrcpyPath);
        // The component may have unmounted while init was awaiting. Starting
        // polling or listeners after destroy() would leak them.
        if (!this.mounted) return;
        this.ready = true;
        void this.deviceStore.refreshDevicesUntilSettled(this.settings.config.scrcpyPath);
        void this.engineStore.checkForAppUpdate(true);

        if (!isTauri()) return;

        this.setupEventListeners();
        this.startPolling();
        void this.engineStore.checkScrcpyUpdate();
    };

    destroy = () => {
        this.mounted = false;
        // Allow a later mount (hot reload/tests) to initialize again.
        this.initialized = false;
        this.ready = false;
        for (const unlisten of this.unlisteners) unlisten();
        this.unlisteners = [];
        for (const interval of this.intervals) clearInterval(interval);
        this.intervals = [];
        this.settings.destroy();
        this.deviceStore.dispose();
    };

    private setupEventListeners = () => {
        const status = listen<any>('scrcpy-status', event => {
            const data = event.payload;
            if (data.type === 'downloading') {
                this.engineStore.isDownloading = true;
                this.engineStore.downloadError = null;
                this.engineStore.downloadProgress = 0;
            } else if (data.type === 'download-progress') {
                this.engineStore.downloadProgress = data.percent;
            } else if (data.type === 'download-complete') {
                this.engineStore.isDownloading = false;
                this.engineStore.downloadError = null;
                this.engineStore.downloadProgress = 100;
                void this.deviceStore.refreshDevicesUntilSettled();
                void this.engineStore.checkScrcpy();
            }
            if (data.device && typeof data.running === 'boolean') {
                this.sessionStore.runningDevices = data.running
                    ? [...new Set([...this.sessionStore.runningDevices, data.device])]
                    : this.sessionStore.runningDevices.filter(d => d !== data.device);
                if (!data.running) this.sessionStore.setAudioStatus(data.device, 'off');
            }
        });

        const windowPos = listen<{ device: string; x: number; y: number }>(
            'scrcpy-window-pos',
            event => {
                if (this.settings.config.rememberWindowPosition === false) return;
                const { device, x, y } = event.payload;
                // The App-level effect persists this; writing here too caused
                // two localStorage writes per window-position event.
                this.settings.windowPositions = {
                    ...this.settings.windowPositions,
                    [device]: { x, y }
                };
            }
        );

        const audioStatus = listen<{ device: string; status: AudioStatus }>(
            'scrcpy-audio-status',
            event => {
                const { device, status } = event.payload;
                if (!device || !['ok', 'warning', 'error', 'off'].includes(status)) return;
                this.sessionStore.setAudioStatus(device, status);
            }
        );

        void Promise.all([status, windowPos, audioStatus])
            .then(fns => {
                if (this.mounted) this.unlisteners.push(...fns);
                else fns.forEach(fn => fn());
            })
            .catch(error => {
                console.error('Failed to subscribe to backend events', error);
            });
    };

    private startPolling = () => {
        this.intervals.push(
            setInterval(() => {
                if (document.visibilityState === 'hidden' || this.deviceStore.isRefreshing) return;
                void this.deviceStore.poll();
            }, 5000)
        );

        this.intervals.push(setInterval(() => this.deviceStore.tryReconnectHistory(), 25000));
        this.deviceStore.tryReconnectHistory();
    };

    // ── Logs ───────────────────────────────────────────────────────────────
    addLogs = (lines: string | string[], level?: Parameters<LogStore['add']>[1]) => {
        this.logStore.add(lines, level);
    };

    clearLogs = () => {
        this.logStore.clear();
    };

    setLogs = (next: Parameters<LogStore['set']>[0]) => {
        this.logStore.set(next);
    };

    // ── Settings ───────────────────────────────────────────────────────────
    updateConfig = (patch: Partial<ScrcpyConfig>) => {
        this.settings.updateConfig(patch);
    };

    setConfig = (next: ScrcpyConfig) => {
        this.settings.setConfig(next);
    };

    setActiveDevice = (device: string) => {
        this.deviceStore.setActiveDevice(device);
    };

    setColorMode = (mode: ColorMode) => {
        this.settings.setColorMode(mode);
    };

    persistConfig = () => {
        this.settings.persistConfig();
    };

    persistWindowPositions = () => {
        this.settings.persistWindowPositions();
    };

    persistHistory = () => {
        this.settings.persistHistory();
    };

    addToHistory = (ip: string) => {
        this.settings.addToHistory(ip);
    };

    clearHistory = () => {
        this.settings.clearHistory();
    };

    // ── Devices ────────────────────────────────────────────────────────────
    refreshDevices = (customPath?: string, silent = false) =>
        this.deviceStore.refreshDevices(customPath, silent);

    refreshDevicesUntilSettled = (customPath?: string, attempts?: number, delayMs?: number) =>
        this.deviceStore.refreshDevicesUntilSettled(customPath, attempts, delayMs);

    pairDevice = async (ip: string, code: string, customPath?: string): Promise<CommandResult> => {
        try {
            const res = await invoke<CommandResult>('adb_pair', {
                ip,
                code,
                customPath: customPath || this.settings.config.scrcpyPath
            });
            if (res.success) {
                this.addLogs(t('logs.successfullyPaired', { ip }), 'ok');
                await this.refreshDevicesUntilSettled(customPath);
            } else {
                const msgs = [t('logs.pairingFailed', { message: String(res.message) })];
                if (typeof res.message === 'string' && res.message.includes('protocol fault')) {
                    msgs.push(t('logs.pairingProtocolFault'));
                }
                this.addLogs(msgs);
            }
            return res;
        } catch (e) {
            this.addLogs(t('logs.pairingError', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        }
    };

    connectDevice = async (ip: string, customPath?: string): Promise<CommandResult> => {
        this.deviceStore.isRefreshing = true;
        try {
            const target = ip.includes(':') ? ip : `${ip}:5555`;
            let res = await invoke<CommandResult>('adb_connect', {
                ip: target,
                customPath: customPath || this.settings.config.scrcpyPath
            });

            if (
                !res.success &&
                typeof res.message === 'string' &&
                (res.message.includes('failed to connect') || res.message.includes('cannot connect'))
            ) {
                this.addLogs(t('logs.connectionFailedRetrying'), 'warn');
                await invoke<CommandResult>('run_terminal_command', {
                    cmd: `adb disconnect ${target}`,
                    customPath: customPath || this.settings.config.scrcpyPath
                });
                await new Promise(r => setTimeout(r, 500));
                res = await invoke<CommandResult>('adb_connect', {
                    ip: target,
                    customPath: customPath || this.settings.config.scrcpyPath
                });
            }

            if (res.success) {
                this.addLogs(t('logs.connectedSuccessfully', { ip: target }), 'ok');
                this.addToHistory(target);
                this.deviceStore.isRefreshing = false;
                await new Promise(r => setTimeout(r, 1000));
                this.deviceStore.isRefreshing = false;
                await this.refreshDevices(customPath || this.settings.config.scrcpyPath, true);
            } else {
                const msgs = [t('logs.connectionFailed', { message: String(res.message) })];
                if (
                    typeof res.message === 'string' &&
                    (res.message.includes('failed to connect') || res.message.includes('cannot connect'))
                ) {
                    msgs.push(t('logs.connectionStaleTip'));
                }
                this.addLogs(msgs);
            }
            return res;
        } catch (e) {
            this.addLogs(t('logs.connectionError', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        } finally {
            this.deviceStore.isRefreshing = false;
        }
    };

    handleKillAdb = async () => {
        try {
            await invoke('kill_adb', { customPath: this.settings.config.scrcpyPath });
            await this.refreshDevices(this.settings.config.scrcpyPath);
        } catch (e) {
            console.error(e);
        }
    };

    handleGotoWireless = async (device: string) => {
        if (this.deviceStore.wirelessSwitching) return;
        this.deviceStore.wirelessSwitching = device;
        try {
            const res = await invoke<{ success: boolean; message: string; ip?: string }>(
                'goto_wireless',
                { device }
            );
            if (res.success) {
                if (res.ip) this.addToHistory(`${res.ip}:5555`);
                showAlert({
                    title: t('alerts.wirelessOkTitle'),
                    message: t('alerts.wirelessOkMsg', { ip: res.ip ?? '' }),
                    kind: 'success'
                });
            } else {
                const detail = (res.message || '').trim();
                showAlert({
                    title: t('alerts.wirelessFailTitle'),
                    message: detail
                        ? `${this.diagnoseWirelessFailure(detail)}\n\n${t('alerts.wirelessConsoleLabel')}: ${detail}`
                        : this.diagnoseWirelessFailure(detail),
                    kind: 'warning'
                });
            }
        } catch (e) {
            const detail = String(e).trim();
            showAlert({
                title: t('alerts.wirelessFailTitle'),
                message: detail
                    ? `${this.diagnoseWirelessFailure(detail)}\n\n${t('alerts.wirelessConsoleLabel')}: ${detail}`
                    : t('alerts.wirelessHintGeneric'),
                kind: 'warning'
            });
        } finally {
            this.deviceStore.wirelessSwitching = null;
            void this.refreshDevices();
        }
    };

    private diagnoseWirelessFailure = (message: string): string => {
        if (/authenticate|unauthorized/i.test(message)) return t('alerts.wirelessHintAuth');
        if (/timed ?out|cannot connect|not connected|failed to connect/i.test(message)) {
            return t('alerts.wirelessHintConn');
        }
        return t('alerts.wirelessHintGeneric');
    };

    // ── Sessions ───────────────────────────────────────────────────────────
    runScrcpy = (config: ScrcpyConfig) => this.sessionStore.runScrcpy(config);

    startSession = () => this.sessionStore.startSession();

    stopSession = () => this.sessionStore.stopSession();

    stopScrcpy = (device: string) => this.sessionStore.stopScrcpy(device);

    recenterMirrorWindow = (device: string) => this.sessionStore.recenterMirrorWindow(device);

    listScrcpyOptions = (device: string, arg: string, customPath?: string) =>
        this.sessionStore.listScrcpyOptions(device, arg, customPath);

    pushFile = (device: string, filePath: string, customPath?: string) =>
        this.sessionStore.pushFile(device, filePath, customPath);

    installApk = (device: string, filePath: string, customPath?: string) =>
        this.sessionStore.installApk(device, filePath, customPath);

    runTerminalCommand = (command: string, customPath?: string) =>
        this.sessionStore.runTerminalCommand(command, customPath);

    handleFileOperation = (path: string) => this.sessionStore.handleFileOperation(path);

    handleFileBrowse = () => this.sessionStore.handleFileBrowse();

    // ── Engine / downloads / updates ───────────────────────────────────────
    downloadScrcpy = () => this.engineStore.downloadScrcpy();

    checkScrcpy = (customPath?: string) => this.engineStore.checkScrcpy(customPath);

    completeOnboarding = () => this.engineStore.completeOnboarding();

    handleSetPath = () => this.engineStore.handleSetPath();

    handleResetPath = () => this.engineStore.handleResetPath();

    checkForAppUpdate = (silent: boolean) => this.engineStore.checkForAppUpdate(silent);

    openAppUpdateModal = (info: { latest: string; notes: string }) =>
        this.engineStore.openAppUpdateModal(info);

    handleAutoUpdate = () => this.engineStore.handleAutoUpdate();

    checkScrcpyUpdate = () => this.engineStore.checkScrcpyUpdate();
}

export const scrcpy = new ScrcpyStore();
