import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { handleError, setupErrorModal } from './common.js';
import { waitForE2eReady } from './e2e.js';
import { listenUntilPageHide } from './event-listener.js';
import { navigateToPage } from './navigation.js';
import { createOperationId, createStatusController } from './status-controller.js';

setupErrorModal();

const startInstallButton = document.getElementById('start-install');
const cancelInstallButton = document.getElementById('cancel-install');
const chooseFolderButton = document.getElementById('choose-folder');
const installPathInput = document.getElementById('install-path');
let initialized = false;
let startupFailed = false;
let installing = false;
let cancelPending = false;
let statePoll = null;

function renderActions(statusState) {
    const busy = statusState.kind === 'busy' || statusState.kind === 'initializing';
    if (startInstallButton) {
        startInstallButton.textContent = t('start_install');
        startInstallButton.disabled = !initialized || startupFailed || busy;
    }
    if (cancelInstallButton) {
        cancelInstallButton.textContent = t('cancel_install');
        cancelInstallButton.disabled = !installing || cancelPending;
    }
    if (chooseFolderButton) chooseFolderButton.disabled = !initialized || startupFailed || busy;
    if (installPathInput) installPathInput.disabled = !initialized || startupFailed || busy;
}

const status = createStatusController({
    root: document.querySelector('.launcher-container') || document.body,
    statusElement: document.getElementById('install-status'),
    progressBar: document.getElementById('progress-bar'),
    progressInfo: document.getElementById('progress-info'),
    renderActions
});

function stopStatePolling() {
    if (statePoll != null) window.clearInterval(statePoll);
    statePoll = null;
}

async function acknowledgePrerequisite(operationId) {
    if (!operationId) return false;
    try {
        return await invoke('acknowledge_prerequisite_state', { operationId });
    } catch (error) {
        console.error('Failed to acknowledge prerequisite state:', error);
        return false;
    }
}

async function handlePrerequisiteTerminal(prerequisite) {
    if (prerequisite?.outcome === 'failed') {
        try {
            sessionStorage.setItem('pending-prerequisite-error', JSON.stringify({
                operationId: prerequisite.operationId,
                error: prerequisite.error
            }));
        } catch (error) {
            console.error('Failed to carry prerequisite error to the main page:', error);
            status.fail('status.prerequisite_failed');
            handleError(null, prerequisite.error);
            return true;
        }
        await acknowledgePrerequisite(prerequisite.operationId);
        await navigateToPage('./public/index.html');
        return true;
    }
    if (prerequisite?.outcome === 'canceled') {
        installing = false;
        status.cancel('status.installation_canceled');
        await acknowledgePrerequisite(prerequisite.operationId);
        return true;
    }
    if (prerequisite?.outcome === 'succeeded') {
        installing = false;
        status.succeed('status.installation_complete');
        await acknowledgePrerequisite(prerequisite.operationId);
        await navigateToPage('./public/index.html');
        return true;
    }
    return false;
}

async function recoverAndRoute() {
    const operationId = createOperationId();
    status.begin({
        flow: 'recovery', step: 'recovery', statusKey: 'status.recovering_install', operationId
    });
    const recovery = await invoke('recover_pending_install', { operationId });
    if (!recovery?.recovered) return false;
    status.succeed('status.recovery_complete');
    const existence = await invoke('check_game_exists');
    if (!existence?.exists) return false;
    await navigateToPage('./public/index.html');
    return true;
}

function restoreOperation(operation) {
    if (!status.restoreOperation(operation)) return false;
    installing = operation === 'installing' || operation === 'installing-prerequisites';
    status.rerender();
    statePoll = window.setInterval(async () => {
        try {
            const current = await invoke('get_current_state');
            if (current.operation && current.operation !== 'idle') return;
            stopStatePolling();
            installing = false;
            const prerequisite = await invoke('get_prerequisite_state');
            if (await handlePrerequisiteTerminal(prerequisite)) return;
            if (await recoverAndRoute()) return;
            const existence = await invoke('check_game_exists');
            if (existence?.exists) {
                await navigateToPage('./public/index.html');
                return;
            }
            status.setIdle('status.install_idle');
        } catch (error) {
            stopStatePolling();
            startupFailed = true;
            status.fail('status.recovery_failed');
            handleError(null, error, { contextKey: 'status.recovery_failed' });
        }
    }, 1000);
    return true;
}

