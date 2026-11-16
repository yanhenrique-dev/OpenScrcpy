import { invoke } from '@tauri-apps/api/core';
import { t } from '../../i18n';
import type {
    CommandResult,
    DeviceListResult,
    MdnsDevice,
    MdnsDevicesResult,
    UnavailableDevice
} from '../types';
import type { LogStore } from './logs.svelte';
import type { SettingsStore } from './settings.svelte';

type ReconnectBackoff = Record<string, { attempts: number; nextAt: number }>;

/** Device discovery, mDNS cache and reconnection bookkeeping. */
export class DevicesStore {
    devices = $state<string[]>([]);
    deviceModels = $state<Record<string, string>>({});
    deviceFriendlyNames = $state<Record<string, string>>({});
    unavailableDevices = $state<UnavailableDevice[]>([]);
    mdnsDevices = $state<MdnsDevice[]>([]);
    activeDevice = $state('');
    isRefreshing = $state(false);
    wirelessSwitching = $state<string | null>(null);

    private active = true;
    private prevDevices: string[] = [];
    private mdnsCache: MdnsDevice[] = [];
    private fetchedFriendlyNames = new Set<string>();
    private reconnectInFlight = new Set<string>();
    private reconnectBackoff: ReconnectBackoff = {};
    private fetchInFlight: Promise<void> | null = null;

    constructor(
        private readonly settings: SettingsStore,
        private readonly logs: LogStore
    ) {}

    activate() {
        this.active = true;
    }

    dispose() {
        this.active = false;
    }

    setActiveDevice(device: string) {
        this.activeDevice = device;
        this.settings.setDevice(device);
    }

    private fetchDevicesOnceUnlocked = async (customPath?: string, silent = false) => {
        try {
            const res = await invoke<DeviceListResult>('get_devices', {
                customPath: customPath || this.settings.config.scrcpyPath
            });

            if (!res.error) {
                const newDevices = res.devices ?? [];
                if (res.deviceModels) this.deviceModels = res.deviceModels;

                const added = newDevices.filter(d => !this.prevDevices.includes(d));
                const removed = this.prevDevices.filter(d => !newDevices.includes(d));

                added.forEach(device =>
                    this.logs.add(t('logs.newDeviceDiscovered', { device }), 'ok')
                );
                removed.forEach(device =>
                    this.logs.add(t('logs.deviceDisconnected', { device }), 'warn')
                );

                this.devices = newDevices;
                this.unavailableDevices = res.unavailable ?? [];
                this.prevDevices = newDevices;
                newDevices.forEach(device => delete this.reconnectBackoff[device]);
                void this.ensureFriendlyNames(newDevices);

                if (!silent && added.length === 0 && removed.length === 0) {
                    this.logs.add(
                        t('logs.discoveryActive', { count: newDevices.length }),
                        'system'
                    );
                }

                if (!newDevices.includes(this.activeDevice)) {
                    this.setActiveDevice(newDevices[0] ?? '');
                }
            } else {
                this.unavailableDevices = [];
                if (!silent) {
                    this.logs.add(
                        t('logs.discoveryError', { error: res.message ?? 'unknown error' }),
                        'err'
                    );
                }
            }

            try {
                const mdnsRes = await invoke<MdnsDevicesResult>('get_mdns_devices', {
                    customPath: customPath || this.settings.config.scrcpyPath
                });
                if (mdnsRes && !mdnsRes.error && mdnsRes.services) {
                    const parsed = mdnsRes.services.filter(
                        s =>
                            s.service &&
                            (s.service.includes('_adb-tls-connect') ||
                                s.service.includes('_adb-tls-pairing'))
                    );
                    this.mdnsDevices = parsed;
                    this.mdnsCache = parsed;
                } else if (mdnsRes && mdnsRes.error) {
                    console.warn('[ADB MDNS] Failed to get mdns devices:', mdnsRes.message);
                    this.mdnsDevices = [];
                    this.mdnsCache = [];
                }
            } catch (e) {
                console.error('Failed to query mDNS devices:', e);
            }
        } catch (e) {
            console.error(e);
            this.logs.add(t('logs.errorRefreshingDevices', { error: String(e) }), 'err');
        }
    };

