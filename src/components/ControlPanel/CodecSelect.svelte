<script lang="ts">
    import SettingSelect from './SettingSelect.svelte';
    import { t } from '../../i18n';

    interface Props {
        value: string | undefined;
        onChange: (value: string) => void;
    }
    let { value, onChange }: Props = $props();
</script>

<SettingSelect
    label={t('controlPanel.codec')}
    value={value || 'h264'}
    {onChange}
    options={[
        { value: 'h264', label: 'H.264' },
        { value: 'h265', label: 'H.265' },
        { value: 'av1', label: 'AV1' }
    ]}
/>
