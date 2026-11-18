import { beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import AppBehavior from '../AppBehavior.svelte';
import { scrcpy } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';

describe('AppBehavior', () => {
    beforeEach(async () => {
        await setLocale('en');
        localStorage.clear();
        scrcpy.backgroundMode = false;
    });

    it('persists the background preference', async () => {
        render(AppBehavior);

        const toggle = screen.getByRole('switch', {
            name: 'Keep running in the background'
        });
        expect(toggle).toHaveAttribute('aria-checked', 'false');

        await fireEvent.click(toggle);

        expect(scrcpy.backgroundMode).toBe(true);
        expect(localStorage.getItem('scrcpy_background_mode')).toBe('true');
        expect(toggle).toHaveAttribute('aria-checked', 'true');
    });
});
