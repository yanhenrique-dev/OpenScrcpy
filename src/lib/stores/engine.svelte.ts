import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { open } from '@tauri-apps/plugin-dialog';
import { isTauri } from '../tauri';
import { showAlert } from '../alerts.svelte';
import { t } from '../../i18n';
import type { DownloadStatus, RenderDriverSupport } from '../types';
import { safeGetItem, safeSetItem } from '../storage';
import type { LogStore } from './logs.svelte';
import type { NotificationService } from '../notifications';
import type { SettingsStore } from './settings.svelte';

/** Scrcpy binary lifecycle, downloads and application updates. */
export class EngineStore {
    downloadProgress = $state(0);
    downloadError = $state<string | null>(null);
    isDownloading = $state(false);
    scrcpyStatus = $state<{ found: boolean; message: string }>({
        found: false,
        message: t('common.loading')
    });
    renderDriverSupport = $state<RenderDriverSupport>({
        hostOs: 'unknown',
        supportsRenderDriver: false,
        supportedDrivers: []
    });
    isOnboardingOpen = $state(false);
    appVersion = $state('1.0.0');
    appUpdate = $state<{ latest: string; notes: string } | null>(null);

    private downloading = false;
    private scrcpyUpdateChecked = false;

    constructor(
        private readonly settings: SettingsStore,
        private readonly logs: LogStore,
        private readonly notifications: NotificationService
    ) {}

    async loadVersion() {
        try {
            this.appVersion = await getVersion();
        } catch (e) {
            console.error('Initialization failed:', e);
        }
    }

    async loadDefaultRecordPath() {
        const path = await invoke<string>('get_videos_dir').catch(() => '');
        this.settings.defaultRecordPath = path;
        if (!this.settings.config.recordPath && path) {
            this.settings.updateConfig({ recordPath: path });
        }
    }

    downloadScrcpy = async () => {
        if (this.downloading) return;
        this.downloading = true;
        try {
            this.isDownloading = true;
            this.downloadError = null;
            this.downloadProgress = 0;
            await invoke('download_scrcpy');
        } catch (e) {
            this.isDownloading = false;
            // Surface the backend reason (for example "stop all sessions
            // first") instead of a generic "Download failed".
            const detail = typeof e === 'string' ? e.trim() : e instanceof Error ? e.message : '';
            const message = detail || t('onboarding.downloadFailed');
            this.downloadError = message;
            this.logs.add(t('logs.downloadError', { error: message }), 'err');
            if (!this.isOnboardingOpen) {
                showAlert({ title: t('alerts.downloadFailedTitle'), message, kind: 'error' });
            }
        } finally {
            this.downloading = false;
        }
    };

    checkScrcpy = async (customPath?: string): Promise<boolean> => {
        try {
            const pathToCheck =
                customPath !== undefined ? customPath : this.settings.config.scrcpyPath;
            const res = await invoke<DownloadStatus>('check_scrcpy', { customPath: pathToCheck });
            this.scrcpyStatus = res;

            if (res.found) {
                try {
                    const renderRes = await invoke<Partial<RenderDriverSupport>>(
                        'get_render_drivers',
                        { customPath: pathToCheck }
                    );
                    this.renderDriverSupport = {
                        hostOs: renderRes?.hostOs || 'unknown',
                        supportsRenderDriver: !!renderRes?.supportsRenderDriver,
                        supportedDrivers: Array.isArray(renderRes?.supportedDrivers)
                            ? renderRes.supportedDrivers
                            : []
                    };
                } catch {
                    this.renderDriverSupport = {
                        hostOs: 'unknown',
                        supportsRenderDriver: false,
                        supportedDrivers: []
                    };
                }
            } else {
                this.renderDriverSupport = {
                    hostOs: 'unknown',
                    supportsRenderDriver: false,
                    supportedDrivers: []
                };
            }

            if (!res.found && safeGetItem('scrcpy_onboarding_done') !== 'true') {
                this.isOnboardingOpen = true;
            }
            if (res.found) void this.checkScrcpyUpdate();
            return res.found;
        } catch (e) {
            this.scrcpyStatus = {
                found: false,
                message: t('logs.genericError', { error: String(e) })
            };
            return false;
        }
    };

    completeOnboarding = () => {
        safeSetItem('scrcpy_onboarding_done', 'true');
        this.isOnboardingOpen = false;
    };

    handleSetPath = async () => {
        try {
            let startPath = this.settings.config.scrcpyPath;
            if (!startPath) {
                startPath = await invoke<string>('get_scrcpy_bin_dir').catch(() => '');
            }
            const selected = await open({
                directory: true,
                multiple: false,
                defaultPath: startPath || undefined
            });
            if (selected && typeof selected === 'string') {
                this.settings.updateConfig({ scrcpyPath: selected });
                this.logs.add(t('logs.customScrcpyPathSet', { path: selected }), 'system');
                void this.checkScrcpy(selected);
            }
        } catch (e) {
            console.error(e);
        }
    };

    handleResetPath = async () => {
        this.settings.updateConfig({ scrcpyPath: undefined });
        this.logs.add(t('logs.customScrcpyPathCleared'), 'system');
        void this.checkScrcpy('');
    };

