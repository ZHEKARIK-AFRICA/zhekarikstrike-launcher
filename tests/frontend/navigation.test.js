import { describe, expect, it, vi } from 'vitest';

import { createNavigator, normalizePage } from '../../src/renderer/navigation.js';

describe('normalizePage', () => {
    it('normalizes a public page for the production layout', () => {
        expect(normalizePage('./public/index.html', '/public/intro.html')).toBe('/public/index.html');
    });
});

describe('createNavigator', () => {
    it('requests the window layout before navigating', async () => {
        const calls = [];
        const invoke = vi.fn(async (command, payload) => {
            calls.push([command, payload]);
        });
        const assign = vi.fn((page) => calls.push(['assign', page]));
        const navigate = createNavigator({ invoke, assign, pathname: '/public/intro.html' });

        await navigate('./public/index.html');

        expect(calls).toEqual([
            ['set_window_layout', { page: './public/index.html' }],
            ['assign', '/public/index.html']
        ]);
    });

    it('still navigates when resizing the window fails', async () => {
        const assign = vi.fn();
        const navigate = createNavigator({
            invoke: vi.fn().mockRejectedValue(new Error('resize failed')),
            assign,
            pathname: '/public/intro.html'
        });

        await navigate('./public/install.html');

        expect(assign).toHaveBeenCalledWith('/public/install.html');
    });
});
