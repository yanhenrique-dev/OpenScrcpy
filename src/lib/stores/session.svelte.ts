import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { showAlert } from '../alerts.svelte';
import { t } from '../../i18n';
import type { AudioStatus, CommandResult, ScrcpyConfig, SessionMode } from '../types';
import type { LogStore } from './logs.svelte';
import type { NotificationService } from '../notifications';
import type { SettingsStore } from './settings.svelte';
import type { DevicesStore } from './devices.svelte';

const SESSION_MODE_LABEL_KEYS: Record<SessionMode, string> = {
    mirror: 'controlPanel.screen',
    camera: 'controlPanel.camera',
    desktop: 'controlPanel.desktop',
    mic: 'controlPanel.microphone'
};

/** Running scrcpy sessions, file transfer and terminal helpers. */
export class SessionStore {
    runningDevices = $state<string[]>([]);
    isStarting = $state(false);
    detectedCameras = $state<{ id: string; name: string }[]>([]);
    /** Per-device audio health, updated by `scrcpy-audio-status` events. */
    audioStatusByDevice = $state<Record<string, AudioStatus>>({});

    private starting = false;
    private startedModeByDevice: Record<string, SessionMode> = {};

    constructor(
        private readonly settings: SettingsStore,
        private readonly logs: LogStore,
        private readonly devices: DevicesStore,
        private readonly notifications: NotificationService
    ) {}

    get sessionRunning(): boolean {
        return this.runningDevices.includes(this.devices.activeDevice);
    }

    get recordingSupported(): boolean {
        return this.settings.config.sessionMode !== 'mic';
    }

    setAudioStatus = (device: string, status: AudioStatus) => {
        if (status === 'off') {
            delete this.audioStatusByDevice[device];
            return;
        }
        this.audioStatusByDevice[device] = status;
    };

    runScrcpy = async (config: ScrcpyConfig) => {
        if (this.starting) return;
        this.starting = true;
        this.isStarting = true;
        try {
            this.logs.add(t('logs.initializingScrcpy', { device: config.device }), 'system');
            const savedPos =
                config.rememberWindowPosition !== false
                    ? this.settings.windowPositions[config.device]
                    : undefined;
            await invoke('run_scrcpy', {
                config: { ...config, windowX: savedPos?.x, windowY: savedPos?.y }
            });
        } catch (e) {
            this.logs.add(t('logs.failedToStartScrcpy', { error: String(e) }), 'err');
            this.notifications.sessionFailed(config.device);
        } finally {
            this.starting = false;
            this.isStarting = false;
        }
    };

    startSession = async () => {
        const activeDevice = this.devices.activeDevice;
        if (!activeDevice) {
            showAlert({
                title: t('alerts.noDeviceSelectedTitle'),
                message: t('alerts.noDeviceSelectedMessage'),
                kind: 'warning'
            });
            return;
        }

        const runningMode = this.runningDevices.includes(activeDevice)
            ? this.startedModeByDevice[activeDevice]
            : undefined;

        if (runningMode && runningMode !== this.settings.config.sessionMode) {
            showAlert({
                title: t('alerts.sessionReplaceTitle'),
                message: t('alerts.sessionReplaceMsg', {
                    mode: t(SESSION_MODE_LABEL_KEYS[runningMode] ?? runningMode),
                    newMode: t(
                        SESSION_MODE_LABEL_KEYS[this.settings.config.sessionMode] ??
                            this.settings.config.sessionMode
                    )
                }),
                kind: 'warning',
                actionLabel: t('alerts.sessionReplaceConfirm'),
                onAction: () => this.performStart(),
                showCancel: true
            });
            return;
        }
        await this.performStart();
    };

    private performStart = async () => {
        if (this.devices.activeDevice) {
            this.startedModeByDevice[this.devices.activeDevice] = this.settings.config.sessionMode;
        }
        await this.runScrcpy(this.settings.config);
    };

    stopSession = async () => {
        if (!this.devices.activeDevice) return;
        delete this.startedModeByDevice[this.devices.activeDevice];
        await this.stopScrcpy(this.devices.activeDevice);
    };

    stopScrcpy = async (device: string) => {
        try {
            await invoke('stop_scrcpy', { device });
        } catch (e) {
            const message = String(e);
            this.logs.add(t('logs.failedToStopScrcpy', { error: message }), 'err');
            showAlert({
                title: t('alerts.stopFailedTitle'),
                message: t('alerts.stopFailedMsg', { error: message }),
                kind: 'error'
            });
        }
    };

