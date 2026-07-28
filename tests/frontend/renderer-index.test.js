// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke, listen, handlers } = vi.hoisted(() => ({
    invoke: vi.fn(),
    listen: vi.fn(),
    handlers: new Map()
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));

function renderMainPage() {
    document.body.innerHTML = `
        <button id="play-button"></button>
        <button id="check-files"></button>
        <input id="game-path">
        <input id="launch-params" value="-novid">
        <input id="clan-tag" value="z">
        <input id="nickname" value="player">
        <div id="progress-bar"></div>
        <div id="launcher-status"></div>
        <div id="progress-info"></div>
        <div id="error-modal"><span id="error-message"></span><button id="error-modal-ok"></button></div>
    `;
}

async function loadRenderer() {
    vi.resetModules();
    await import('../../src/renderer/renderer_index.js');
    document.dispatchEvent(new Event('DOMContentLoaded'));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('get_current_state'));
}

describe('main renderer Tauri command contracts', () => {
    beforeEach(() => {
        renderMainPage();
        invoke.mockReset();
        listen.mockClear();
        handlers.clear();
        listen.mockImplementation(async (eventName, handler) => {
            handlers.set(eventName, handler);
            return () => handlers.delete(eventName);
        });
        invoke.mockImplementation(async (command) => {
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') {
                return { processInProgress: false, verificationInProgress: false };
            }
            if (command === 'get_game_process_state') {
                return { kind: 'stopped', pid: null };
            }
            return null;
        });
    });

    it('passes checkAllFiles true for a manual verification', async () => {
        await loadRenderer();

        document.getElementById('check-files').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledWith('verify_files', { checkAllFiles: true });
        });
    });

    it('passes checkAllFiles false for the pre-launch verification', async () => {
        await loadRenderer();

        document.getElementById('play-button').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledWith('verify_files', { checkAllFiles: false });
        });
    });

    it('release_1_6_11_blocks_play_and_verify_until_the_running_game_closes', async () => {
        await loadRenderer();
        await vi.waitFor(() => expect(handlers.has('game-started')).toBe(true));

        handlers.get('game-started')({ payload: { pid: 42 } });
        document.getElementById('play-button').click();
        document.getElementById('check-files').click();

        expect(document.getElementById('play-button').disabled).toBe(true);
        expect(document.getElementById('check-files').disabled).toBe(true);
        expect(invoke).not.toHaveBeenCalledWith('launch_game');
        expect(invoke).not.toHaveBeenCalledWith('verify_files', { checkAllFiles: true });

        handlers.get('game-closed')({ payload: null });
        expect(document.getElementById('play-button').disabled).toBe(false);
        expect(document.getElementById('check-files').disabled).toBe(false);
    });

    it('release_1_6_11_detects_a_game_that_was_running_before_the_page_loaded', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') {
                return { processInProgress: false, verificationInProgress: false };
            }
            if (command === 'get_game_process_state') {
                return { kind: 'running', pid: 42 };
            }
            return null;
        });

        await loadRenderer();

        expect(invoke).toHaveBeenCalledWith('get_game_process_state');
        expect(document.getElementById('play-button').disabled).toBe(true);
        expect(document.getElementById('check-files').disabled).toBe(true);
    });
});
