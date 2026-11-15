import { beforeEach, describe, expect, it } from 'vitest';
import { render } from '@testing-library/svelte';
import axe from 'axe-core';
import ControlPanel from '../ControlPanel.svelte';
import Header from '../Header.svelte';
import LogPanel from '../LogPanel.svelte';
import SessionBehavior from '../SessionBehavior.svelte';
import Sidebar from '../Sidebar.svelte';
import { DEFAULT_CONFIG, scrcpy } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';
import type { ScrcpyConfig } from '../../lib/types';

const DISABLED_RULES: axe.RunOptions['rules'] = {
    // jsdom has no layout/canvas, so axe cannot evaluate computed contrast.
    'color-contrast': { enabled: false },
    // These components are fragments inside the app shell, not standalone pages.
    'landmark-one-main': { enabled: false },
    'page-has-heading-one': { enabled: false },
    region: { enabled: false }
};

async function expectNoViolations(container: HTMLElement) {
    const results = await axe.run(container, { rules: DISABLED_RULES });
    const summary = results.violations.map(
        violation => `${violation.id}: ${violation.help} (${violation.nodes.length})`
    );
    expect(summary).toEqual([]);
}

function controlPanelProps(config: ScrcpyConfig = { ...DEFAULT_CONFIG }) {
    return {
        config,
        setConfig: () => {},
        onStart: () => {},
        onStop: () => {},
        isRunning: false,
        onListOptions: () => {}
    };
}

function headerProps() {
    return {
        colorMode: 'system' as const,
        onColorModeChange: () => {},
        binaryStatus: { found: true, message: 'Scrcpy Ready' },
        onDownload: () => {},
        onSetPath: () => {},
        onResetPath: () => {},
        isDownloading: false,
        downloadProgress: 0,
        version: '2.0.0'
    };
}

describe('accessibility', () => {
    beforeEach(async () => {
        await setLocale('en');
        scrcpy.logs = [];
        scrcpy.config = { ...DEFAULT_CONFIG };
        scrcpy.devices = [];
    });

    it('keeps the mirror control panel free of axe violations', async () => {
        const { container } = render(ControlPanel, { props: controlPanelProps() });
        await expectNoViolations(container);
    });

    it('keeps the camera control panel free of axe violations', async () => {
        const { container } = render(ControlPanel, {
            props: controlPanelProps({ ...DEFAULT_CONFIG, sessionMode: 'camera' })
        });
        await expectNoViolations(container);
    });

    it('keeps the console free of axe violations', async () => {
        const { container } = render(LogPanel);
        await expectNoViolations(container);
    });

    it('keeps the header free of axe violations', async () => {
        const { container } = render(Header, { props: headerProps() });
        await expectNoViolations(container);
    });

    it('keeps the session behavior panel free of axe violations', async () => {
        const { container } = render(SessionBehavior);
        await expectNoViolations(container);
    });

    it('keeps the sidebar free of axe violations', async () => {
        const { container } = render(Sidebar);
        await expectNoViolations(container);
    });
});
