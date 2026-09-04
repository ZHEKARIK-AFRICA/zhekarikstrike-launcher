import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { handleError, setupErrorModal } from './common.js';
import { waitForE2eReady } from './e2e.js';
import { listenUntilPageHide } from './event-listener.js';
import { navigateToPage } from './navigation.js';
import { createOperationId, createStatusController } from './status-controller.js';

setupErrorModal();

const playButton = document.getElementById('play-button');
const checkFilesButton = document.getElementById('check-files');
const root = document.querySelector('.launcher-container') || document.body;
let initialized = false;
let startupFailed = false;
let gameRunning = false;
let manualVerificationActive = false;
let cancelPending = false;
let launchEventSeen = false;
let statePoll = null;

function renderActions(statusState) {
    const busy = statusState.kind === 'busy' || statusState.kind === 'initializing';
    if (playButton) {
        playButton.textContent = t('play');
        playButton.disabled = !initialized || startupFailed || gameRunning || busy;
    }
    if (checkFilesButton) {
        checkFilesButton.textContent = t(manualVerificationActive ? 'cancel' : 'check_files');
        const anotherFlowBusy = busy && !manualVerificationActive;
        checkFilesButton.disabled = !initialized || startupFailed || gameRunning
            || anotherFlowBusy || cancelPending;
    }
}

const status = createStatusController({
    root,
    statusElement: document.getElementById('launcher-status'),
    progressBar: document.getElementById('progress-bar'),
    progressInfo: document.getElementById('progress-info'),
    renderActions
});

function isGameActive(gameState) {
    return gameState?.kind === 'starting' || gameState?.kind === 'running';
}

async function syncIdleState() {
    const gameState = await invoke('get_game_process_state');
    gameRunning = isGameActive(gameState);
    if (gameRunning) status.setRunning();
    else status.setIdle();
}

function stopStatePolling() {
    if (statePoll != null) window.clearInterval(statePoll);
    statePoll = null;
}

function restoreOperation(operation) {
    if (!status.restoreOperation(operation)) return false;
    stopStatePolling();
    statePoll = window.setInterval(async () => {
        try {
            const current = await invoke('get_current_state');
            if (current.operation && current.operation !== 'idle') return;
            stopStatePolling();
            await syncIdleState();
        } catch (error) {
            console.error('Failed to refresh launcher operation state:', error);
        }
    }, 1000);
    return true;
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
        status.fail('status.prerequisite_failed');
        handleError(null, prerequisite.error);
        await acknowledgePrerequisite(prerequisite.operationId);
        return true;
    }
    if (prerequisite?.outcome === 'canceled') {
        status.cancel('status.launch_canceled');
        await acknowledgePrerequisite(prerequisite.operationId);
        return true;
    }
    if (prerequisite?.outcome === 'succeeded') {
        await syncIdleState();
        await acknowledgePrerequisite(prerequisite.operationId);
        return true;
    }
    return false;
}

async function finishRestoredPrerequisite() {
    const prerequisite = await invoke('get_prerequisite_state');
    if (!await handlePrerequisiteTerminal(prerequisite)) await syncIdleState();
}

async function restoreCurrentOperation(current, prerequisite) {
    if (current?.operation === 'installing-prerequisites') {
        if (status.restorePrerequisite(prerequisite)) {
            stopStatePolling();
            statePoll = window.setInterval(async () => {
                try {
                    const latest = await invoke('get_current_state');
                    if (latest.operation && latest.operation !== 'idle') return;
                    stopStatePolling();
                    await finishRestoredPrerequisite();
                } catch (error) {
                    console.error('Failed to refresh prerequisite state:', error);
                }
            }, 1000);
            return true;
        }
    }
    return restoreOperation(current?.operation);
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
        status.rerender();
        let current = await invoke('get_current_state');
        if (!current.operation || current.operation === 'idle') {
            const operationId = createOperationId();
            status.begin({ flow: 'recovery', step: 'recovery', statusKey: 'status.recovering_install', operationId });
            const recovery = await invoke('recover_pending_install', { operationId });
            if (recovery?.recovered) {
                status.succeed('status.recovery_complete');
                const existence = await invoke('check_game_exists');
                if (!existence?.exists) {
                    await navigateToPage('./public/install.html');
                    return;
                }
            }
            current = await invoke('get_current_state');
        }

        const { nickname, clanTag, launchParams, gamePath } = await invoke('get_game_data');
        document.getElementById('launch-params').value = launchParams || '';
        document.getElementById('clan-tag').value = clanTag || '';
        document.getElementById('nickname').value = nickname || '';
        const gamePathInput = document.getElementById('game-path');
        gamePathInput.value = gamePath || t('stageMessages.game_path_not_found');
        gamePathInput.readOnly = true;
        document.dispatchEvent(new CustomEvent('gameDataLoaded', {
            detail: { nickname, clanTag, launchParams, gamePath }
        }));

        initialized = true;
        const prerequisite = await invoke('get_prerequisite_state');
        if (!await handlePrerequisiteTerminal(prerequisite)
            && !await restoreCurrentOperation(current, prerequisite)) await syncIdleState();
        try {
            const pendingPrerequisiteError = sessionStorage.getItem('pending-prerequisite-error');
            if (pendingPrerequisiteError) {
                const handoff = JSON.parse(pendingPrerequisiteError);
                handleError(null, handoff.error || handoff);
                sessionStorage.removeItem('pending-prerequisite-error');
                await acknowledgePrerequisite(handoff.operationId);
            }
        } catch (error) {
            sessionStorage.removeItem('pending-prerequisite-error');
            console.error('Failed to restore prerequisite error:', error);
        }
        document.body.classList.add('fade-in');
    } catch (error) {
        startupFailed = true;
        status.fail(status.getState().flow === 'recovery'
            ? 'status.recovery_failed'
            : 'status.loading_failed');
        handleError(null, error, { contextKey: status.getState().statusKey });
    }
}, { once: true });

