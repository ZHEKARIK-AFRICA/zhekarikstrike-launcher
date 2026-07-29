// @vitest-environment jsdom

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke, listen, handlers, windowApi, getCurrentWindow } = vi.hoisted(() => {
    const windowApi = {
        close: vi.fn(() => ({
            then(_resolve, reject) {
                reject(new Error('close denied'));
            }
        })),
        minimize: vi.fn().mockResolvedValue()
    };
    return {
        invoke: vi.fn(() => Promise.resolve()),
        listen: vi.fn(),
        handlers: new Map(),
        windowApi,
        getCurrentWindow: vi.fn(() => windowApi)
    };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow }));

describe('shared renderer window controls', () => {
    beforeEach(() => {
        vi.resetModules();
        invoke.mockClear();
        listen.mockReset();
        handlers.clear();
        listen.mockImplementation(async (event, handler) => {
            handlers.set(event, handler);
            return () => handlers.delete(event);
        });
        windowApi.minimize.mockClear();
    });

    it('uses the Rust shutdown lifecycle for close and Tauri for minimize', async () => {
        document.body.innerHTML = `
            <button id="close-window"></button>
            <button id="minimize-window"></button>
        `;
        await import('../../src/renderer/renderer.js');
        document.dispatchEvent(new Event('DOMContentLoaded'));

        document.getElementById('close-window').click();
        document.getElementById('minimize-window').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledWith('close_window');
            expect(windowApi.close).not.toHaveBeenCalled();
            expect(windowApi.minimize).toHaveBeenCalledOnce();
        });
    });

    it('release_1_6_12_shows_the_styled_confirmation_and_confirms_game_shutdown_once', async () => {
        document.body.innerHTML = `
            <button id="close-window"></button>
            <div id="close-confirmation-modal" style="display:none">
                <h2 id="close-confirmation-title"></h2>
                <p id="close-confirmation-message"></p>
                <button id="close-confirmation-cancel"></button>
                <button id="close-confirmation-confirm"></button>
            </div>
        `;

        await import('../../src/renderer/renderer.js');
        document.dispatchEvent(new Event('DOMContentLoaded'));
        await vi.waitFor(() => expect(handlers.has('close-confirmation-requested')).toBe(true));

        handlers.get('close-confirmation-requested')({
            payload: { reason: 'game-running', operation: 'idle' }
        });

        expect(document.getElementById('close-confirmation-modal').style.display).toBe('block');
        expect(document.getElementById('close-confirmation-confirm').textContent)
            .toBe('Close game and launcher');

        document.getElementById('close-confirmation-confirm').click();
        document.getElementById('close-confirmation-confirm').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledTimes(1);
            expect(invoke).toHaveBeenCalledWith('confirm_close_window');
        });
    });

    it('release_1_6_12_cancels_a_pending_close_confirmation', async () => {
        document.body.innerHTML = `
            <div id="error-modal" style="display:block"></div>
            <div id="close-confirmation-modal" style="display:none">
                <h2 id="close-confirmation-title"></h2>
                <p id="close-confirmation-message"></p>
                <button id="close-confirmation-cancel"></button>
                <button id="close-confirmation-confirm"></button>
            </div>
        `;

        await import('../../src/renderer/renderer.js');
        document.dispatchEvent(new Event('DOMContentLoaded'));
        await vi.waitFor(() => expect(handlers.has('close-confirmation-requested')).toBe(true));
        handlers.get('close-confirmation-requested')({
            payload: { reason: 'operation-active', operation: 'installing' }
        });
        document.getElementById('close-confirmation-cancel').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledWith('cancel_close_window');
            expect(document.getElementById('close-confirmation-modal').style.display).toBe('none');
            expect(document.getElementById('error-modal').style.display).toBe('block');
        });
    });

    it('grants the runtime permissions needed by both controls', () => {
        const capabilityPath = join(process.cwd(), 'src-tauri', 'capabilities', 'default.json');
        const capability = JSON.parse(readFileSync(capabilityPath, 'utf8'));

        expect(capability.permissions).toContain('core:window:allow-minimize');
        expect(capability.permissions).not.toContain('core:window:allow-close');
    });
});
