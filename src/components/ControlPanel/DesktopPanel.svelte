<script lang="ts">
    import { Lock, Settings2, Unlock } from '@lucide/svelte';
    import SettingSelect from './SettingSelect.svelte';
    import SliderRow from '../SliderRow.svelte';
    import Tooltip from '../Tooltip.svelte';
    import PerformanceGrid from './PerformanceGrid.svelte';
    import BitrateSlider from './BitrateSlider.svelte';
    import type { ConfigFieldSetter, ScrcpyConfig } from '../../lib/types';
    import type { RendererOption } from './rendererOptions';
    import { t } from '../../i18n';

    interface Props {
        config: ScrcpyConfig;
        setConfig: (config: ScrcpyConfig) => void;
        handleChange: ConfigFieldSetter;
        rendererOptions: RendererOption[];
    }

    let { config, setConfig, handleChange, rendererOptions }: Props = $props();

    const vdPreset = $derived.by(() => {
        const w = config.vdWidth;
        const h = config.vdHeight;
        if (w === 1920 && h === 1080) return '1080p';
        if (w === 2560 && h === 1440) return '1440p';
        if (w === 3840 && h === 2160) return '4k';
        if (w === 2560 && h === 1080) return 'ultrawide';
        return 'custom';
    });
</script>

<div class="space-y-3">
    <div class="ds-panel p-3 space-y-4">
        <div class="flex items-center justify-between" style="border-bottom: 1px solid var(--border-base); padding-bottom: 8px">
            <div class="flex items-center gap-2">
                <Settings2 size={12} />
                <h3 class="ds-label ds-strong">{t('controlPanel.virtualDisplayEngine')}</h3>
            </div>
            <div class="flex items-center gap-2">
                <Tooltip text={t('controlPanel.flexDisplayTooltip')}>
                    <button
                        type="button"
                        aria-pressed={!!config.flexDisplay}
                        onclick={() => handleChange('flexDisplay', !config.flexDisplay)}
                        class="ds-btn-tertiary"
                        style={config.flexDisplay ? 'color: var(--primary)' : undefined}
                        title={t('controlPanel.flexDisplay')}
                    >
                        <span class="ds-badge" data-live={config.flexDisplay ? 'true' : 'false'}>{t('controlPanel.flexDisplay')}</span>
                    </button>
                </Tooltip>
                <button
                    type="button"
                    aria-pressed={!!config.aspectRatioLock}
                    onclick={() => handleChange('aspectRatioLock', !config.aspectRatioLock)}
                    class="ds-btn-tertiary"
                    style={config.aspectRatioLock ? 'color: var(--primary)' : undefined}
                    title={t('controlPanel.ratioLockTitle')}
                >
                    {#if config.aspectRatioLock}<Lock size={10} />{:else}<Unlock size={10} />{/if}
                    <span>{t('controlPanel.ratioLock')}</span>
                </button>
            </div>
        </div>

        <div class="grid grid-cols-2 gap-x-4 gap-y-3">
            <SliderRow
                label={t('controlPanel.width')}
                value={config.vdWidth || 1920}
                min={480}
                max={3840}
                format={(v) => `${v}px`}
                onChange={(val) => {
                    if (config.aspectRatioLock && config.vdWidth && config.vdHeight) {
                        const ratio = config.vdHeight / config.vdWidth;
                        setConfig({ ...config, vdWidth: val, vdHeight: Math.round(val * ratio) });
                    } else {
                        handleChange('vdWidth', val);
                    }
                }}
            />
            <SliderRow
                label={t('controlPanel.height')}
                value={config.vdHeight || 1080}
                min={360}
                max={2160}
                format={(v) => `${v}px`}
                onChange={(val) => {
                    if (config.aspectRatioLock && config.vdWidth && config.vdHeight) {
                        const ratio = config.vdWidth / config.vdHeight;
                        setConfig({ ...config, vdHeight: val, vdWidth: Math.round(val * ratio) });
                    } else {
                        handleChange('vdHeight', val);
                    }
                }}
            />
            <SliderRow
                label={t('controlPanel.uiScaling')}
                tooltip={t('controlPanel.uiScalingTooltip')}
                value={config.vdDpi || 420}
                min={120}
                max={640}
                format={(v) => `${v} DPI`}
                onChange={(val) => handleChange('vdDpi', val)}
            />
            <SettingSelect
                label={t('controlPanel.quickPresets')}
                value={vdPreset}
                onChange={(val) => {
                    if (val === '1080p') setConfig({ ...config, vdWidth: 1920, vdHeight: 1080 });
                    if (val === '1440p') setConfig({ ...config, vdWidth: 2560, vdHeight: 1440 });
                    if (val === '4k') setConfig({ ...config, vdWidth: 3840, vdHeight: 2160 });
                    if (val === 'ultrawide') setConfig({ ...config, vdWidth: 2560, vdHeight: 1080 });
                }}
                options={[
                    { value: '1080p', label: t('controlPanel.preset1080p') },
                    { value: '1440p', label: t('controlPanel.preset1440p') },
                    { value: '4k', label: t('controlPanel.preset4k') },
                    { value: 'ultrawide', label: t('controlPanel.presetUltrawide') }
                ]}
            />
        </div>
    </div>

    <div class="space-y-2.5">
        <PerformanceGrid {config} {handleChange} {rendererOptions} showResolution={false} />
        <BitrateSlider value={config.bitrate} onChange={(v) => handleChange('bitrate', v)} />
        <div class="space-y-1">
            <div class="flex items-center gap-1.5">
                <label class="ds-label" for="cp-background-color">{t('controlPanel.backgroundColor')}</label>
                <Tooltip text={t('controlPanel.backgroundColorTooltip')} placement="top" />
            </div>
            <div class="flex items-center gap-2">
                <div
                    class="shrink-0"
                    style="width: 20px; height: 20px; border-radius: var(--radius-sm); border: 1px solid var(--border-base); background-color: {config.backgroundColor || '#222222'}"
                ></div>
                <input
                    id="cp-background-color"
                    type="text"
                    placeholder="#1a1a1a"
                    value={config.backgroundColor || ''}
                    oninput={(e) => handleChange('backgroundColor', (e.currentTarget as HTMLInputElement).value)}
                    class="ds-input flex-1 font-mono"
                />
                {#if config.backgroundColor}
                    <button type="button" onclick={() => handleChange('backgroundColor', '')} class="ds-btn-tertiary">
                        {t('controlPanel.backgroundColorNone')}
                    </button>
                {/if}
            </div>
        </div>
    </div>
</div>