playButton?.addEventListener('click', async () => {
    if (!initialized || startupFailed || gameRunning || status.getState().kind === 'busy') return;
    launchEventSeen = false;
    let failureKey = 'status.game_update_failed';
    let operationId = createOperationId();
    status.begin({
        flow: 'play', step: 'game-updates', statusKey: 'status.checking_game_updates', operationId
    });

    try {
        await invoke('update_game', { operationId });
        failureKey = 'status.launch_verification_failed';
        operationId = createOperationId();
        status.beginStep({
            step: 'launch-verification', statusKey: 'status.checking_launch_files', operationId
        });
        await invoke('verify_files', { checkAllFiles: false, operationId });

        failureKey = 'status.prerequisite_failed';
        operationId = createOperationId();
        status.beginStep({
            step: 'detecting', statusKey: 'status.prerequisite_detecting', operationId
        });
        await invoke('ensure_game_prerequisites', { operationId });
        failureKey = 'status.launch_settings_failed';
        status.beginStep({
            step: 'launch-settings', statusKey: 'status.saving_launch_settings'
        });
        await acknowledgePrerequisite(operationId);
        await invoke('update_rev_ini', {
            launchParams: document.getElementById('launch-params').value,
            clanTag: document.getElementById('clan-tag').value,
            nickname: document.getElementById('nickname').value
        });

        failureKey = 'status.game_prepare_failed';
        operationId = createOperationId();
        status.beginStep({ step: 'prepare-game', statusKey: 'status.preparing_game', operationId });
        await invoke('launch_game', { operationId });
    } catch (error) {
        if (error?.code === 'canceled') {
            status.cancel('status.launch_canceled');
        } else if (error?.code === 'game_already_running') {
            gameRunning = true;
            status.setRunning();
        } else {
            const prerequisiteFailure = String(error?.code || '').startsWith('prerequisite_');
            if (prerequisiteFailure) failureKey = 'status.prerequisite_failed';
            else if (launchEventSeen) failureKey = 'status.game_launch_failed';
            status.fail(failureKey);
            if (prerequisiteFailure) handleError(null, error);
            else handleError(null, error, { contextKey: failureKey });
        }
        await acknowledgePrerequisite(operationId);
    }
});

checkFilesButton?.addEventListener('click', async () => {
    if (!initialized || startupFailed || gameRunning) return;
    if (manualVerificationActive) {
        if (cancelPending) return;
        cancelPending = true;
        status.updateBusy({
            step: 'cancel-verification', statusKey: 'status.canceling_verification'
        });
        try {
            const requested = await invoke('cancel_verify');
            if (!requested) {
                cancelPending = false;
                status.updateBusy({ step: 'verify', statusKey: 'status.verifying_files' });
            }
        } catch (error) {
            cancelPending = false;
            status.updateBusy({ step: 'verify', statusKey: 'status.verifying_files' });
            handleError(null, error);
        }
        return;
    }
    if (status.getState().kind === 'busy') return;

    manualVerificationActive = true;
    const operationId = createOperationId();
    status.begin({
        flow: 'manual-verify', step: 'verify', statusKey: 'status.verifying_files', operationId
    });
    try {
        await invoke('verify_files', { checkAllFiles: true, operationId });
        status.succeed('status.files_good');
    } catch (error) {
        if (error?.code === 'canceled') {
            status.cancel('status.verification_canceled');
        } else if (error?.code === 'game_already_running') {
            gameRunning = true;
            status.setRunning();
        } else {
            status.fail('status.verification_failed');
            handleError(null, error, { contextKey: 'status.verification_failed' });
        }
    } finally {
        manualVerificationActive = false;
        cancelPending = false;
        status.rerender();
    }
});

function statusKeyForProgress(payload) {
    const current = status.getState();
    if (payload.stage === 'complete') return current.statusKey;
    if (payload.message === 'resume') return 'status.resuming_install';
    const repairing = ['install', 'download', 'update', 'copy', 'extract'].includes(payload.stage);
    if (current.flow === 'manual-verify') {
        return repairing ? 'status.repairing_files' : 'status.verifying_files';
    }
    if (current.step === 'launch-verification') {
        return repairing ? 'status.repairing_files' : 'status.checking_launch_files';
    }
    if (current.flow === 'recovery' || payload.stage === 'cleanup') {
        return 'status.recovering_install';
    }
    return repairing ? 'status.installing' : 'status.checking_game_updates';
}

void listenUntilPageHide('verify-progress', ({ payload }) => {
    status.applyProgress(payload, statusKeyForProgress(payload));
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
void listenUntilPageHide('game-starting', () => {
    launchEventSeen = true;
    gameRunning = true;
    status.updateBusy({ step: 'launching-game', statusKey: 'status.launching_game' });
});
void listenUntilPageHide('game-started', () => {
    gameRunning = true;
    status.setRunning();
});
void listenUntilPageHide('game-closed', () => {
    gameRunning = false;
    if (!['error', 'canceled'].includes(status.getState().kind)) status.setIdle();
});

window.addEventListener('pagehide', () => {
    stopStatePolling();
    status.dispose();
}, { once: true });
