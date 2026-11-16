import { beforeEach, describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
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
});
