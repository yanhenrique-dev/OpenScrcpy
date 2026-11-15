import { beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import LogPanel from '../LogPanel.svelte';
import { scrcpy } from '../../lib/scrcpy.svelte';
import { setLocale } from '../../i18n';

describe('LogPanel', () => {
    beforeEach(async () => {
        await setLocale('en');
        scrcpy.logs = [];
    });

    it('exposes the console as a live log region', () => {
        render(LogPanel);

        expect(screen.getByRole('log')).toHaveAttribute('aria-live', 'polite');
        expect(screen.getByRole('log')).toHaveAccessibleName('System Console');
    });

    it('toggles the expanded console state', async () => {
        render(LogPanel);

        const expand = screen.getByRole('button', { name: 'Expand console' });
        expect(expand).toHaveAttribute('aria-expanded', 'false');

        await fireEvent.click(expand);

        expect(screen.getByRole('button', { name: 'Collapse console' })).toHaveAttribute(
            'aria-expanded',
            'true'
        );
    });
});
