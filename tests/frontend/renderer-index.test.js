// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke, listen } = vi.hoisted(() => ({
    invoke: vi.fn(),
    listen: vi.fn().mockResolvedValue(() => {})
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
        invoke.mockImplementation(async (command) => {
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') {
                return { processInProgress: false, verificationInProgress: false };
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
});
