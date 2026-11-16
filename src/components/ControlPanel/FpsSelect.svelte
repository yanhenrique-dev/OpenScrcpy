<script lang="ts">
    import SettingSelect from './SettingSelect.svelte';
    import { t } from '../../i18n';

    interface Props {
        value: number | undefined;
        onChange: (value: number | undefined) => void;
    }
    let { value, onChange }: Props = $props();
</script>

<SettingSelect
    label={t('controlPanel.fps')}
    value={value ?? 0}
    onChange={(next) => onChange(next === 0 ? undefined : next)}
    options={[
        { value: 0, label: t('controlPanel.rendererAuto') || 'Automatic' },
        { value: 30, label: '30' },
        { value: 60, label: '60' },
        { value: 90, label: '90' },
        { value: 120, label: '120' }
    ]}
/>
