<script lang="ts">
    import type { Component } from 'svelte';
    import Tooltip from './Tooltip.svelte';

    interface Props {
        checked: boolean;
        onChange: (value: boolean) => void;
        label: string;
        icon?: Component<{ size?: number }>;
        hint?: string;
        tooltip?: string;
        variant?: 'card' | 'compact';
    }

    let {
        checked,
        onChange,
        label,
        icon: Icon,
        hint,
        tooltip,
        variant = 'card'
    }: Props = $props();

    const rowClass = $derived(
        variant === 'compact'
            ? 'sb-row flex items-center justify-between gap-3 py-1 px-2'
            : 'cp-row flex items-center justify-between gap-3 px-3 py-2'
    );
</script>

<div class={rowClass}>
    <!-- The label is a mouse shortcut; the real keyboard/AT control is the
         switch button on the right, so the tooltip button is not nested
         inside it. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="flex items-center gap-2 min-w-0 flex-1 cursor-pointer"
        onclick={() => onChange(!checked)}
    >
        {#if Icon}
            {#if variant === 'compact'}
                <span
                    class="ds-inset flex items-center justify-center shrink-0"
                    style="padding: 4px; color: {checked ? 'var(--primary)' : 'var(--text-muted)'}"
                >
                    <Icon size={12} />
                </span>
            {:else}
                <span
                    style="color: {checked ? 'var(--primary)' : 'var(--text-muted)'}"
                    class="flex items-center shrink-0"
                >
                    <Icon size={12} />
                </span>
            {/if}
        {/if}
        <div class="min-w-0">
            <span class="ds-label truncate block">{label}</span>
            {#if hint}
                <span
                    class="block ds-caption truncate"
                    style="color: var(--text-muted); font-size: 10.5px; line-height: 13px"
                >
                    {hint}
                </span>
            {/if}
        </div>
    </div>

    {#if tooltip}
        <span class="shrink-0"><Tooltip text={tooltip} /></span>
    {/if}

    <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onclick={() => onChange(!checked)}
        class="shrink-0 border-0 bg-transparent p-0"
    >
        <span class="ds-toggle" data-on={checked ? 'true' : 'false'} aria-hidden="true">
            <span></span>
        </span>
    </button>
</div>
