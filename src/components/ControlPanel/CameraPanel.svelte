<script lang="ts">
    import { ExternalLink, Video } from '@lucide/svelte';
    import SettingSelect from './SettingSelect.svelte';
    import SliderRow from '../SliderRow.svelte';
    import SwitchRow from '../SwitchRow.svelte';
    import Tooltip from '../Tooltip.svelte';
    import CodecSelect from './CodecSelect.svelte';
    import PerformanceGrid from './PerformanceGrid.svelte';
    import BitrateSlider from './BitrateSlider.svelte';
    import type { ConfigFieldSetter, ScrcpyConfig } from '../../lib/types';
    import type { RendererOption } from './rendererOptions';
    import { t } from '../../i18n';

    interface Props {
        config: ScrcpyConfig;
        handleChange: ConfigFieldSetter;
        rendererOptions: RendererOption[];
        detectedCameras: { id: string; name: string }[];
        onListOptions: (arg: string) => void;
    }

    let {
        config,
        handleChange,
        rendererOptions,
        detectedCameras,
        onListOptions
    }: Props = $props();
</script>

<div class="space-y-3">
    <div class="ds-panel p-3.5 flex items-center gap-3.5">
        <div class="ds-inset flex items-center justify-center shrink-0" style="width: 34px; height: 34px; color: var(--primary)">
            <Video size={15} />
        </div>
        <div class="min-w-0">
            <div class="flex items-center justify-between gap-2">
                <h3 class="ds-label ds-strong">{t('controlPanel.webcamProTip')}</h3>
                <a href="https://obsproject.com/" target="_blank" rel="noopener noreferrer" class="ds-link flex items-center gap-1 shrink-0">
                    {t('controlPanel.getObs')} <ExternalLink size={10} />
                </a>
            </div>
            <p class="ds-caption mt-[3px]" style="color: var(--text-secondary)">
                {t('controlPanel.webcamProTipTextBefore')}
                <span class="ds-strong">{t('controlPanel.webcamProTipObs')}</span>
                {t('controlPanel.webcamProTipAndStart')}
                <span class="ds-strong">{t('controlPanel.webcamProTipVirtualCamera')}</span>
                {t('controlPanel.webcamProTipTextAfter')}
            </p>
        </div>
    </div>

    <div class="space-y-1">
        <div class="flex justify-between items-center">
            <div class="flex items-center gap-1.5">
                <span class="ds-label">{t('controlPanel.cameraDevice')}</span>
                <Tooltip text={t('controlPanel.cameraDeviceTooltip')} placement="top" />
            </div>
            <div class="flex items-center gap-1.5">
                <Tooltip text={t('controlPanel.refreshLensesTooltip')} placement="top" />
                <button type="button" onclick={() => onListOptions('--list-cameras')} class="ds-btn-tertiary">
                    {t('controlPanel.refreshLenses')}
                </button>
            </div>
        </div>
        <SettingSelect
            value={config.cameraId || ''}
            onChange={(val) => handleChange('cameraId', val)}
            ariaLabel={t('controlPanel.cameraDevice')}
            options={[
                { value: '', label: t('controlPanel.autoSelect') },
                ...detectedCameras.map(cam => ({ value: cam.id, label: cam.name }))
            ]}
        />
    </div>

    <div class="grid grid-cols-2 gap-2">
        <CodecSelect value={config.codec} onChange={(val) => handleChange('codec', val)} />
        <SettingSelect
            label={t('controlPanel.aspect')}
            value={config.cameraAr || '0'}
            onChange={(val) => handleChange('cameraAr', val)}
            options={[
                { value: '0', label: t('controlPanel.aspectDefault') },
                { value: '16:9', label: '16:9' },
                { value: '4:3', label: '4:3' }
            ]}
        />
    </div>

    <div class="ds-panel p-2.5 space-y-2">
        <div class="flex items-center gap-2">
            <span class="ds-caption" style="color: var(--text-muted)">{t('controlPanel.cameraControls')}</span>
            <span class="ds-badge">{t('controlPanel.badgeNew')}</span>
        </div>
        <Tooltip text={t('controlPanel.cameraTorchTooltip')} block>
            <SwitchRow
                checked={!!config.cameraTorch}
                onChange={(v) => handleChange('cameraTorch', v)}
                label={t('controlPanel.cameraTorch')}
            />
        </Tooltip>
        <SliderRow
            label={t('controlPanel.cameraZoom')}
            tooltip={t('controlPanel.cameraZoomTooltip')}
            value={config.cameraZoom || 1.0}
            min={1}
            max={5}
            step={0.1}
            format={(v) => `${v.toFixed(1)}x`}
            onChange={(v) => handleChange('cameraZoom', v)}
        />
    </div>

    <div class="space-y-2.5">
        <PerformanceGrid
            {config}
            {handleChange}
            {rendererOptions}
            showCodec={false}
            showResolution={false}
        />
        <BitrateSlider value={config.bitrate} onChange={(v) => handleChange('bitrate', v)} />
    </div>
</div>
