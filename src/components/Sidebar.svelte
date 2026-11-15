<script lang="ts">
    import { Smartphone, RefreshCw, Usb, Wifi, Zap } from '@lucide/svelte';
    import { t } from '../i18n';
    import { scrcpy } from '../lib/scrcpy.svelte';
    import DeviceList from './Sidebar/DeviceList.svelte';
    import WirelessPanel from './Sidebar/WirelessPanel.svelte';
    import FileDropCard from './Sidebar/FileDropCard.svelte';

    let activeTab = $state<'usb' | 'wireless'>('usb');
    const isRefreshing = $derived(scrcpy.isRefreshing);
</script>

<aside class="lg:col-span-3 space-y-4">
    <div class="ds-card p-4 space-y-4">
        <div class="flex flex-wrap items-center gap-2 pb-2 mb-1" style="border-bottom: 1px solid var(--border-base)">
            <h2 class="ds-label flex items-center gap-2" style="color: var(--text-muted)">
                <Smartphone size={14} />
                {t('sidebar.deviceHub')}
            </h2>
            <div class="flex gap-2 items-center ms-auto">
                <button
                    type="button"
                    onclick={() => scrcpy.handleKillAdb()}
                    class="ds-btn ds-btn-tertiary"
                    title={t('sidebar.killAdbTitle')}
                >
                    <Zap size={10} />
                    {t('sidebar.killAdb')}
                </button>
                <button
                    type="button"
                    onclick={() => scrcpy.refreshDevices()}
                    disabled={isRefreshing}
                    class="ds-btn ds-btn-tertiary"
                >
                    <RefreshCw size={10} class={isRefreshing ? 'animate-spin' : ''} />
                    {isRefreshing ? t('sidebar.syncing') : t('sidebar.refresh')}
                </button>
            </div>
        </div>

        <div class="space-y-2">
            <DeviceList
                devices={scrcpy.devices}
                deviceFriendlyNames={scrcpy.deviceFriendlyNames}
                deviceModels={scrcpy.deviceModels}
                unavailable={scrcpy.unavailableDevices}
                runningDevices={scrcpy.runningDevices}
                selectedDevice={scrcpy.activeDevice}
                wirelessSwitching={scrcpy.wirelessSwitching}
                onSelect={scrcpy.setActiveDevice}
                onGotoWireless={(device) => void scrcpy.handleGotoWireless(device)}
            />

            <div class="ds-tabs" role="tablist" aria-label={t('sidebar.deviceHub')}>
                <button
                    type="button"
                    role="tab"
                    aria-selected={activeTab === 'usb'}
                    data-active={activeTab === 'usb'}
                    onclick={() => (activeTab = 'usb')}
                    class="ds-tab"
                >
                    <Usb size={11} /> {t('sidebar.usb')}
                </button>
                <button
                    type="button"
                    role="tab"
                    aria-selected={activeTab === 'wireless'}
                    data-active={activeTab === 'wireless'}
                    onclick={() => (activeTab = 'wireless')}
                    class="ds-tab"
                >
                    <Wifi size={11} /> {t('sidebar.wireless')}
                </button>
            </div>

            {#if activeTab === 'usb'}
                <div class="pt-1">
                    <div class="ds-panel p-3 space-y-1.5">
                        <div class="flex items-center gap-2">
                            <span class="ds-dot" data-live="false"></span>
                            <span class="ds-label">{t('sidebar.usbSetupTip')}</span>
                        </div>
                        <p class="ds-caption" style="color: var(--text-muted)">
                            {t('sidebar.usbSetupTipTextBefore')}
                            <span class="ds-strong" style="color: var(--text-base)">{t('sidebar.usbSetupTipDeveloperOptions')}</span>
                            {t('sidebar.usbSetupTipAnd')}
                            <span class="ds-strong" style="color: var(--text-base)">{t('sidebar.usbSetupTipUsbDebugging')}</span>
                            {t('sidebar.usbSetupTipTextAfter')}
                        </p>
                    </div>
                </div>
            {/if}

            <!-- Keep the panel mounted so a half-typed IP or an open pairing
                 dialog survives a tab switch. -->
            <div class={activeTab === 'wireless' ? '' : 'hidden'}>
                <WirelessPanel />
            </div>
        </div>
    </div>

    <FileDropCard onBrowse={() => void scrcpy.handleFileBrowse()} />
</aside>
