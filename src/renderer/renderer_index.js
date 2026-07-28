import { invoke } from '@tauri-apps/api/core';

import { initializeLanguage, t } from '../localization/i18n.js';
import { errorMessage } from './errors.js';
import { waitForE2eReady } from './e2e.js';
import { handleError, resetProgressUI, setupErrorModal, updateProgressBar, updateStatus } from './common.js';
import { listenUntilPageHide } from './event-listener.js';

setupErrorModal();

const playButton = document.getElementById('play-button');
const checkFilesButton = document.getElementById('check-files');
let verificationInProgress = false;
let gameRunning = false;

function updateUIBasedOnState() {
    if (checkFilesButton) checkFilesButton.textContent = t(verificationInProgress ? 'cancel' : 'check_files');
    if (playButton) playButton.disabled = verificationInProgress || gameRunning;
    if (checkFilesButton) checkFilesButton.disabled = gameRunning;
}

function setGameRunning(running) {
    gameRunning = running;
    updateUIBasedOnState();
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        await waitForE2eReady();
        await initializeLanguage();
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

        const state = await invoke('get_current_state');
        verificationInProgress = state.verificationInProgress;
        const gameState = await invoke('get_game_process_state');
        gameRunning = gameState.kind === 'starting' || gameState.kind === 'running';
        updateUIBasedOnState();
        document.body.classList.add('fade-in');
    } catch (error) {
        handleError(null, `${t('stageMessages.error_loading_data')}: ${errorMessage(error)}`);
    }
});

playButton?.addEventListener('click', async () => {
    if (verificationInProgress || gameRunning) return;
    verificationInProgress = true;
    updateUIBasedOnState();
    if (checkFilesButton) checkFilesButton.disabled = true;
    updateStatus('searching_updates', 'launcher-status');

    try {
        await invoke('update_game');
        updateStatus('verifying', 'launcher-status');
        await invoke('verify_files', { checkAllFiles: false });
        await invoke('update_rev_ini', {
            launchParams: document.getElementById('launch-params').value,
            clanTag: document.getElementById('clan-tag').value,
            nickname: document.getElementById('nickname').value
        });
        await invoke('launch_game');
    } catch (error) {
        handleError(null, `${t('error_launching_game')}: ${errorMessage(error)}`);
    } finally {
        verificationInProgress = false;
        if (checkFilesButton) checkFilesButton.disabled = false;
        updateUIBasedOnState();
    }
});

checkFilesButton?.addEventListener('click', async () => {
    if (gameRunning) return;
    if (verificationInProgress) {
        await invoke('cancel_verify');
        return;
    }

    verificationInProgress = true;
    updateUIBasedOnState();
    try {
        await invoke('verify_files', { checkAllFiles: true });
        resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'files_good');
    } catch (error) {
        if (error?.code === 'canceled') {
            resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'verification_canceled');
        } else {
            resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'error');
            handleError(null, `${t('stageMessages.error_starting_verification')}: ${errorMessage(error)}`);
        }
    } finally {
        verificationInProgress = false;
        updateUIBasedOnState();
    }
});

listenUntilPageHide('verify-progress', ({ payload }) => {
    updateProgressBar(
        payload.progress, payload.stage, payload.timeRemainingSec,
        'progress-bar', 'launcher-status', 'progress-info'
    );
});

listenUntilPageHide('game-starting', () => setGameRunning(true));
listenUntilPageHide('game-started', () => setGameRunning(true));
listenUntilPageHide('game-closed', () => setGameRunning(false));