async function restoreCurrentOperation(operation, prerequisite) {
    if (operation === 'installing-prerequisites') {
        if (status.restorePrerequisite(prerequisite)) {
            installing = true;
            status.rerender();
            statePoll = window.setInterval(async () => {
                try {
                    const current = await invoke('get_current_state');
                    if (current.operation && current.operation !== 'idle') return;
                    stopStatePolling();
                    installing = false;
                    const terminal = await invoke('get_prerequisite_state');
                    if (!await handlePrerequisiteTerminal(terminal)) {
                        await navigateToPage('./public/index.html');
                    }
                } catch (error) {
                    stopStatePolling();
                    startupFailed = true;
                    status.fail('status.prerequisite_failed');
                    handleError(null, error);
                }
            }, 1000);
            return true;
        }
    }
    return restoreOperation(operation);
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        status.rerender();
        const current = await invoke('get_current_state');
        if (sessionStorage.getItem('pending-prerequisite-error')) {
            await navigateToPage('./public/index.html');
            return;
        }
        const prerequisite = await invoke('get_prerequisite_state');
        const terminal = await handlePrerequisiteTerminal(prerequisite);
        const restored = terminal || await restoreCurrentOperation(current.operation, prerequisite);
        if (!restored && await recoverAndRoute()) return;

        const savedPath = await invoke('get_game_path');
        if (savedPath && installPathInput && !installPathInput.value) {
            installPathInput.value = savedPath;
        }
        initialized = true;
        if (!restored) status.setIdle('status.install_idle');
        document.body.classList.add('fade-in');
    } catch (error) {
        startupFailed = true;
        status.fail(status.getState().flow === 'recovery'
            ? 'status.recovery_failed'
            : 'status.loading_failed');
        handleError(null, error, { contextKey: status.getState().statusKey });
    }
}, { once: true });

startInstallButton?.addEventListener('click', async () => {
    const gamePath = installPathInput?.value?.trim();
    if (!gamePath) {
        handleError(null, t('errors.install_path_not_set'));
        return;
    }
    if (!initialized || startupFailed || installing) return;

    installing = true;
    const operationId = createOperationId();
    status.begin({
        flow: 'install', step: 'install', statusKey: 'status.installing', operationId
    });
    try {
        await invoke('install_game', { gamePath, operationId });
        status.succeed('status.installation_complete');
        await acknowledgePrerequisite(operationId);
        document.body.classList.remove('fade-in');
        document.body.classList.add('fade-out');
        await navigateToPage('./public/index.html');
    } catch (error) {
        if (error?.code === 'canceled') {
            status.cancel('status.installation_canceled');
            await acknowledgePrerequisite(operationId);
        } else if (String(error?.code || '').startsWith('prerequisite_')) {
            status.fail('status.prerequisite_failed');
            handleError(null, error);
            let carriedError = error;
            let carriedOperationId = operationId;
            try {
                const terminal = await invoke('get_prerequisite_state');
                carriedError = terminal?.error || error;
                carriedOperationId = terminal?.operationId || operationId;
            } catch { /* preserve original */ }
            try {
                sessionStorage.setItem('pending-prerequisite-error', JSON.stringify({
                    operationId: carriedOperationId,
                    error: carriedError
                }));
            } catch (storageError) {
                console.error('Failed to carry prerequisite error to the main page:', storageError);
                return;
            }
            await acknowledgePrerequisite(carriedOperationId);
            await navigateToPage('./public/index.html');
        } else {
            status.fail('status.installation_failed');
            handleError(null, error, { contextKey: 'status.installation_failed' });
        }
    } finally {
        installing = false;
        cancelPending = false;
        status.rerender();
    }
});

cancelInstallButton?.addEventListener('click', async () => {
    if (!installing || cancelPending) return;
    cancelPending = true;
    status.updateBusy({ step: 'cancel-install', statusKey: 'status.canceling_install' });
    try {
        const requested = await invoke('cancel_install');
        if (!requested) {
            cancelPending = false;
            status.updateBusy({ step: 'install', statusKey: 'status.installing' });
        }
    } catch (error) {
        cancelPending = false;
        status.updateBusy({ step: 'install', statusKey: 'status.installing' });
        handleError(null, error);
    }
});

chooseFolderButton?.addEventListener('click', async () => {
    if (!initialized || startupFailed || installing) return;
    try {
        const folderPath = await invoke('select_game_folder');
        if (folderPath && installPathInput) installPathInput.value = `${folderPath}\\ZHEKARIKSTRIKE`;
    } catch (error) {
        handleError(null, error);
    }
});

void listenUntilPageHide('install-progress', ({ payload }) => {
    const progressStatus = payload.message === 'resume'
        ? 'status.resuming_install'
        : payload.stage === 'checking'
        ? 'status.verifying_files'
        : 'status.installing';
    status.applyProgress(payload, progressStatus);
});
void listenUntilPageHide('recovery-progress', ({ payload }) => {
    status.applyProgress(payload, 'status.recovering_install');
});
void listenUntilPageHide('prerequisite-progress', ({ payload }) => {
    const statusKey = {
        detecting: 'status.prerequisite_detecting',
        downloading: 'status.prerequisite_downloading',
        verifying: 'status.prerequisite_verifying',
        installing: 'status.prerequisite_installing',
        complete: 'status.prerequisite_verifying'
    }[payload.stage] || 'status.prerequisite_detecting';
    status.applyProgress(payload, statusKey);
});

window.addEventListener('pagehide', () => {
    stopStatePolling();
    status.dispose();
}, { once: true });
