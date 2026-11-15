<script lang="ts">
    import { Keyboard, Mouse } from '@lucide/svelte';
    import SwitchRow from '../SwitchRow.svelte';
    import Tooltip from '../Tooltip.svelte';
    import PerformanceGrid from './PerformanceGrid.svelte';
    import BitrateSlider from './BitrateSlider.svelte';
    import type { ConfigFieldSetter, ScrcpyConfig } from '../../lib/types';
    import type { RendererOption } from './rendererOptions';
    import { t } from '../../i18n';

    interface Props {
        config: ScrcpyConfig;
        handleChange: ConfigFieldSetter;
        rendererOptions: RendererOption[];
    }

    let { config, handleChange, rendererOptions }: Props = $props();
</script>

<div class="space-y-2.5">
    <div class="ds-panel p-3 space-y-3">
        <div class="flex items-center gap-2">
            <Keyboard size={12} />
            <span class="ds-label ds-strong">{t('controlPanel.inputEnhancements')}</span>
        </div>
        <p class="ds-caption" style="color: var(--text-secondary)">{t('controlPanel.inputEnhancementsDescription')}</p>
        <div class="grid grid-cols-2 gap-3">
            <Tooltip text={t('controlPanel.hidKeyboardTooltip')} block>
                <SwitchRow
                    checked={!!config.hidKeyboard}
                    onChange={(v) => handleChange('hidKeyboard', v)}
                    icon={Keyboard}
                    label={t('controlPanel.hidKeyboard')}
                />
            </Tooltip>
            <Tooltip text={t('controlPanel.hidMouseTooltip')} block>
                <SwitchRow
                    checked={!!config.hidMouse}
                    onChange={(v) => handleChange('hidMouse', v)}
                    icon={Mouse}
                    label={t('controlPanel.hidMouse')}
                />
            </Tooltip>
            {#if config.hidKeyboard || config.hidMouse}
                <Tooltip text={t('controlPanel.pureHidTooltip')} block>
                    <SwitchRow
                        checked={!!config.otgPure}
                        onChange={(v) => handleChange('otgPure', v)}
                        label={t('controlPanel.pureHid')}
                    />
                </Tooltip>
            {/if}
        </div>
    </div>

    <div
        class="space-y-2.5"
        aria-disabled={!!config.otgPure}
        inert={config.otgPure ? true : undefined}
        style={config.otgPure ? 'opacity: 0.45' : 'opacity: 1'}
    >
        <PerformanceGrid {config} {handleChange} {rendererOptions} />
        <BitrateSlider value={config.bitrate} onChange={(v) => handleChange('bitrate', v)} />
    </div>
</div>