    checkForAppUpdate = async (silent: boolean) => {
        if (!isTauri()) return null;
        try {
            const { check } = await import('@tauri-apps/plugin-updater');
            const update = await check();
            const info = update ? { latest: update.version, notes: update.body ?? '' } : null;
            const previous = this.appUpdate?.latest;
            this.appUpdate = info;
            if (info && info.latest !== previous) {
                this.notifications.updateAvailable(info.latest);
            }
            if (!silent) {
                if (info) this.openAppUpdateModal(info);
                else {
                    showAlert({
                        title: t('alerts.appUpToDateTitle'),
                        message: t('alerts.appUpToDateMsg', { v: this.appVersion }),
                        kind: 'info'
                    });
                }
            }
            return info;
        } catch {
            if (!silent) {
                showAlert({
                    title: t('alerts.appCheckFailedTitle'),
                    message: t('alerts.appCheckFailedMsg'),
                    kind: 'warning'
                });
            }
            return null;
        }
    };

    openAppUpdateModal = (info: { latest: string; notes: string }) => {
        const notes = (info.notes || '')
            .replace(/^#{1,4}\s?/gm, '')
            .replace(/^>\s?/gm, '')
            .replace(/[*`]/g, '')
            .split('\n')
            .map(line => line.trim())
            .filter(line => line && !/^what'?s changed$/i.test(line) && !/^full changelog:/i.test(line))
            .join('\n')
            .trim();
        const isPlaceholder = /^see the assets below for the release\.?$/i.test(notes);
        showAlert({
            title: t('alerts.newAppVersionTitle'),
            message: notes && !isPlaceholder ? notes : t('alerts.newAppVersionMsg'),
            kind: 'info',
            eyebrow: `v${this.appVersion} → v${info.latest}`,
            actionLabel: t('header.autoUpdateBtn'),
            onAction: () => this.handleAutoUpdate(),
            showCancel: true,
            cancelLabel: t('alerts.appCloseBtn'),
            secondaryLabel: t('alerts.updateOpenBtn'),
            onSecondary: async () => {
                try {
                    await invoke('open_app_release');
                } catch {
                    window.open(
                        'https://github.com/yanhenrique-dev/OpenScrcpy/releases/latest',
                        '_blank'
                    );
                }
            }
        });
    };

    handleAutoUpdate = async () => {
        try {
            const { check } = await import('@tauri-apps/plugin-updater');
            const { relaunch } = await import('@tauri-apps/plugin-process');
            const update = await check();
            if (!update) {
                showAlert({
                    title: t('alerts.appUpToDateTitle'),
                    message: t('alerts.appUpToDateMsg', { v: this.appVersion }),
                    kind: 'info'
                });
                return;
            }
            let total = 0;
            let done = 0;
            let lastShownPct = -1;
            const fmtMb = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1);
            await update.downloadAndInstall(event => {
                if (event.event === 'Started') {
                    total = event.data.contentLength ?? 0;
                    if (total > 0) {
                        lastShownPct = 0;
                        showAlert({
                            title: t('header.autoDownloading'),
                            message: t('header.autoProgress', { p: 0 }),
                            kind: 'info',
                            progress: 0,
                            progressDetail: `0/${fmtMb(total)} MB`
                        });
                    }
                } else if (event.event === 'Progress') {
                    done += event.data.chunkLength;
                    if (total > 0) {
                        const p = Math.min(100, Math.round((done / total) * 100));
                        if (p !== lastShownPct) {
                            lastShownPct = p;
                            showAlert({
                                title: t('header.autoDownloading'),
                                message: t('header.autoProgress', { p }),
                                kind: 'info',
                                progress: p,
                                progressDetail: `${fmtMb(done)}/${fmtMb(total)} MB`
                            });
                        }
                    }
                }
            });
            showAlert({
                title: t('header.autoInstalled'),
                message: t('header.autoRestarting'),
                kind: 'success'
            });
            await new Promise(resolve => setTimeout(resolve, 1200));
            await relaunch();
        } catch (e) {
            showAlert({
                title: t('header.autoFailTitle'),
                message: `${t('header.autoFailMsg')}\n\n${t('alerts.wirelessConsoleLabel')}: ${String(e)}`,
                kind: 'warning'
            });
        }
    };

    checkScrcpyUpdate = async () => {
        if (this.scrcpyUpdateChecked || !this.scrcpyStatus.found || this.isDownloading) return;
        this.scrcpyUpdateChecked = true;
        try {
            const updateRes = await invoke<{
                update_available?: boolean;
                local_version?: string;
                latest_version?: string;
            }>('check_scrcpy_update', {
                customPath: this.settings.config.scrcpyPath
            });
            if (updateRes && updateRes.update_available) {
                showAlert({
                    title: t('alerts.updateAvailableTitle'),
                    message: t('alerts.updateAvailableMessage', {
                        local: updateRes.local_version || 'unknown',
                        latest: updateRes.latest_version || 'unknown'
                    }),
                    kind: 'info',
                    actionLabel: t('alerts.updateBtn'),
                    onAction: async () => {
                        if (this.settings.config.scrcpyPath) {
                            this.settings.updateConfig({ scrcpyPath: undefined });
                        }
                        await this.downloadScrcpy();
                    },
                    showCancel: true,
                    cancelLabel: t('alerts.cancelBtn')
                });
            }
        } catch (e) {
            console.error('Failed to check for scrcpy updates:', e);
        }
    };
}
