// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invoke, listen, handlers, navigateToPage } = vi.hoisted(() => ({
    invoke: vi.fn(),
    listen: vi.fn(),
    handlers: new Map(),
    navigateToPage: vi.fn()
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen }));
vi.mock('../../src/renderer/navigation.js', () => ({ navigateToPage }));

function renderInstallPage() {
    document.body.innerHTML = `
        <main class="launcher-container">
            <button id="start-install"></button>
            <button id="cancel-install"></button>
            <button id="choose-folder"></button>
            <input id="install-path" value="C:\\Game">
            <div id="progress-bar"></div>
            <div id="install-status"></div>
            <div id="progress-info"></div>
            <div id="error-modal"><span id="error-message"></span>
                <details id="error-technical"><pre id="error-technical-message"></pre></details>
                <button id="error-modal-ok"></button>
            </div>
        </main>
    `;
}

async function loadRenderer() {
    vi.resetModules();
    await import('../../src/renderer/renderer_install.js');
    document.dispatchEvent(new Event('DOMContentLoaded'));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('get_current_state'));
}

describe('install prerequisite flow', () => {
    beforeEach(() => {
        renderInstallPage();
        invoke.mockReset();
        listen.mockReset();
        handlers.clear();
        navigateToPage.mockReset();
        sessionStorage.clear();
        listen.mockImplementation(async (event, handler) => {
            handlers.set(event, handler);
            return () => handlers.delete(event);
        });
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            return null;
        });
    });

    it('shows exact prerequisite failure and routes installed content to main', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'install_game') {
                throw {
                    code: 'prerequisite_install_failed',
                    message: 'Prerequisite install failed: installer exited with code 1603'
                };
            }
            return null;
        });
        await loadRenderer();

        document.getElementById('start-install').click();

        await vi.waitFor(() => {
            expect(document.getElementById('error-technical-message').textContent)
                .toContain('installer exited with code 1603');
            expect(navigateToPage).toHaveBeenCalledWith('./public/index.html');
            expect(JSON.parse(sessionStorage.getItem('pending-prerequisite-error')))
                .toMatchObject({
                    operationId: expect.any(String),
                    error: {
                        code: 'prerequisite_install_failed',
                        message: expect.stringContaining('code 1603')
                    }
                });
        });
    });

    it('renders prerequisite progress emitted by the automatic install command', async () => {
        let finishInstall;
        const pendingInstall = new Promise((resolve) => { finishInstall = resolve; });
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            if (command === 'install_game') return pendingInstall;
            return null;
        });
        await loadRenderer();
        await vi.waitFor(() => expect(handlers.has('prerequisite-progress')).toBe(true));
        document.getElementById('start-install').click();
        await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(
            'install_game', expect.objectContaining({ gamePath: 'C:\\Game' })
        ));
        const operationId = invoke.mock.calls.find(([command]) => command === 'install_game')[1]
            .operationId;

        handlers.get('prerequisite-progress')({
            payload: {
                operationId, stage: 'downloading', componentId: 'vc2010-sp1-x86',
                progress: 45, downloadedBytes: 450, totalBytes: 1000,
                restartRecommended: false
            }
        });

        expect(document.getElementById('install-status').textContent)
            .toBe('downloading prerequisite component...');
        expect(document.getElementById('progress-bar').style.width).toBe('45%');
        finishInstall();
    });

    it('restores the exact prerequisite state after install page reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'installing-prerequisites' };
            if (command === 'get_prerequisite_state') {
                return {
                    active: true, operationId: 'op', stage: 'verifying',
                    componentId: 'vc2010-sp1-x86', progress: 70,
                    downloadedBytes: null, totalBytes: null, restartRecommended: false
                };
            }
            if (command === 'get_game_path') return 'C:\\Game';
            return null;
        });

        await loadRenderer();

        expect(invoke).toHaveBeenCalledWith('get_prerequisite_state');
        expect(document.getElementById('install-status').textContent)
            .toBe('verifying prerequisite component...');
        expect(document.getElementById('progress-bar').style.width).toBe('70%');
    });

    it('routes an exact failed terminal from an install reload to main', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'failed', outcome: 'failed',
                error: {
                    code: 'prerequisite_verification_failed',
                    message: 'Prerequisite verification failed: signer mismatch', details: null
                }
            };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            return null;
        });

        await loadRenderer();

        expect(navigateToPage).toHaveBeenCalledWith('./public/index.html');
        expect(JSON.parse(sessionStorage.getItem('pending-prerequisite-error')))
            .toMatchObject({
                operationId: 'failed',
                error: { code: 'prerequisite_verification_failed' }
            });
    });

    it('restores a canceled prerequisite terminal on install reload', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'cancel', outcome: 'canceled',
                error: { code: 'canceled', message: 'Operation canceled', details: null }
            };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            return null;
        });

        await loadRenderer();

        expect(document.getElementById('install-status').textContent)
            .toBe('installation canceled');
        expect(navigateToPage).not.toHaveBeenCalled();
    });

    it('routes a successful prerequisite terminal from install reload to main', async () => {
        invoke.mockImplementation(async (command) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'success', outcome: 'succeeded',
                result: { ready: true, installed: [], alreadyPresent: [], restartRecommended: false }
            };
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            return null;
        });

        await loadRenderer();

        expect(navigateToPage).toHaveBeenCalledWith('./public/index.html');
    });

    it('persists a failed handoff before ack and resumes routing after a reload', async () => {
        let acknowledgeCalls = 0;
        invoke.mockImplementation(async (command, args) => {
            if (command === 'get_language') return 'en';
            if (command === 'get_current_state') return { operation: 'idle' };
            if (command === 'get_prerequisite_state') return {
                active: false, operationId: 'race', outcome: 'failed',
                error: {
                    code: 'prerequisite_verification_failed',
                    message: 'Prerequisite verification failed: durable race detail', details: null
                }
            };
            if (command === 'acknowledge_prerequisite_state') {
                expect(args).toEqual({ operationId: 'race' });
                acknowledgeCalls += 1;
                return new Promise(() => {});
            }
            if (command === 'get_game_path') return 'C:\\Game';
            if (command === 'recover_pending_install') return { recovered: false };
            return null;
        });

        await loadRenderer();
        await vi.waitFor(() => expect(acknowledgeCalls).toBe(1));
        expect(JSON.parse(sessionStorage.getItem('pending-prerequisite-error')))
            .toMatchObject({
                operationId: 'race',
                error: { code: 'prerequisite_verification_failed' }
            });
        expect(navigateToPage).not.toHaveBeenCalled();

        renderInstallPage();
        await loadRenderer();
        await vi.waitFor(() => {
            expect(navigateToPage).toHaveBeenCalledWith('./public/index.html');
        });
    });
});
