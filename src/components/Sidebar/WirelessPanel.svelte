<script lang="ts">
    import { RefreshCw, Wifi } from '@lucide/svelte';
    import SheetModal from '../SheetModal.svelte';
    import { t } from '../../i18n';
    import { scrcpy } from '../../lib/scrcpy.svelte';
    import { isMdnsDeviceConnected, mdnsHostIp, type MdnsDevice } from '../../lib/types';

    let connectIp = $state('');
    let pairIp = $state('');
    let pairCode = $state('');
    let pairingTarget = $state<MdnsDevice | null>(null);
    let modalCode = $state('');
    let modalError = $state(false);
    let modalSubmitting = $state(false);
    let connecting = $state(false);
    let pairing = $state(false);

    const devices = $derived(scrcpy.devices);
    const historyDevices = $derived(scrcpy.historyDevices);
    const mdnsDevices = $derived(scrcpy.mdnsDevices);
    const isRefreshing = $derived(scrcpy.isRefreshing);

    const uniqueMdnsDevices = $derived.by(() => {
        const byHost = new Map<string, MdnsDevice>();
        for (const dev of mdnsDevices) {
            const key = mdnsHostIp(dev.address);
            const existing = byHost.get(key);
            const devIsPairing = dev.service.includes('_adb-tls-pairing');
            const existingIsPairing = existing?.service.includes('_adb-tls-pairing') ?? false;
            if (!existing || (devIsPairing && !existingIsPairing)) byHost.set(key, dev);
        }
        return Array.from(byHost.values());
    });

    const unconnectedMdnsDevices = $derived(
        uniqueMdnsDevices.filter(dev => !isMdnsDeviceConnected(dev, devices))
    );

    const isPairingModal = $derived(!!pairingTarget?.service.includes('_adb-tls-pairing'));
    const pairingModalTitle = $derived(
        isPairingModal ? t('sidebar.pairModalTitle') : t('sidebar.pairModalNotReadyTitle')
    );

    function formatMdnsName(name: string): string {
        return name.replace(/^adb-/, '');
    }

    $effect(() => {
        if (!pairingTarget || pairingTarget.service.includes('_adb-tls-pairing')) return;
        const targetIp = mdnsHostIp(pairingTarget.address);
        const ready = uniqueMdnsDevices.find(
            d => d.service.includes('_adb-tls-pairing') && mdnsHostIp(d.address) === targetIp
        );
        if (ready) pairingTarget = ready;
    });

    $effect(() => {
        const waiting = !!pairingTarget && !pairingTarget.service.includes('_adb-tls-pairing');
        if (!waiting) return;
        const interval = setInterval(() => {
            if (document.visibilityState === 'hidden') return;
            void scrcpy.refreshDevices();
        }, 2000);
        return () => clearInterval(interval);
    });

    async function handleConnect(ip: string) {
        if (!ip || connecting) return;
        connecting = true;
        try {
            await scrcpy.connectDevice(ip);
        } finally {
            connecting = false;
        }
    }

    async function completePairing(ip: string, code: string) {
        return scrcpy.pairDevice(ip, code);
    }

    async function submitPairingModal() {
        if (!pairingTarget || !modalCode || modalSubmitting) return;
        modalSubmitting = true;
        modalError = false;
        const res = await completePairing(pairingTarget.address, modalCode);
        modalSubmitting = false;
        if (res.success) {
            pairingTarget = null;
            modalCode = '';
        } else {
            modalError = true;
        }
    }

    function openPairing(dev: MdnsDevice) {
        pairingTarget = dev;
        modalCode = '';
        modalError = false;
    }
</script>

