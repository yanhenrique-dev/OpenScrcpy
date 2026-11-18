import { beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import SessionBehavior from '../SessionBehavior.svelte';
import { scrcpy, DEFAULT_CONFIG } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';

describe('SessionBehavior', () => {
    beforeEach(async () => {
        await setLocale('en');
        scrcpy.config = { ...DEFAULT_CONFIG };
    });

    it('offers screen-off for mirror sessions', () => {
        render(SessionBehavior);

        expect(screen.getByRole('switch', { name: 'Screen off' })).toBeInTheDocument();
    });

    it('hides screen-off for camera sessions, where it is ignored', () => {
        scrcpy.config = { ...DEFAULT_CONFIG, sessionMode: 'camera' };
        render(SessionBehavior);

        expect(screen.queryByRole('switch', { name: 'Screen off' })).not.toBeInTheDocument();
    });

    it('hides the whole behavior card for mic sessions', () => {
        scrcpy.config = { ...DEFAULT_CONFIG, sessionMode: 'mic' };
        render(SessionBehavior);

        expect(screen.queryByText('Session behavior')).not.toBeInTheDocument();
    });

    it('offers the require-audio switch while audio is forwarded', async () => {
        render(SessionBehavior);

        const toggle = screen.getByRole('switch', { name: 'Require audio' });
        await fireEvent.click(toggle);
        expect(scrcpy.config.requireAudio).toBe(true);
    });

    it('hides the require-audio switch when audio forwarding is off', () => {
        scrcpy.config = { ...DEFAULT_CONFIG, audioEnabled: false };
        render(SessionBehavior);

        expect(screen.queryByRole('switch', { name: 'Require audio' })).not.toBeInTheDocument();
    });

    it('offers the mirror audio source and only shows device duplication for playback', async () => {
        render(SessionBehavior);

        expect(
            screen.getByRole('combobox', { name: 'Audio source' })
        ).toBeInTheDocument();
        expect(
            screen.queryByRole('switch', { name: 'Keep playing on device' })
        ).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('combobox', { name: 'Audio source' }));
        await fireEvent.click(screen.getByRole('option', { name: 'Media playback' }));

        expect(scrcpy.config.mirrorAudioSource).toBe('playback');
        expect(
            screen.getByRole('switch', { name: 'Keep playing on device' })
        ).toBeInTheDocument();
    });

    it('hides the mirror audio source outside mirror mode', () => {
        scrcpy.config = { ...DEFAULT_CONFIG, sessionMode: 'camera' };
        render(SessionBehavior);

        expect(
            screen.queryByRole('combobox', { name: 'Audio source' })
        ).not.toBeInTheDocument();
    });

    it('clears device duplication when leaving playback', async () => {
        scrcpy.config = {
            ...DEFAULT_CONFIG,
            mirrorAudioSource: 'playback',
            audioDup: true
        };
        render(SessionBehavior);

        await fireEvent.click(screen.getByRole('combobox', { name: 'Audio source' }));
        await fireEvent.click(screen.getByRole('option', { name: 'Phone microphone' }));

        expect(scrcpy.config.mirrorAudioSource).toBe('mic');
        expect(scrcpy.config.audioDup).toBe(false);
    });
});
