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
    label={t('controlPanel.rotation')}
    value={value || '0'}
    {onChange}
    options={[
        { value: '0', label: '0°' },
        { value: '270', label: '-90°' },
        { value: '90', label: '+90°' },
        { value: '180', label: '180°' }
    ]}
/>
