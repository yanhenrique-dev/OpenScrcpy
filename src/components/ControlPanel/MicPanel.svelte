<script lang="ts">
    import { Mic } from '@lucide/svelte';
    import SettingSelect from './SettingSelect.svelte';
    import MicMeter from './MicMeter.svelte';
    import type { ConfigFieldSetter, ScrcpyConfig } from '../../lib/types';
    import { t } from '../../i18n';

    interface Props {
        config: ScrcpyConfig;
        handleChange: ConfigFieldSetter;
        isRunning: boolean;
    }

    let { config, handleChange, isRunning }: Props = $props();
</script>

<div class="space-y-3">
    <div class="ds-panel p-3 space-y-3">
        <div class="flex items-center gap-2" style="border-bottom: 1px solid var(--border-base); padding-bottom: 8px">
            <Mic size={12} />
            <h3 class="ds-label ds-strong">{t('controlPanel.micBridgeTitle')}</h3>
            {#if isRunning}
                <span class="ds-caption" style="margin-left: auto; color: var(--status-ok)">{t('controlPanel.micActive')}</span>
            {/if}
        </div>

        <MicMeter sessionRunning={isRunning} />

        <div class="grid grid-cols-2 gap-x-4 gap-y-3">
            <SettingSelect
                label={t('controlPanel.micSourceLabel')}
                value={config.micSource || 'mic'}
                onChange={(val) => handleChange('micSource', val)}
                options={[
                    { value: 'mic', label: t('controlPanel.micSourceMic') },
                    { value: 'mic-voice-communication', label: t('controlPanel.micSourceVoiceComm') }
                ]}
            />
            <SettingSelect
                label={t('controlPanel.micQualityLabel')}
                value={String(config.micBitrate || 128)}
                onChange={(val) => handleChange('micBitrate', Number(val))}
                options={[
                    { value: '64', label: '64 kbps' },
                    { value: '128', label: '128 kbps' },
                    { value: '192', label: '192 kbps' },
                    { value: '256', label: '256 kbps' }
                ]}
            />
            <SettingSelect
                label={t('controlPanel.micCodecLabel')}
                value={config.micCodec || 'opus'}
                onChange={(val) => handleChange('micCodec', val)}
                options={[
                    { value: 'opus', label: 'Opus' },
                    { value: 'aac', label: 'AAC' }
                ]}
            />
            <div class="flex items-end">
                <button
                    type="button"
                    aria-pressed={!!config.micDenoise}
                    onclick={() => handleChange('micDenoise', !config.micDenoise)}
                    class="ds-btn-tertiary"
                    style={config.micDenoise ? 'color: var(--primary)' : undefined}
                    title={t('controlPanel.micDenoiseTooltip')}
                >
                    <span>{t('controlPanel.micDenoiseLabel')}</span>
                </button>
            </div>
        </div>

        <p class="ds-caption" style="color: var(--text-secondary); line-height: 1.5">{t('controlPanel.micBridgeDesc')}</p>
        <div class="ds-caption" style="color: var(--text-muted); line-height: 1.5; display: grid; gap: 6px">
            <span>{t('controlPanel.micAndroidReq')}</span>
            <span>{t('controlPanel.micDestLinux')}</span>
            <span>{t('controlPanel.micDestOther')}</span>
        </div>
    </div>
    <p class="ds-caption" style="color: var(--text-muted); line-height: 1.5">
        {isRunning ? t('controlPanel.micStopHint') : t('controlPanel.micStartHint')}
    </p>
</div>
