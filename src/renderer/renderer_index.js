// renderer_index.js
import { handleError, setupErrorModal, updateProgressBar, resetProgressUI, updateStatus } from './common.js';


setupErrorModal();

const playButton = document.getElementById('play-button');
const checkFilesButton = document.getElementById('check-files');

let verificationInProgress = false;

async function updateUIBasedOnState() {
    if (verificationInProgress) {
        checkFilesButton.textContent = await window.electronAPI.t('cancel'); // Используем window.electronAPI.t
        if (playButton) {
            playButton.disabled = true;
        }
    } else {
        checkFilesButton.textContent = await window.electronAPI.t('check_files'); // Используем window.electronAPI.t
        if (playButton) {
            playButton.disabled = false;
        }
    }
}

document.addEventListener('DOMContentLoaded', async () => {
    try {
        // Получаем данные о игре: путь, никнейм, клантег, параметры запуска
        const { nickname, clanTag, launchParams, gamePath } = await window.electronAPI.invoke('get-game-data');

        // Заполняем параметры запуска
        document.getElementById('launch-params').value = launchParams || '';
        document.getElementById('clan-tag').value = clanTag || '';
        document.getElementById('nickname').value = nickname || '';

        // Заполняем путь к игре
        const gamePathInput = document.getElementById('game-path');
        gamePathInput.value = gamePath || await window.electronAPI.t('stageMessages.game_path_not_found'); // Используем stageMessages.game_path_not_found
        gamePathInput.readOnly = true; // Поле не редактируемое

        document.dispatchEvent(new CustomEvent('gameDataLoaded', {
            detail: { nickname, clanTag, launchParams, gamePath }
        }));

        const state = await window.electronAPI.invoke('get-current-state');
        console.log('Current state:', state);

        verificationInProgress = state.verificationInProgress;
        updateUIBasedOnState();
        document.body.classList.add('fade-in');
    } catch (error) {
        handleError(null, await window.electronAPI.t('stageMessages.error_loading_data') + ': ' + error.message);
    }
});

function fadeOutAndNavigate(page) {
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');

    document.body.addEventListener('transitionend', function handler() {
        document.body.removeEventListener('transitionend', handler);
        // Send message to main process for navigation
        window.electronAPI.navigateToPage(page);
    });
}

if (playButton) {
    playButton.addEventListener('click', async () => {
        if (verificationInProgress) return;

        verificationInProgress = true;
        playButton.disabled = true;
        if (checkFilesButton) checkFilesButton.disabled = true;

        updateStatus('searching_updates', 'launcher-status');

        try {
            await window.electronAPI.invoke('update-game');
            console.log('Game update completed successfully');

            updateStatus('verifying', 'launcher-status');

            await window.electronAPI.invoke('verify-files', false);
            console.log('Verification before launch completed successfully');

            const launchParams = document.getElementById('launch-params').value;
            const clanTag = document.getElementById('clan-tag').value;
            const nickname = document.getElementById('nickname').value;

            await window.electronAPI.invoke('update-rev-ini', { nickname, clanTag, launchParams });
            console.log('rev.ini обновлен успешно.');

            await window.electronAPI.invoke('launch-game');
            console.log('Game launched successfully');
        } catch (error) {
            handleError(null,await window.electronAPI.t('error_launching_game') + ': ' + error.message);
        } finally {
            console.log('Finally block executed');
            verificationInProgress = false;
            if (checkFilesButton) {
                checkFilesButton.disabled = false;
                console.log('checkFilesButton.disabled:', checkFilesButton.disabled);
            }
            if (playButton) {
                playButton.disabled = false;
            }
        }
    });
}

if (checkFilesButton) {
    checkFilesButton.addEventListener('click', async () => {
        if (!verificationInProgress) {
            console.log('Starting file integrity check...');
            verificationInProgress = true;
            checkFilesButton.textContent =await window.electronAPI.t('stageMessages.cancel');

            if (playButton) {
                playButton.disabled = true;
            }

            try {
                await window.electronAPI.invoke('verify-files');
                console.log('Verification started');
            } catch (error) {
                handleError(null,await window.electronAPI.t('stageMessages.error_starting_verification') + ': ' + error.message);
                verificationInProgress = false;
                checkFilesButton.textContent =await window.electronAPI.t('check_files');
                if (playButton) {
                    playButton.disabled = false;
                }
            }
        } else {
            console.log('Cancelling verification...');
            const canceled = await window.electronAPI.invoke('cancel-verify');
            if (canceled) {
                console.log('Verification canceled by user');
            } else {
                console.log('No active verification to cancel');
            }
        }
    });
}

window.electronAPI.on('verify-progress', (event, { progress, stage, timeRemaining, errorMessage }) => {
    updateProgressBar(progress, stage, timeRemaining, 'progress-bar', 'launcher-status', 'progress-info');

    if (errorMessage) {
        handleError(event, errorMessage);
    }
});

window.electronAPI.on('verify-complete', async () => {
    console.log('Verification completed successfully');
    verificationInProgress = false;
    checkFilesButton.textContent =await window.electronAPI.t('check_files');
    if (playButton) {
        playButton.disabled = false;
    }
    resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'files_good');
});

window.electronAPI.on('verify-canceled', async () => {
    console.log('Verification canceled');
    verificationInProgress = false;
    checkFilesButton.textContent =await window.electronAPI.t('check_files');
    if (playButton) {
        playButton.disabled = false;
    }
    resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'verification_canceled');
});

window.electronAPI.on('verify-error', async (event, errorMessage) => {
    console.error('Verification error:', errorMessage);
    verificationInProgress = false;
    checkFilesButton.textContent =await window.electronAPI.t('check_files');
    if (playButton) {
        playButton.disabled = false;
    }
    resetProgressUI('progress-bar', 'launcher-status', 'progress-info', 'error');
    handleError(event, errorMessage);
});

window.electronAPI.on('launch-error', (event, message) => {
    updateStatus('error', 'launcher-status');
    handleError(event, message);
});