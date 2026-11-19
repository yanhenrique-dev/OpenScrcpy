<script lang="ts">
    import { AppWindow, Bell, RefreshCw, Settings2 } from '@lucide/svelte';
    import SwitchRow from './SwitchRow.svelte';
    import { t } from '../i18n';
    import { scrcpy } from '../lib/scrcpy.svelte';
</script>

<div class="ds-card p-3 space-y-2">
    <div
        class="flex items-center gap-2"
        style="border-bottom: 1px solid var(--border-base); padding-bottom: 8px"
    >
        <Settings2 size={12} style="color: var(--text-muted)" />
        <h2 class="ds-label ds-strong">{t('appBehavior.title')}</h2>
    </div>

    <div class="flex flex-col gap-1">
        <SwitchRow
            variant="compact"
            checked={scrcpy.backgroundMode}
            onChange={(value) => scrcpy.setBackgroundMode(value)}
            icon={AppWindow}
            label={t('appBehavior.keepInBackground')}
            tooltip={t('appBehavior.keepInBackgroundTooltip')}
        />
        <SwitchRow
            variant="compact"
            checked={scrcpy.notificationsEnabled}
            onChange={(value) => void scrcpy.setNotificationsEnabled(value)}
            icon={Bell}
            label={t('appBehavior.notifications')}
            tooltip={t('appBehavior.notificationsTooltip')}
        />
        <SwitchRow
            variant="compact"
            checked={scrcpy.config.autoReconnect || false}
            onChange={(value) => scrcpy.updateConfig({ autoReconnect: value })}
            icon={RefreshCw}
            label={t('appBehavior.autoReconnect')}
            tooltip={t('appBehavior.autoReconnectTooltip')}
        />
    </div>
</div>
