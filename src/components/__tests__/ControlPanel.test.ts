import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import ControlPanel from '../ControlPanel.svelte';
import { DEFAULT_CONFIG } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';
import type { ScrcpyConfig } from '../../lib/types';

function props(config: ScrcpyConfig = { ...DEFAULT_CONFIG }) {
    return {
        config,
        setConfig: vi.fn(),
        onStart: vi.fn(),
        onStop: vi.fn(),
        isRunning: false,
        onListOptions: vi.fn()
    };
}

describe('ControlPanel', () => {
    beforeEach(async () => {
        await setLocale('en');
    });

    it('renders the mirror panel by default', () => {
        render(ControlPanel, { props: props() });

        expect(screen.getByText('Input enhancements')).toBeInTheDocument();
        expect(screen.queryByText('Webcam pro tip')).not.toBeInTheDocument();
    });

    it('renders the camera panel for camera sessions', () => {
        render(ControlPanel, {
            props: props({ ...DEFAULT_CONFIG, sessionMode: 'camera' })
        });

        expect(screen.getByText('Webcam pro tip')).toBeInTheDocument();
        expect(screen.queryByText('Input enhancements')).not.toBeInTheDocument();
    });

    it('renders the mic panel for mic sessions', () => {
        render(ControlPanel, {
            props: props({ ...DEFAULT_CONFIG, sessionMode: 'mic' })
        });

        expect(screen.getByText('Microphone Bridge')).toBeInTheDocument();
    });

    it('switches the capture source from the tabs', async () => {
        const p = props();
        render(ControlPanel, { props: p });

        await fireEvent.click(screen.getByRole('tab', { name: 'Camera' }));

        expect(p.setConfig).toHaveBeenCalledWith(
            expect.objectContaining({ sessionMode: 'camera' })
        );
    });

    it('shows the audio health badge only for a running session', () => {
        render(ControlPanel, {
            props: { ...props(), isRunning: true, audioStatus: 'warning' }
        });

        const badge = screen.getByText('Audio unavailable');
        expect(badge).toBeInTheDocument();
        expect(badge).toHaveAttribute('data-audio', 'warning');
    });

    it('hides the audio badge when the session is stopped', () => {
        render(ControlPanel, { props: { ...props(), audioStatus: 'error' } });

        expect(screen.queryByText('Audio failed')).not.toBeInTheDocument();
    });
});
