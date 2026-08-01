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
        <div id="error-modal">
            <span id="error-message"></span>
            <details id="error-technical"><pre id="error-technical-message"></pre></details>
            <button id="error-modal-ok"></button>
        </div>
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
        sessionStorage.clear();
        listen.mockImplementation(async (eventName, handler) => {
            handlers.set(eventName, handler);
            return () => handlers.delete(eventName);
        });
        invoke.mockImplementation(async (command) => {
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') {
                return { processInProgress: false, verificationInProgress: false, operation: 'idle' };
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
            expect(invoke).toHaveBeenCalledWith('verify_files', expect.objectContaining({
                checkAllFiles: true,
                operationId: expect.any(String)
            }));
        });
    });

    it('passes checkAllFiles false for the pre-launch verification', async () => {
        await loadRenderer();

        document.getElementById('play-button').click();

        await vi.waitFor(() => {
            expect(invoke).toHaveBeenCalledWith('verify_files', expect.objectContaining({
                checkAllFiles: false,
                operationId: expect.any(String)
            }));
        });
    });

    it('ensures prerequisites after verification and before launch settings', async () => {
        const calls = [];
        invoke.mockImplementation(async (command) => {
            calls.push(command);
            if (command === 'get_language') return 'en';
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return { active: false };
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'ensure_game_prerequisites') {
                return { ready: true, installed: [], alreadyPresent: [], restartRecommended: false };
            }
            return null;
        });
        await loadRenderer();

        document.getElementById('play-button').click();
        await vi.waitFor(() => expect(calls).toContain('launch_game'));

        expect(calls.indexOf('verify_files')).toBeLessThan(calls.indexOf('ensure_game_prerequisites'));
        expect(calls.indexOf('ensure_game_prerequisites')).toBeLessThan(calls.indexOf('update_rev_ini'));
        expect(calls.indexOf('update_rev_ini')).toBeLessThan(calls.indexOf('launch_game'));
    });

    it('blocks Play and manual verification while prerequisites are running', async () => {
        let finishPrerequisites;
        const pendingPrerequisites = new Promise((resolve) => { finishPrerequisites = resolve; });
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return { active: false };
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'ensure_game_prerequisites') return pendingPrerequisites;
            return null;
        });
        await loadRenderer();

        document.getElementById('play-button').click();
        await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(
            'ensure_game_prerequisites', expect.objectContaining({ operationId: expect.any(String) })
        ));

        expect(document.getElementById('play-button').disabled).toBe(true);
        expect(document.getElementById('check-files').disabled).toBe(true);
        const verifyCallCount = invoke.mock.calls.filter(([command]) => command === 'verify_files').length;
        document.getElementById('check-files').click();
        expect(invoke.mock.calls.filter(([command]) => command === 'verify_files')).toHaveLength(verifyCallCount);

        finishPrerequisites({ ready: true, installed: [], alreadyPresent: [], restartRecommended: false });
    });

    it('restores an in-flight prerequisite snapshot on reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'installing-prerequisites' };
            if (command === 'get_prerequisite_state') {
                return {
                    active: true, operationId: 'restored', stage: 'installing',
                    componentId: 'directx-june-2010', progress: 20,
                    downloadedBytes: null, totalBytes: null, restartRecommended: false
                };
            }
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            return null;
        });

        await loadRenderer();

        expect(invoke).toHaveBeenCalledWith('get_prerequisite_state');
        expect(document.getElementById('launcher-status').textContent)
            .toBe('installing prerequisite component...');
        expect(document.getElementById('play-button').disabled).toBe(true);
        expect(document.getElementById('check-files').disabled).toBe(true);
    });

    it('shows the exact prerequisite error carried from the install page', async () => {
        sessionStorage.setItem('pending-prerequisite-error', JSON.stringify({
            operationId: 'handoff',
            error: {
                code: 'prerequisite_install_failed',
                message: 'Prerequisite install failed: installer exited with code 1603'
            }
        }));
        await loadRenderer();

        expect(document.getElementById('error-technical-message').textContent)
            .toContain('installer exited with code 1603');
        expect(sessionStorage.getItem('pending-prerequisite-error')).toBeNull();
        expect(invoke).toHaveBeenCalledWith(
            'acknowledge_prerequisite_state', { operationId: 'handoff' }
        );
    });

    it('preserves the localized prerequisite restart message after game-starting', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return { outcome: 'none', active: false };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            if (command === 'ensure_game_prerequisites') {
                return { ready: true, installed: [], alreadyPresent: [], restartRecommended: false };
            }
            if (command === 'launch_game') {
                handlers.get('game-starting')({ payload: null });
                throw {
                    code: 'prerequisite_restart_required',
                    message: 'Prerequisite restart required: missing runtime DLL'
                };
            }
            return null;
        });
        await loadRenderer();

        document.getElementById('play-button').click();

        await vi.waitFor(() => {
            expect(document.getElementById('error-message').textContent)
                .toBe('Restart Windows to finish installing the required components.');
            expect(document.getElementById('launcher-status').textContent)
                .toBe('failed to install a required component');
        });
    });

    it('restores and shows an exact failed prerequisite terminal on main reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'failed', outcome: 'failed',
                error: {
                    code: 'prerequisite_install_failed',
                    message: 'Prerequisite install failed: exit 1603', details: null
                }
            };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            return null;
        });

        await loadRenderer();

        expect(document.getElementById('error-technical-message').textContent)
            .toContain('exit 1603');
        expect(document.getElementById('launcher-status').textContent)
            .toBe('failed to install a required component');
    });

    it('restores a canceled prerequisite terminal on main reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'cancel', outcome: 'canceled',
                error: { code: 'canceled', message: 'Operation canceled', details: null }
            };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            return null;
        });

        await loadRenderer();

        expect(document.getElementById('launcher-status').textContent)
            .toBe('game launch canceled');
    });

    it('restores a successful prerequisite terminal on main reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'success', outcome: 'succeeded',
                result: { ready: true, installed: [], alreadyPresent: [], restartRecommended: false }
            };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            return null;
        });

        await loadRenderer();

        expect(document.getElementById('launcher-status').textContent).toBe('ready to launch!');
    });

    it('re-delivers a terminal after reload between peek and acknowledgement', async () => {
        const terminal = {
            active: false, operationId: 'race', outcome: 'failed',
            error: {
                code: 'prerequisite_install_failed',
                message: 'Prerequisite install failed: durable race detail', details: null
            }
        };
        let acknowledgeCalls = 0;
        let terminalPresent = true;
        invoke.mockImplementation(async (command, args) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') {
                return terminalPresent ? terminal : { active: false, outcome: 'none' };
            }
            if (command === 'acknowledge_prerequisite_state') {
                expect(args).toEqual({ operationId: 'race' });
                acknowledgeCalls += 1;
                if (acknowledgeCalls === 1) return new Promise(() => {});
                terminalPresent = false;
                return true;
            }
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\Game' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            return null;
        });

        await loadRenderer();
        await vi.waitFor(() => expect(acknowledgeCalls).toBe(1));
        expect(document.getElementById('error-technical-message').textContent)
            .toContain('durable race detail');

        renderMainPage();
        await loadRenderer();
        await vi.waitFor(() => expect(acknowledgeCalls).toBe(2));
        expect(document.getElementById('error-technical-message').textContent)
            .toContain('durable race detail');
    });

    it('release_1_6_12_replaces_searching_updates_with_a_terminal_network_error', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_game_data') {
                return { nickname: '', clanTag: '', launchParams: '', gamePath: 'C:\\Game' };
            }
            if (command === 'get_current_state') {
                return { processInProgress: false, verificationInProgress: false, operation: 'idle' };
            }
            if (command === 'get_game_process_state') return { kind: 'stopped', pid: null };
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'update_game') {
                throw {
                    code: 'network',
                    message: 'error sending request for url (https://api.zhekarik.africa/launcher/game/v2/manifest)'
                };
            }
            return null;
        });
        await loadRenderer();

        document.getElementById('play-button').click();

        await vi.waitFor(() => {
            expect(document.getElementById('launcher-status').textContent)
                .toBe('failed to check game updates');
            expect(document.getElementById('progress-bar').style.width).toBe('0%');
            expect(document.getElementById('error-technical-message').textContent)
                .toContain('https://api.zhekarik.africa/launcher/game/v2/manifest');
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
