<script lang="ts">
    import { Check, Copy, Download, Maximize2, Minimize2, Terminal, Trash2 } from '@lucide/svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { t } from '../i18n';
    import type { LogEntry, LogLevel } from '../lib/logs';
    import { scrcpy } from '../lib/scrcpy.svelte';
    import { storageSnapshot } from '../lib/storage';
    import { showAlert } from '../lib/alerts.svelte';

    const LEVEL_COLORS: Record<LogLevel, string> = {
        system: 'var(--log-info)',
        warn: 'var(--log-warn)',
        err: 'var(--log-err)',
        ok: 'var(--log-ok)'
    };

    function logColor(entry: LogEntry): string {
        if (entry.level) return LEVEL_COLORS[entry.level];
        const text = entry.text;
        if (/\[SYSTEM\]|\[INFO\]/i.test(text)) return LEVEL_COLORS.system;
        if (/\[ERR.+?\]|failed|falha|erro[ :]|error/i.test(text)) return LEVEL_COLORS.err;
        if (/\[WARN[ING]*\]|aviso/i.test(text)) return LEVEL_COLORS.warn;
        if (/\[OK\]|connected|descoberto|estabelecid|iniciad|concluíd|pronto|ready|success/i.test(text)) {
            return LEVEL_COLORS.ok;
        }
        return 'var(--log-text)';
    }

    let containerRef: HTMLDivElement;
    let stick = true;
    let isLive = $state(false);
    let command = $state('');
    let expanded = $state(false);
    let copied = $state(false);

    $effect(() => {
        const entries = scrcpy.logs;
        if (stick && containerRef) {
            containerRef.scrollTop = containerRef.scrollHeight;
        }
        if (entries.length > 0) {
            isLive = true;
            const timer = setTimeout(() => (isLive = false), 2000);
            return () => clearTimeout(timer);
        }
    });

    function handleScroll() {
        const el = containerRef;
        if (!el) return;
        stick = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    }

    function handleKeyDown(e: KeyboardEvent) {
        if (e.key === 'Enter' && command.trim()) {
            void scrcpy.runTerminalCommand(command.trim());
            command = '';
        }
    }

    async function copyLogs() {
        const text = scrcpy.logs.map(entry => `[${entry.time}] ${entry.text}`).join('\n');
        try {
            if (navigator.clipboard?.writeText) {
                await navigator.clipboard.writeText(text);
            } else {
                const area = document.createElement('textarea');
                area.value = text;
                area.style.position = 'fixed';
                area.style.opacity = '0';
                document.body.appendChild(area);
                area.select();
                document.execCommand('copy');
                area.remove();
            }
            copied = true;
            setTimeout(() => (copied = false), 1500);
        } catch {
            scrcpy.addLogs(t('logPanel.copyFailed'), 'err');
        }
    }

    async function exportReport() {
        const data = {
            timestamp: new Date().toISOString(),
            localStorage: storageSnapshot([
                'scrcpy_config',
                'scrcpy_color_mode',
                'scrcpy_onboarding_done',
                'scrcpy_locale'
            ]),
            logs: scrcpy.logs
        };
        try {
            const fileName = `openscrcpy-logs-${Date.now()}.json`;
            await invoke('save_report', {
                content: JSON.stringify(data, null, 2),
                name: fileName
            });
            scrcpy.addLogs(t('logPanel.diagnosticReportSaved', { fileName }));
        } catch (e) {
            console.error('Export failed:', e);
            showAlert({
                title: t('logPanel.reportTitle'),
                message: t('logs.genericError', { error: String(e) }),
                kind: 'error'
            });
        }
    }
</script>

<div
    class="force-dark rounded-lg {expanded ? 'h-[420px]' : 'h-[220px]'} flex-none overflow-hidden font-mono flex flex-col relative transition-[height] duration-200"
    style="border: 1px solid var(--log-border)"
>
    <div class="px-4 py-2.5 flex justify-between items-center shrink-0" style="border-bottom: 1px solid var(--log-border)">
        <div class="flex items-center gap-3">
            <Terminal size={12} />
            <div class="flex items-center gap-2">
                <span class="ds-label" style="color: var(--log-muted)">{t('logPanel.systemConsole')}</span>
                <span class="ds-dot" data-live={isLive}></span>
            </div>
        </div>
        <div class="flex gap-2 shrink-0">
            <button onclick={exportReport} class="ds-btn ds-btn-tertiary" title={t('logPanel.reportTitle')}>
                <Download size={10} />
                {t('logPanel.report')}
            </button>
            <button onclick={() => scrcpy.clearLogs()} class="ds-btn ds-btn-tertiary">
                <Trash2 size={10} />
                {t('logPanel.clear')}
            </button>
            <button
                onclick={copyLogs}
                class="ds-btn ds-btn-tertiary"
                title={copied ? t('logPanel.copied') : t('logPanel.copy')}
                aria-label={copied ? t('logPanel.copied') : t('logPanel.copy')}
            >
                {#if copied}<Check size={10} />{:else}<Copy size={10} />{/if}
            </button>
            <button
                onclick={() => (expanded = !expanded)}
                class="ds-btn ds-btn-tertiary"
                title={expanded ? t('logPanel.collapse') : t('logPanel.expand')}
                aria-label={expanded ? t('logPanel.collapse') : t('logPanel.expand')}
                aria-expanded={expanded}
            >
                {#if expanded}<Minimize2 size={10} />{:else}<Maximize2 size={10} />{/if}
            </button>
        </div>
    </div>

    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
        bind:this={containerRef}
        onscroll={handleScroll}
        class="flex-1 overflow-y-auto p-4 pt-2"
        role="log"
        tabindex="0"
        aria-live="polite"
        aria-relevant="additions"
        aria-label={t('logPanel.systemConsole')}
    >
        {#if scrcpy.logs.length === 0}
            <div class="h-full flex items-center justify-center">
                <span class="ds-caption" style="color: var(--log-muted)">{t('logPanel.waitingForSequence')}</span>
            </div>
        {:else}
            <div class="space-y-1">
                {#each scrcpy.logs as entry (entry.id)}
                    <div
                        class="flex gap-3 leading-relaxed py-0.5 ps-3"
                        style="border-left: 1px solid var(--log-border); font-size: 11px"
                    >
                        <span class="shrink-0 tabular-nums" style="color: var(--log-muted)">{entry.time}</span>
                        <span class="break-all" style="color: {logColor(entry)}">{entry.text}</span>
                    </div>
                {/each}
            </div>
        {/if}
    </div>

    <div class="px-4 py-2 flex items-center gap-2 shrink-0" style="border-top: 1px solid var(--log-border)">
        <label for="log-terminal-input" class="select-none" style="color: var(--primary); font-size: 11px; font-weight: 700">$</label>
        <input
            id="log-terminal-input"
            type="text"
            aria-label={t('logPanel.terminalPlaceholder')}
            bind:value={command}
            onkeydown={handleKeyDown}
            placeholder={t('logPanel.terminalPlaceholder')}
            class="ds-input flex-1 font-mono"
            style="background: transparent; border-color: transparent; color: var(--log-text); font-size: 11px"
        />
    </div>
</div>
