// @vitest-environment jsdom

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { describe, expect, it, vi } from 'vitest';

const { invoke, windowApi, getCurrentWindow } = vi.hoisted(() => {
    const windowApi = {
        close: vi.fn(() => ({
            then(_resolve, reject) {
                reject(new Error('close denied'));
            }
        })),
        minimize: vi.fn().mockResolvedValue()
    };
    return {
        invoke: vi.fn(),
        windowApi,
        getCurrentWindow: vi.fn(() => windowApi)
    };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow }));

describe('shared renderer window controls', () => {
    it('invokes close and minimize and reports a rejected window action', async () => {
        document.body.innerHTML = `
            <button id="close-window"></button>
            <button id="minimize-window"></button>
        `;
        const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});

        await import('../../src/renderer/renderer.js');
        document.dispatchEvent(new Event('DOMContentLoaded'));

        document.getElementById('close-window').click();
        document.getElementById('minimize-window').click();

        await vi.waitFor(() => {
            expect(windowApi.close).toHaveBeenCalledOnce();
            expect(windowApi.minimize).toHaveBeenCalledOnce();
            expect(consoleError).toHaveBeenCalledWith(
                'Failed to close window:',
                expect.objectContaining({ message: 'close denied' })
            );
        });

        consoleError.mockRestore();
    });

    it('grants the runtime permissions needed by both controls', () => {
        const capabilityPath = join(process.cwd(), 'src-tauri', 'capabilities', 'default.json');
        const capability = JSON.parse(readFileSync(capabilityPath, 'utf8'));

        expect(capability.permissions).toEqual(expect.arrayContaining([
            'core:window:allow-close',
            'core:window:allow-minimize'
        ]));
    });
});
