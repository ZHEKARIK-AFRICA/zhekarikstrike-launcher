// @vitest-environment jsdom

import { describe, expect, it, vi } from 'vitest';

import { createPageListener } from '../../src/renderer/event-listener.js';

describe('createPageListener', () => {
    it('unsubscribes even when pagehide happens before listen resolves', async () => {
        let resolveListen;
        const unlisten = vi.fn();
        const listen = vi.fn(() => new Promise((resolve) => {
            resolveListen = resolve;
        }));
        const register = createPageListener({ listenFn: listen, target: window });

        const registered = register('progress', vi.fn());
        window.dispatchEvent(new Event('pagehide'));
        resolveListen(unlisten);
        await registered;

        expect(unlisten).toHaveBeenCalledOnce();
    });
});