<div class="space-y-5 pt-2">
    <div class="ds-panel p-3 space-y-1.5">
        <div class="flex items-center gap-2">
            <span class="ds-dot" data-live="false"></span>
            <span class="ds-label">{t('sidebar.wirelessSetupTip')}</span>
        </div>
        <p class="ds-caption" style="color: var(--text-muted)">
            {t('sidebar.wirelessSetupTipTextBefore')}
            <span class="ds-strong" style="color: var(--text-base)">{t('sidebar.wirelessSetupTipSameWifi')}</span>
            {t('sidebar.wirelessSetupTipAnd')}
            <span class="ds-strong" style="color: var(--text-base)">{t('sidebar.wirelessSetupTipWirelessDebugging')}</span>
            {t('sidebar.wirelessSetupTipTextAfter')}
        </p>
    </div>

    <div class="space-y-3">
        <div class="pb-1.5" style="border-bottom: 1px solid var(--border-base)">
            <label class="ds-label" for="sidebar-connect-ip">{t('sidebar.ipConnect')}</label>
        </div>
        <div class="flex gap-2">
            <div class="relative flex-1 flex items-center gap-2">
                <Wifi size={10} class="shrink-0" />
                <input
                    id="sidebar-connect-ip"
                    type="text"
                    placeholder={t('sidebar.ipPlaceholder')}
                    bind:value={connectIp}
                    inputmode="url"
                    autocapitalize="off"
                    autocorrect="off"
                    spellcheck="false"
                    class="ds-input w-full"
                />
            </div>
            <button onclick={() => handleConnect(connectIp)} disabled={connecting} class="ds-btn ds-btn-secondary">
                {connecting ? t('sidebar.connecting') : t('sidebar.connect')}
            </button>
        </div>
    </div>

    {#if unconnectedMdnsDevices.length > 0}
        <div class="space-y-3 pt-1">
            <div class="flex items-center justify-between pb-1.5" style="border-bottom: 1px solid var(--border-base)">
                <span class="ds-label">{t('sidebar.discoveredDevices')}</span>
            </div>
            <p class="ds-caption" style="color: var(--text-muted)">{t('sidebar.discoveredHint')}</p>
            <div class="space-y-2">
                {#each unconnectedMdnsDevices as dev (dev.name)}
                    <button
                        disabled={connecting || isRefreshing}
                        onclick={() => openPairing(dev)}
                        class="ds-panel w-full flex items-center justify-between p-2 text-start"
                    >
                        <div class="flex items-center gap-2 min-w-0">
                            <Wifi size={10} class="shrink-0" />
                            <span class="ds-label truncate" title="{dev.address} ({formatMdnsName(dev.name)})">
                                {dev.address} ({formatMdnsName(dev.name)})
                            </span>
                        </div>
                        <span class="ds-caption shrink-0 ms-2" style="color: var(--primary)">{t('sidebar.startPairing')}</span>
                    </button>
                {/each}
            </div>
        </div>
    {/if}

    {#if historyDevices.length > 0}
        <div class="space-y-3 pt-1">
            <div class="flex items-center justify-between pb-1.5" style="border-bottom: 1px solid var(--border-base)">
                <span class="ds-label">{t('sidebar.recentDevices')}</span>
                <button onclick={() => scrcpy.clearHistory()} class="ds-btn ds-btn-tertiary">{t('sidebar.clear')}</button>
            </div>
            <div class="space-y-2">
                {#each historyDevices as ip (ip)}
                    <button
                        disabled={connecting || isRefreshing}
                        onclick={() => {
                            connectIp = ip;
                            void handleConnect(ip);
                        }}
                        class="ds-panel w-full flex items-center justify-between p-2"
                    >
                        <span class="flex items-center gap-2">
                            <Wifi size={10} />
                            <span class="ds-label">{ip}</span>
                        </span>
                        <span class="ds-caption" style="color: var(--primary)">{t('sidebar.connect')}</span>
                    </button>
                {/each}
            </div>
        </div>
    {/if}

    <div class="space-y-3 pt-1">
        <div class="pb-1.5" style="border-bottom: 1px solid var(--border-base)">
            <span class="ds-label">{t('sidebar.pairDeviceTitle')}</span>
        </div>
        <div class="grid grid-cols-2 gap-2">
            <div class="space-y-1">
                <label class="ds-caption" for="sidebar-pair-ip" style="color: var(--text-muted)">{t('sidebar.ipPortPlaceholder')}</label>
                <input
                    id="sidebar-pair-ip"
                    type="text"
                    placeholder={t('sidebar.ipPortPlaceholder')}
                    bind:value={pairIp}
                    inputmode="url"
                    autocapitalize="off"
                    autocorrect="off"
                    spellcheck="false"
                    autocomplete="off"
                    class="ds-input w-full"
                />
            </div>
            <div class="space-y-1">
                <label class="ds-caption" for="sidebar-pair-code" style="color: var(--text-muted)">{t('sidebar.pairingCodePlaceholder')}</label>
                <input
                    id="sidebar-pair-code"
                    type="text"
                    placeholder={t('sidebar.pairingCodePlaceholder')}
                    bind:value={pairCode}
                    inputmode="numeric"
                    maxlength="6"
                    autocomplete="one-time-code"
                    class="ds-input w-full"
                    style={pairIp ? 'border-style: dashed' : undefined}
                />
            </div>
        </div>
        <button
            onclick={async () => {
                if (!pairIp || !pairCode || pairing) return;
                pairing = true;
                try {
                    const res = await completePairing(pairIp, pairCode);
                    if (res.success) pairCode = '';
                } finally {
                    pairing = false;
                }
            }}
            disabled={pairing || isRefreshing}
            class="ds-btn ds-btn-primary w-full"
        >
            {pairing ? t('sidebar.synchronizing') : t('sidebar.startPairing')}
        </button>
    </div>
</div>

{#if pairingTarget}
    <SheetModal
        isOpen
        onClose={() => (pairingTarget = null)}
        title={pairingModalTitle}
        overlayClassName="p-6"
        panelClassName="w-full max-w-sm p-6"
    >
        {#snippet footer()}
            {#if isPairingModal}
                <button onclick={submitPairingModal} disabled={!modalCode || modalSubmitting} class="ds-btn ds-btn-primary w-full">
                    {modalSubmitting ? t('sidebar.synchronizing') : t('common.ok')}
                </button>
            {:else}
                <button onclick={() => (pairingTarget = null)} class="ds-btn ds-btn-secondary w-full">{t('common.cancel')}</button>
            {/if}
        {/snippet}
        <p class="ds-caption truncate mb-4" style="color: var(--text-muted)">
            {pairingTarget.address} ({formatMdnsName(pairingTarget.name)})
        </p>
        {#if isPairingModal}
            <label class="ds-label block mb-1" for="sidebar-modal-code">{t('sidebar.pairingCodePlaceholder')}</label>
            <input
                id="sidebar-modal-code"
                data-autofocus
                type="text"
                placeholder={t('sidebar.pairingCodePlaceholder')}
                bind:value={modalCode}
                inputmode="numeric"
                maxlength="6"
                autocomplete="one-time-code"
                oninput={() => (modalError = false)}
                onkeydown={(e) => {
                    if (e.key === 'Enter') void submitPairingModal();
                }}
                class="ds-input w-full mb-2"
            />
            {#if modalError}
                <p class="ds-badge mb-2" style="border-style: dashed">{t('sidebar.pairModalError')}</p>
            {/if}
        {:else}
            <p class="ds-caption mb-4" style="color: var(--text-secondary)">{t('sidebar.pairModalNotReadyBody')}</p>
            <div class="flex items-center justify-center gap-2" style="color: var(--text-muted)">
                <RefreshCw size={12} class="animate-spin" />
                <span class="ds-label">{t('sidebar.pairModalWaiting')}</span>
            </div>
        {/if}
    </SheetModal>
{/if}