    recenterMirrorWindow = async (device: string) => {
        try {
            await invoke('recenter_scrcpy_window', { device });
        } catch (e) {
            console.error(e);
        }
    };

    listScrcpyOptions = async (
        device: string,
        arg: string,
        customPath?: string
    ): Promise<CommandResult> => {
        try {
            this.logs.add(t('logs.runningScrcpyArg', { arg }), 'system');
            const res = await invoke<CommandResult>('list_scrcpy_options', {
                device,
                arg,
                customPath: customPath || this.settings.config.scrcpyPath
            });
            if (res.output) {
                const lines = res.output.split('\n');
                this.logs.add(lines);
                if (arg === '--list-cameras') {
                    const cameras: { id: string; name: string }[] = [];
                    lines.forEach(line => {
                        const trimmed = line.trim();
                        const next = trimmed.match(/--camera-id=(\w+)\s*\((.*?)\)/);
                        const old = trimmed.match(/^(?:-\s*)?\[(\w+)\]\s*\((.*?)\)\s*(.*)/);
                        if (next) {
                            cameras.push({ id: next[1], name: `${next[1]}: ${next[2]}` });
                        } else if (old) {
                            const resolution = old[2];
                            const metadata = old[3].replace(/\r$/, '').trim();
                            cameras.push({
                                id: old[1],
                                name: `${old[1]}: ${metadata || 'Camera'} (${resolution})`
                            });
                        }
                    });
                    if (cameras.length > 0) this.detectedCameras = cameras;
                    else this.logs.add(t('logs.noCamerasParsed'), 'warn');
                }
            }
            return res;
        } catch (e) {
            this.logs.add(t('logs.genericError', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        }
    };

    pushFile = async (
        device: string,
        filePath: string,
        customPath?: string
    ): Promise<CommandResult> => {
        try {
            this.logs.add(t('logs.pushingFile', { device, filePath }), 'system');
            const res = await invoke<CommandResult>('push_file', {
                device,
                filePath,
                customPath: customPath || this.settings.config.scrcpyPath
            });
            this.logs.add(t('logs.adbPrefix', { message: String(res.message) }));
            return res;
        } catch (e) {
            this.logs.add(t('logs.genericError', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        }
    };

    installApk = async (
        device: string,
        filePath: string,
        customPath?: string
    ): Promise<CommandResult> => {
        try {
            this.logs.add(t('logs.installingApk', { device, filePath }), 'system');
            const res = await invoke<CommandResult>('install_apk', {
                device,
                filePath,
                customPath: customPath || this.settings.config.scrcpyPath
            });
            this.logs.add(t('logs.adbPrefix', { message: String(res.message) }));
            return res;
        } catch (e) {
            this.logs.add(t('logs.genericError', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        }
    };

    runTerminalCommand = async (command: string, customPath?: string): Promise<CommandResult> => {
        try {
            const lower = command.trim().toLowerCase();
            const prefix = lower.startsWith('scrcpy') || lower.startsWith('adb') ? '' : 'adb ';
            this.logs.add(`> ${prefix}${command}`, 'system');
            const res = await invoke<CommandResult>('run_terminal_command', {
                device: this.devices.activeDevice,
                cmd: command,
                customPath: customPath || this.settings.config.scrcpyPath
            });
            if (res.stdout) this.logs.add(res.stdout.trim().split('\n'));
            if (res.stderr) {
                this.logs.add(
                    res.stderr
                        .trim()
                        .split('\n')
                        .map(l => `[${res.binary?.toUpperCase() || 'ERR'}] ${l}`)
                );
            }
            if (!res.success && res.message) {
                this.logs.add(t('logs.commandFailed', { error: String(res.message) }), 'err');
            }
            return res;
        } catch (e) {
            this.logs.add(t('logs.commandFailed', { error: String(e) }), 'err');
            return { success: false, message: String(e) };
        }
    };

    handleFileOperation = async (path: string) => {
        if (!this.devices.activeDevice) return;
        if (path.toLowerCase().endsWith('.apk')) {
            await this.installApk(this.devices.activeDevice, path);
        } else {
            await this.pushFile(this.devices.activeDevice, path);
        }
    };

    handleFileBrowse = async () => {
        if (!this.devices.activeDevice) return;
        try {
            const selected = await open({
                multiple: true,
                filters: [
                    { name: 'All Files', extensions: ['*'] },
                    { name: 'Android App (APK)', extensions: ['apk'] }
                ]
            });
            if (selected) {
                const paths = Array.isArray(selected) ? selected : [selected];
                for (const path of paths) await this.handleFileOperation(path);
            }
        } catch (e) {
            console.error(e);
        }
    };
}
