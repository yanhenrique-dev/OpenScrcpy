<script lang="ts" generics="V extends string | number">
    import Select from '../Select.svelte';
    import { t } from '../../i18n';

    interface Props {
        value: V | undefined;
        onChange: (value: V) => void;
        options: { value: V; label: string }[];
        label?: string;
        ariaLabel?: string;
        disabled?: boolean;
        customLabel?: string;
        class?: string;
    }

    let {
        value,
        onChange,
        options,
        label,
        ariaLabel,
        disabled,
        customLabel,
        class: className = ''
    }: Props = $props();
</script>

<Select
    {value}
    {onChange}
    {options}
    {label}
    {ariaLabel}
    {disabled}
    customLabel={customLabel ?? t('controlPanel.custom')}
    triggerClassName="cp-select"
    optionClassName="cp-opt"
    class={className}
/>
