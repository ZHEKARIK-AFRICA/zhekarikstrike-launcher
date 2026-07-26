// renderer_launcher_update.js

import { updateProgressBar, handleError, updateStatus } from './common.js';

let updateFailed = false;

function continueWithoutUpdate() {
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');
    document.body.addEventListener('transitionend', function handler() {
        document.body.removeEventListener('transitionend', handler);
        window.electronAPI.navigateToPage('./public/intro.html');
    });
}

function ensureContinueButton() {
    let button = document.getElementById('continue-without-update');
    if (button) return button;

    button = document.createElement('button');
    button.id = 'continue-without-update';
    button.className = 'modal-ok';
    button.type = 'button';
    button.textContent = 'продолжить без обновления';
    button.addEventListener('click', continueWithoutUpdate);

    const footer = document.querySelector('footer');
    footer?.appendChild(button);
    return button;
}

document.addEventListener('DOMContentLoaded', async () => {
    console.log('Launcher update page loaded');
    document.body.classList.add('fade-in');

    window.electronAPI.on('update-progress', async (event, { progress, stage, timeRemaining, errorMessage }) => {
        updateProgressBar(progress, stage, timeRemaining, 'progress-bar', 'update-status', 'progress-info');

        if (errorMessage) {
            console.error(`Error during update: ${errorMessage}`);
            handleError(event, `${await window.electronAPI.t('stageMessages.error')}: ${errorMessage}`);
        }
    });

    window.electronAPI.on('launcher-update-ready', async () => {
        if (updateFailed) return;
        updateStatus('complete', 'update-status');
        try {
            await window.electronAPI.invoke('apply-launcher-update');
        } catch (error) {
            updateFailed = true;
            ensureContinueButton();
            handleError(null, `${await window.electronAPI.t('stageMessages.error')}: ${error.message}`);
        }
    });

    window.electronAPI.on('launcher-update-error', async (event, errorMessage) => {
        updateFailed = true;
        ensureContinueButton();
        handleError(event, `${await window.electronAPI.t('stageMessages.error')}: ${errorMessage}`);
    });

    window.electronAPI.on('launcher-update-applied', () => {
        updateStatus('complete', 'update-status');
    });

    try {
        await window.electronAPI.invoke('download-launcher-update');
    } catch (error) {
        updateFailed = true;
        ensureContinueButton();
        handleError(null, `${await window.electronAPI.t('stageMessages.error')}: ${error.message}`);
    }
});
