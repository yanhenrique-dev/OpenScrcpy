<script lang="ts">
    import { UploadCloud } from '@lucide/svelte';
    import { t } from '../../i18n';

    interface Props {
        onBrowse: () => void;
    }

    let { onBrowse }: Props = $props();
</script>

<div
    onclick={onBrowse}
    onkeydown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onBrowse();
        }
    }}
    role="button"
    tabindex={0}
    aria-label={t('sidebar.flashPushFiles')}
    class="ds-panel p-5 flex flex-col items-center justify-center gap-2 cursor-pointer"
    style="border-style: dashed"
>
    <div class="ds-inset p-3 rounded-full flex items-center justify-center">
        <UploadCloud size={24} />
    </div>
    <div class="text-center">
        <h3 class="ds-label">{t('sidebar.flashPushFiles')}</h3>
        <p class="ds-caption mt-1" style="color: var(--text-muted)">{t('sidebar.flashPushSubtitle')}</p>
    </div>
</div>