    private ensureFriendlyNames = async (serials: string[]) => {
        for (const serial of serials) {
            if (this.fetchedFriendlyNames.has(serial)) continue;
            this.fetchedFriendlyNames.add(serial);
            const isEmulator = /^emulator-\d+$/.test(serial);
            try {
                const command = isEmulator
                    ? 'getprop ro.boot.qemu.avd_name; getprop ro.kernel.qemu.avd_name'
                    : 'getprop ro.product.marketname; getprop ro.product.manufacturer; getprop ro.product.model';
                const res = await invoke<CommandResult>('adb_shell', {
                    device: serial,
                    command,
                    customPath: this.settings.config.scrcpyPath
                });

                if (isEmulator) {
                    const avdName = res?.success
                        ? (res.output ?? '')
                              .split('\n')
                              .map(line => line.trim())
                              .find(line => line.length > 0)
                        : undefined;
                    this.deviceFriendlyNames = {
                        ...this.deviceFriendlyNames,
                        [serial]: avdName ? `${avdName.replace(/_/g, ' ')} (${serial})` : serial
                    };
                    continue;
                }

                if (res?.success) {
                    const [marketname, manufacturer, model] = (res.output ?? '')
                        .split('\n')
                        .map(line => line.trim());
                    const name =
                        marketname ||
                        (manufacturer &&
                        model &&
                        !model.toLowerCase().startsWith(manufacturer.toLowerCase())
                            ? `${manufacturer} ${model}`
                            : model);
                    if (name) {
                        this.deviceFriendlyNames = { ...this.deviceFriendlyNames, [serial]: name };
                    }
                }
            } catch (e) {
                console.error('Failed to fetch friendly name for', serial, e);
            }
        }
    };

    fetchDevicesOnce = (customPath?: string, silent = false): Promise<void> => {
        if (this.fetchInFlight) return this.fetchInFlight;
        const run = this.fetchDevicesOnceUnlocked(customPath, silent);
        let tracked: Promise<void>;
        tracked = run.finally(() => {
            if (this.fetchInFlight === tracked) this.fetchInFlight = null;
        });
        this.fetchInFlight = tracked;
        return tracked;
    };

    /** Silent periodic refresh used by the facade's polling timer. */
    poll = (): Promise<void> => this.fetchDevicesOnce(undefined, true);

    refreshDevices = async (customPath?: string, silent = false) => {
        if (this.isRefreshing) return;
        this.isRefreshing = true;
        try {
            await this.fetchDevicesOnce(customPath, silent);
        } finally {
            this.isRefreshing = false;
        }
    };

    refreshDevicesUntilSettled = async (customPath?: string, attempts = 8, delayMs = 2000) => {
        if (this.isRefreshing) return;
        this.isRefreshing = true;
        try {
            let prevSnapshot = '';
            for (let i = 0; i <= attempts; i++) {
                if (!this.active) return;
                await this.fetchDevicesOnce(customPath, true);
                if (!this.active) return;
                const snapshot = JSON.stringify([this.prevDevices, this.mdnsCache]);
                if (snapshot === prevSnapshot) return;
                prevSnapshot = snapshot;
                if (i < attempts) await new Promise(r => setTimeout(r, delayMs));
            }
        } finally {
            if (this.active) this.isRefreshing = false;
        }
    };

    tryReconnectHistory = () => {
        if (document.visibilityState === 'hidden') return;
        const now = Date.now();
        this.settings.historyDevices.forEach(ip => {
            if (this.prevDevices.includes(ip)) return;
            if (this.reconnectInFlight.has(ip)) return;
            const backoff = this.reconnectBackoff[ip];
            if (backoff && now < backoff.nextAt) return;

            this.reconnectInFlight.add(ip);
            invoke<CommandResult>('adb_connect', {
                ip,
                customPath: this.settings.config.scrcpyPath,
                silent: true
            })
                .then(res => {
                    if (res?.success) delete this.reconnectBackoff[ip];
                    else this.scheduleReconnect(ip);
                })
                .catch(() => this.scheduleReconnect(ip))
                .finally(() => this.reconnectInFlight.delete(ip));
        });
    };

    private scheduleReconnect = (ip: string) => {
        const attempts = (this.reconnectBackoff[ip]?.attempts ?? 0) + 1;
        const delay = Math.min(25000 * 2 ** (attempts - 1), 5 * 60_000);
        this.reconnectBackoff[ip] = { attempts, nextAt: Date.now() + delay };
    };
}
