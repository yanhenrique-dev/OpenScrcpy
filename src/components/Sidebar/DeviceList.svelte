<script lang="ts">
    import { AlertTriangle, Smartphone, Usb, Wifi } from '@lucide/svelte';
    import { t } from '../../i18n';
    import type { DeviceGroup, UnavailableDevice } from '../../lib/types';

    interface Props {
        devices: string[];
        deviceFriendlyNames: Record<string, string>;
        deviceModels: Record<string, string>;
        unavailable: UnavailableDevice[];
        runningDevices: string[];
        selectedDevice: string;
        wirelessSwitching: string | null;
        onSelect: (device: string) => void;
        onGotoWireless: (device: string) => void;
    }

    let {
        devices,
        deviceFriendlyNames,
        deviceModels,
        unavailable,
        runningDevices,
        selectedDevice,
        wirelessSwitching,
        onSelect,
        onGotoWireless
    }: Props = $props();

    function formatDeviceLabel(serial: string): string {
        const match = serial.match(/^adb-(.+?)( \(\d+\))?\._adb-tls-connect\._(tcp|udp)$/);
        return match ? `${match[1]}${match[2] ?? ''}` : serial;
    }

    const deviceGroups = $derived.by(() => {
        const byName = new Map<string, string[]>();
        for (const device of devices) {
            const name =
                deviceFriendlyNames[device] || deviceModels[device] || formatDeviceLabel(device);
            const ids = byName.get(name);
            if (ids) ids.push(device);
            else byName.set(name, [device]);
        }

        const groups: DeviceGroup[] = [];
        for (const [name, ids] of byName) {
            const usb = ids.filter(id => !id.includes('.'));
            const wireless = ids.filter(id => id.includes('.'));
            if (usb.length === 1 && wireless.length === 1) {
                groups.push({ name, ids: [usb[0], wireless[0]] });
            } else {
                for (const id of ids) groups.push({ name, ids: [id] });
            }
        }
        return groups;
    });
</script>

<div class="space-y-2">
    <div class="flex flex-col gap-1.5 max-h-[220px] overflow-y-auto pe-1">
        {#if devices.length === 0}
            <div class="ds-panel p-4 text-center" style="border-style: dashed">
                <span class="ds-caption" style="color: var(--text-muted)">{t('sidebar.noDevicesDetected')}</span>
            </div>
        {:else}
            {#each deviceGroups as group (group.ids[0])}
                {@const ids = group.ids}
                {@const primary = ids[0]}
                {@const isRunning = runningDevices.some(r => ids.includes(r))}
                {@const isSelected = !!selectedDevice && ids.includes(selectedDevice)}
                {@const usbId = ids.find(id => !id.includes('.'))}
                {@const hasWireless = ids.some(id => id.includes('.'))}
                {@const switching = wirelessSwitching != null && ids.includes(wirelessSwitching)}
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <div
                    role="group"
                    tabindex="0"
                    aria-label={group.name}
                    onclick={() => onSelect(primary)}
                    onkeydown={(e) => {
                        if (e.target !== e.currentTarget) return;
                        if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault();
                            onSelect(primary);
                        }
                    }}
                    class="ds-panel flex items-center gap-2 p-2 text-start w-full cursor-pointer"
                    style={isSelected ? 'border-color: var(--primary)' : undefined}
                >
                    <div class="ds-inset p-1.5 flex items-center justify-center shrink-0">
                        <Smartphone size={14} style="color: {isSelected ? 'var(--primary)' : 'var(--text-muted)'}" />
                    </div>
                    <div class="flex-1 min-w-0">
                        <p class="ds-label truncate">{group.name}</p>
                        <div class="flex items-center gap-2 mt-0.5 flex-wrap">
                            <span class="flex items-center gap-1">
                                <span class="ds-dot" data-live={isRunning}></span>
                                <span class="ds-badge" data-live={isRunning}>
                                    {isRunning ? t('sidebar.live') : t('sidebar.ready')}
                                </span>
                            </span>
                            {#if ids.length > 1}
                                <div class="tpx-group" role="group" aria-label={t('sidebar.transport')}>
                                    {#each ids as id (id)}
                                        <button
                                            type="button"
                                            aria-pressed={selectedDevice === id}
                                            aria-label="{id.includes('.') ? t('sidebar.wifi') : t('sidebar.usb')}: {id}"
                                            class="tpx-seg"
                                            data-active={selectedDevice === id ? 'true' : 'false'}
                                            onclick={(e) => {
                                                e.stopPropagation();
                                                onSelect(id);
                                            }}
                                        >
                                            {#if id.includes('.')}
                                                <Wifi size={9.5} />
                                                {t('sidebar.wifi')}
                                            {:else}
                                                <Usb size={9.5} />
                                                {t('sidebar.usb')}
                                            {/if}
                                        </button>
                                    {/each}
                                </div>
                            {:else}
                                {#each ids as id (id)}
                                    <button
                                        type="button"
                                        aria-pressed={selectedDevice === id}
                                        aria-label="{id.includes('.') ? t('sidebar.wifi') : t('sidebar.usb')}: {id}"
                                        class="ds-badge cursor-pointer"
                                        style={selectedDevice === id
                                            ? 'border-color: var(--primary); color: var(--primary)'
                                            : undefined}
                                        onclick={(e) => {
                                            e.stopPropagation();
                                            onSelect(id);
                                        }}
                                    >
                                        {#if id.includes('.')}
                                            <Wifi size={9.5} />
                                            {t('sidebar.wifi')}
                                        {:else}
                                            <Usb size={9.5} />
                                            {t('sidebar.usb')}
                                        {/if}
                                    </button>
                                {/each}
                            {/if}
                            {#if usbId && !hasWireless}
                                <button
                                    type="button"
                                    disabled={wirelessSwitching != null}
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        onGotoWireless(usbId);
                                    }}
                                    class="ds-btn ds-btn-tertiary w-full mt-1"
                                    style="padding: 5px 8px; font-size: 11.5px; line-height: 14px; gap: 6px"
                                >
                                    <Wifi size={11} />
                                    {switching ? t('sidebar.switching') : t('sidebar.gotoWireless')}
                                </button>
                            {/if}
                        </div>
                    </div>
                </div>
            {/each}
        {/if}
    </div>

    {#if unavailable.length > 0}
        <div class="ds-panel p-2 space-y-1.5" role="status">
            {#each unavailable as device (device.serial)}
                <div class="flex items-center gap-2 min-w-0">
                    <AlertTriangle size={12} class="shrink-0" />
                    <span class="ds-caption truncate" title={device.serial}>
                        {device.model || formatDeviceLabel(device.serial)}
                    </span>
                    <span class="ds-badge ms-auto shrink-0">
                        {device.state === 'unauthorized'
                            ? t('sidebar.deviceUnauthorized')
                            : device.state === 'offline'
                              ? t('sidebar.deviceOffline')
                              : t('sidebar.deviceUnavailable')}
                    </span>
                </div>
            {/each}
        </div>
    {/if}
</div>
