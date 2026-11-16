<script lang="ts">
    import SliderRow from '../SliderRow.svelte';
    import { t } from '../../i18n';

    interface Props {
        value: number | undefined;
        onChange: (value: number) => void;
    }

    let { value, onChange }: Props = $props();
</script>

<SliderRow
    label={t('controlPanel.bitrate')}
    value={value || 8}
    min={1}
    max={50}
    format={(v) => `${v}M`}
    {onChange}
/>
