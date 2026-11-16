<script lang="ts">
    import { Play, Square, Monitor, Camera, LayoutGrid, Mic } from '@lucide/svelte';
    import type { RenderDriverSupport, ScrcpyConfig } from '../lib/types';
    import MirrorPanel from './ControlPanel/MirrorPanel.svelte';
    import CameraPanel from './ControlPanel/CameraPanel.svelte';
    import DesktopPanel from './ControlPanel/DesktopPanel.svelte';
    import MicPanel from './ControlPanel/MicPanel.svelte';
    import { buildRendererOptions } from './ControlPanel/rendererOptions';
    import { t } from '../i18n';

    interface Props {
        config: ScrcpyConfig;
        setConfig: (config: ScrcpyConfig) => void;
        onStart: () => void;
        onStop: () => void;
        isRunning: boolean;
        isStarting?: boolean;
        onListOptions: (arg: string) => void;
        detectedCameras?: { id: string; name: string }[];
        renderDriverSupport?: RenderDriverSupport;
    }

    let {
        config,
        setConfig,
        onStart,
        onStop,
        isRunning,
        isStarting = false,
        onListOptions,
        detectedCameras = [],
        renderDriverSupport = {
            hostOs: 'unknown',
            supportsRenderDriver: false,
            supportedDrivers: []
        }
    }: Props = $props();

    function handleChange<K extends keyof ScrcpyConfig>(field: K, value: ScrcpyConfig[K]) {
        setConfig({ ...config, [field]: value });
    }

    const rendererOptions = $derived(buildRendererOptions(renderDriverSupport, t('controlPanel.rendererAuto')));
    const startLabel = $derived.by(() => {
        if (config.sessionMode === 'mirror') {
            const pureHid = (config.hidKeyboard || config.hidMouse) && config.otgPure;
            return pureHid ? t('controlPanel.initializeOtg') : t('controlPanel.startMission');
        }
        if (config.sessionMode === 'camera') return t('controlPanel.engageCamera');
        if (config.sessionMode === 'mic') return t('controlPanel.startMicBridge');
        return t('controlPanel.ejectToDesktop');
    });

</script>

<main class="lg:col-span-6 space-y-4">
    <section class="ds-card p-3">
        <span class="ds-label block" style="color: var(--text-secondary); margin-bottom: 8px">{t('controlPanel.captureSource')}</span>
        <div class="ds-tabs" role="tablist" aria-label={t('controlPanel.captureSource')}>
            <button
                type="button"
                role="tab"
                aria-selected={config.sessionMode === 'mirror'}
                data-active={config.sessionMode === 'mirror'}
                onclick={() => handleChange('sessionMode', 'mirror')}
                class="ds-tab"
            >
                <Monitor size={16} />
                <span>{t('controlPanel.screen')}</span>
            </button>
            <button
                type="button"
                role="tab"
                aria-selected={config.sessionMode === 'camera'}
                data-active={config.sessionMode === 'camera'}
                onclick={() => handleChange('sessionMode', 'camera')}
                class="ds-tab"
            >
                <Camera size={16} />
                <span>{t('controlPanel.camera')}</span>
            </button>
            <button
                type="button"
                role="tab"
                aria-selected={config.sessionMode === 'desktop'}
                data-active={config.sessionMode === 'desktop'}
                onclick={() => handleChange('sessionMode', 'desktop')}
                class="ds-tab"
            >
                <LayoutGrid size={16} />
                <span>{t('controlPanel.desktop')}</span>
            </button>
            <button
                type="button"
                role="tab"
                aria-selected={config.sessionMode === 'mic'}
                data-active={config.sessionMode === 'mic'}
                onclick={() => handleChange('sessionMode', 'mic')}
                class="ds-tab"
            >
                <Mic size={16} />
                <span>{t('controlPanel.microphone')}</span>
            </button>
        </div>
    </section>

    <section class="ds-card p-3 space-y-3">
        <div class="flex justify-between items-center" style="border-bottom: 1px solid var(--border-base); padding-bottom: 8px">
            <h2 class="ds-label ds-strong">{t('controlPanel.engineConfiguration')}</h2>
            <div class="flex gap-1.5">
                {#if config.sessionMode === 'mirror' && (config.hidKeyboard || config.hidMouse) && config.otgPure}
                    <span class="ds-badge">{t('controlPanel.otgOnly')}</span>
                {/if}
                <span class="ds-badge" data-live={isRunning}>{isRunning ? t('controlPanel.active') : t('controlPanel.ready')}</span>
            </div>
        </div>

        <div class="space-y-2.5">
            {#if config.sessionMode === 'mirror'}
                <MirrorPanel {config} {handleChange} {rendererOptions} />
            {:else if config.sessionMode === 'camera'}
                <CameraPanel
                    {config}
                    {handleChange}
                    {rendererOptions}
                    {detectedCameras}
                    {onListOptions}
                />
            {:else if config.sessionMode === 'desktop'}
                <DesktopPanel {config} {setConfig} {handleChange} {rendererOptions} />
            {:else}
                <MicPanel {config} {handleChange} {isRunning} />
            {/if}
        </div>
    </section>

    <div class="pt-2">
        {#if !isRunning}
            <button
                type="button"
                onclick={onStart}
                disabled={isStarting}
                aria-busy={isStarting}
                class="ds-btn ds-btn-primary w-full"
                style="padding: 12px 16px"
            >
                <span class="flex items-center justify-center gap-3">
                    <Play size={18} />
                    {startLabel}
                </span>
            </button>
        {:else}
            <button type="button" onclick={onStop} class="ds-btn ds-btn-danger w-full" style="padding: 12px 16px">
                <span class="flex items-center justify-center gap-3">
                    <Square size={18} />
                    {t('controlPanel.stopSession')}
                </span>
            </button>
        {/if}
    </div>
</main>
